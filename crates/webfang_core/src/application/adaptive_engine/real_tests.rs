use super::*;
use crate::domain::dom_inspector::{DomStructureReport, SelectorSuggestion};
use crate::domain::semantic_inspector::{
    BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
};
use std::sync::Arc;

/// Mock DOM inspector — returns configurable suggestions.
struct MockInspector {
    suggestions: Vec<SelectorSuggestion>,
}

impl MockInspector {
    fn new(suggestions: Vec<SelectorSuggestion>) -> Self {
        Self { suggestions }
    }
}

impl DomInspectorPort for MockInspector {
    fn inspect(&self, _document: &scraper::Html) -> DomStructureReport {
        DomStructureReport::default()
    }

    fn suggest(
        &self,
        _document: &scraper::Html,
        _failed_selector: &str,
    ) -> Vec<SelectorSuggestion> {
        self.suggestions.clone()
    }
}

/// Mock semantic inspector — returns a configurable match.
struct MockSemanticInspector(Option<SemanticMatch>);

impl SemanticInspectorPort for MockSemanticInspector {
    fn find_semantic_match<'a>(
        &'a self,
        _ctx: SemanticContext,
    ) -> BoxFuture<'a, Result<Option<SemanticMatch>, SelectorErrorKind>> {
        let response = self.0.clone();
        Box::pin(async { Ok(response) })
    }
}

/// Tier 2 provider that never matches.
fn semantic_no_match() -> Arc<MockSemanticInspector> {
    Arc::new(MockSemanticInspector(None))
}

/// Tier 2 provider returning a single match for `selector` at `confidence`.
fn semantic_match(selector: &str, confidence: f32) -> Arc<MockSemanticInspector> {
    Arc::new(MockSemanticInspector(Some(SemanticMatch {
        selector: selector.to_owned(),
        confidence,
        source: TierSource::Semantic,
    })))
}

/// Ambiguous Tier 1 inspector (score 0.65 ≤ 0.85) — the cascade must
/// escalate to Tier 2 instead of settling for the lexical suggestion.
fn ambiguous_tier1() -> Arc<MockInspector> {
    Arc::new(MockInspector::new(vec![SelectorSuggestion {
        selector: ".ambiguous".to_owned(),
        score: 0.65,
    }]))
}

#[tokio::test]
async fn test_select_sync_aware_degraded_when_tier2_returns_none() {
    // Tier 1 returns low score (0.65 < 0.70 threshold), Tier 2 returns None
    // → the Tier 1 best suggestion is returned as the fallback
    let inspector = Arc::new(MockInspector::new(vec![SelectorSuggestion {
        selector: ".low-score".to_owned(),
        score: 0.65,
    }]));
    let semantic = semantic_no_match();

    let engine = AdaptiveSelectorEngine::new(
        inspector,
        Some(semantic),
        AdaptiveSelectorOptions::default(),
    );
    let html = "<div>Hello</div>".to_owned();

    let result = engine
        .select_sync_aware(html, ".missing".to_owned(), None)
        .await;
    assert!(result.is_ok());
    let outcome = result.unwrap();
    // The Tier 1 best is returned as the fallback, so the status is
    // `Repaired` (not `Degraded`) — `Degraded` is reserved for a Tier 2
    // match whose confidence is below the semantic threshold.
    assert_eq!(outcome.suggestion.selector, ".low-score");
    assert_eq!(outcome.status, RepairStatus::Repaired);
    let trace = outcome.trace.as_ref().expect("repair carries a trace");
    assert!(
        trace.tier2_score.is_none(),
        "Tier 2 produced no match, so the trace carries no tier2 score"
    );
}

#[tokio::test]
async fn test_select_sync_aware_degraded_when_no_tier1_and_tier2_returns_none() {
    // Tier 1 returns empty, Tier 2 returns None → RepairInconclusive error
    let inspector = Arc::new(MockInspector::new(vec![]));
    let semantic = semantic_no_match();

    let engine = AdaptiveSelectorEngine::new(
        inspector,
        Some(semantic),
        AdaptiveSelectorOptions::default(),
    );
    let html = "<div>Hello</div>".to_owned();

    let result = engine
        .select_sync_aware(html, ".missing".to_owned(), None)
        .await;
    assert!(result.is_err());
    match result.unwrap_err() {
        SelectorErrorKind::RepairInconclusive(_) => {},
        other => panic!("expected RepairInconclusive, got {other:?}"),
    }
}

#[tokio::test]
async fn test_select_sync_aware_tier1_high_score_returns_immediately() {
    // Tier 1 returns high score (0.9 > 0.85 threshold) → immediate return
    let inspector = Arc::new(MockInspector::new(vec![SelectorSuggestion {
        selector: ".high-score".to_owned(),
        score: 0.9,
    }]));
    let semantic = semantic_no_match();

    let engine = AdaptiveSelectorEngine::new(
        inspector,
        Some(semantic),
        AdaptiveSelectorOptions::default(),
    );
    let html = "<div>Hello</div>".to_owned();

    let result = engine
        .select_sync_aware(html, ".content".to_owned(), None)
        .await;
    assert!(result.is_ok());
    let outcome = result.unwrap();
    assert_eq!(outcome.status, RepairStatus::Repaired);
    assert_eq!(outcome.suggestion.selector, ".high-score");
}

/// The cache must be keyed by (selector, structural hash): a second call
/// with the same inputs is served from the cache instead of re-running the
/// cascade.
#[tokio::test]
async fn test_select_sync_aware_cache_hit_on_second_call() {
    let inspector = Arc::new(MockInspector::new(vec![SelectorSuggestion {
        selector: ".cached".to_owned(),
        score: 0.9,
    }]));
    let semantic = semantic_no_match();

    let engine = AdaptiveSelectorEngine::new(
        inspector,
        Some(semantic),
        AdaptiveSelectorOptions::default(),
    );
    let html = "<div class='cached'>Hello</div>".to_owned();

    // First call — populates the cache
    let first = engine
        .select_sync_aware(html.clone(), ".target".to_owned(), None)
        .await
        .expect("tier1 repair must succeed");
    assert!(!first.trace.as_ref().expect("trace").cache_hit);
    assert_eq!(
        engine.cache_len(),
        1,
        "cache should hold one entry after the first call"
    );

    // Second call — served from the cache
    let second = engine
        .select_sync_aware(html, ".target".to_owned(), None)
        .await
        .expect("cached repair must succeed");
    assert!(
        second.trace.as_ref().expect("trace").cache_hit,
        "second call with the same selector and DOM must hit the cache"
    );
}

/// Tier 2 returns a confident match → the repaired selector comes from
/// the semantic provider and the trace carries the Tier 2 score (the
/// marker that the semantic tier ran).
#[tokio::test]
async fn test_repair_resolves_via_tier2_when_confidence_above_threshold() {
    // Tier 1 stays ambiguous (0.65 ≤ 0.85) so the cascade escalates to Tier 2.
    let inspector = ambiguous_tier1();
    let semantic = semantic_match(".semantic-hit", 0.9); // above semantic_threshold (0.75)

    let engine = AdaptiveSelectorEngine::new(
        inspector,
        Some(semantic),
        AdaptiveSelectorOptions::default(),
    );
    let html = "<div class='semantic-hit'>Hello</div>".to_owned();

    let result = engine
        .select_sync_aware(html, ".content".to_owned(), None)
        .await;
    assert!(result.is_ok(), "tier2 repair should succeed");
    let outcome = result.unwrap();
    assert_eq!(outcome.status, RepairStatus::Repaired);
    assert_eq!(outcome.suggestion.selector, ".semantic-hit");
    let trace = outcome
        .trace
        .as_ref()
        .expect("tier2 repair carries a trace");
    assert_eq!(
        trace.tier2_score,
        Some(0.9),
        "trace must record the tier2_semantic score"
    );
    assert!(!trace.cache_hit);
}

/// Tier 2 returns a low-confidence match → Degraded, but the Tier 2 score
/// must still be visible in the trace (the semantic tier ran).
#[tokio::test]
async fn test_repair_degraded_when_tier2_confidence_below_threshold() {
    let inspector = ambiguous_tier1();
    let semantic = semantic_match(".semantic-weak", 0.6); // below semantic_threshold (0.75)

    let engine = AdaptiveSelectorEngine::new(
        inspector,
        Some(semantic),
        AdaptiveSelectorOptions::default(),
    );
    let html = "<div class='semantic-weak'>Hello</div>".to_owned();

    let result = engine
        .select_sync_aware(html, ".content".to_owned(), None)
        .await;
    assert!(result.is_ok(), "low-confidence tier2 still returns Ok");
    let outcome = result.unwrap();
    assert_eq!(
        outcome.status,
        RepairStatus::Degraded,
        "confidence below 0.75 must mark the repair degraded"
    );
    assert_eq!(outcome.suggestion.selector, ".semantic-weak");
    assert_eq!(outcome.trace.as_ref().unwrap().tier2_score, Some(0.6));
}

#[test]
fn has_semantic_provider_reflects_wiring() {
    let inspector = Arc::new(MockInspector::new(vec![]));

    let tier1_only =
        AdaptiveSelectorEngine::new(inspector.clone(), None, AdaptiveSelectorOptions::default());
    assert!(
        !tier1_only.has_semantic_provider(),
        "shared=None must leave a Tier-1-only engine"
    );

    let wired = AdaptiveSelectorEngine::new(
        inspector,
        Some(semantic_no_match()),
        AdaptiveSelectorOptions::default(),
    );
    assert!(
        wired.has_semantic_provider(),
        "a semantic provider must be visible on the engine"
    );
}
