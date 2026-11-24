#[cfg(feature = "adaptive-selectors")]
use super::*;
#[cfg(feature = "adaptive-selectors")]
use webfang_core::application::adaptive_engine::AdaptiveSelectorOptions;
#[cfg(feature = "adaptive-selectors")]
use webfang_core::application::adaptive_engine::RepairStatus;
#[cfg(feature = "adaptive-selectors")]
use webfang_core::domain::dom_inspector::SelectorErrorKind;
#[cfg(feature = "adaptive-selectors")]
use webfang_core::domain::semantic_inspector::{
    BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
};

/// Mock semantic inspector returning a configurable match.
#[cfg(feature = "adaptive-selectors")]
struct MockSemanticInspector(Option<SemanticMatch>);

#[cfg(feature = "adaptive-selectors")]
impl SemanticInspectorPort for MockSemanticInspector {
    fn find_semantic_match<'a>(
        &'a self,
        _ctx: SemanticContext,
    ) -> BoxFuture<'a, Result<Option<SemanticMatch>, SelectorErrorKind>> {
        let response = self.0.clone();
        Box::pin(async { Ok(response) })
    }
}

#[cfg(feature = "adaptive-selectors")]
fn opts_with_adaptive(enabled: bool) -> CrawlOptions {
    CrawlOptions {
        adaptive_selectors: enabled,
        ..CrawlOptions::default()
    }
}

#[cfg(feature = "adaptive-selectors")]
#[test]
fn build_adaptive_engine_wires_semantic_provider_when_present() {
    let engine = build_adaptive_engine(
        &opts_with_adaptive(true),
        Some(Arc::new(MockSemanticInspector(None))),
        AdaptiveSelectorOptions::default(),
    )
    .expect("engine must exist when adaptive_selectors is enabled");

    assert!(
        engine.has_semantic_provider(),
        "wired semantic provider must be visible on the engine"
    );
}

#[cfg(feature = "adaptive-selectors")]
#[test]
fn build_adaptive_engine_degrades_to_tier1_without_semantic() {
    let engine = build_adaptive_engine(
        &opts_with_adaptive(true),
        None,
        AdaptiveSelectorOptions::default(),
    )
    .expect("engine must exist when adaptive_selectors is enabled");

    assert!(
        !engine.has_semantic_provider(),
        "no shared pool must leave a Tier-1-only engine"
    );
}

#[cfg(feature = "adaptive-selectors")]
#[test]
fn build_adaptive_engine_absent_when_flag_off() {
    assert!(
        build_adaptive_engine(
            &opts_with_adaptive(false),
            Some(Arc::new(MockSemanticInspector(None))),
            AdaptiveSelectorOptions::default()
        )
        .is_none(),
        "flag off must not build an engine even with a semantic provider available"
    );
}

/// End-to-end wiring evidence (AS-2): the engine built by the production
/// wiring function resolves a repair through Tier 2. `trace.tier2_score`
/// is `Some` only on the semantic path — Tier 1 outcomes always carry
/// `None`, so this marker is the cascade's own record of which tier
/// answered.
#[cfg(feature = "adaptive-selectors")]
#[tokio::test]
async fn wired_engine_resolves_repair_through_tier2() {
    let semantic_hit = Some(SemanticMatch {
        selector: ".semantic-hit".to_owned(),
        confidence: 0.9, // above semantic_threshold (0.75)
        source: TierSource::Semantic,
    });
    let engine = build_adaptive_engine(
        &opts_with_adaptive(true),
        Some(Arc::new(MockSemanticInspector(semantic_hit))),
        AdaptiveSelectorOptions::default(),
    )
    .expect("engine must exist when adaptive_selectors is enabled");

    let html = "<div class='semantic-hit'>Actual content</div>".to_owned();
    let outcome = engine
        .select_sync_aware(html, ".missing".to_owned(), None)
        .await
        .expect("tier2 repair must succeed with a confident semantic match");

    assert_eq!(outcome.status, RepairStatus::Repaired);
    assert_eq!(
        outcome.suggestion.selector, ".semantic-hit",
        "the repaired selector must come from the semantic provider"
    );
    let trace = outcome.trace.expect("tier2 repair carries a trace");
    assert_eq!(
        trace.tier2_score,
        Some(0.9),
        "the trace must record the tier2_semantic score"
    );
}

/// Recording `EmbeddingPort` that returns a fixed vector and counts the
/// texts it was asked to embed. Used to prove *which* port the inspector
/// is bound to, not just that it runs.
///
/// Wrapped in a newtype because the orphan rule forbids implementing a
/// foreign trait for `Arc<T>` — and `Arc<CountingEmbedder>` would be the
/// otherwise-natural shape.
///
/// Ungated since ADR-0004 step 6: the fixture needs only the core
/// `EmbeddingPort` trait, and the test it serves (the Tier 2 slot stays
/// bound to the port it was handed) is a TYPE-level invariant that
/// outlived the local ONNX pool.
#[cfg(feature = "adaptive-selectors")]
struct CountingEmbedder {
    inner: Arc<CountingInner>,
}

#[cfg(feature = "adaptive-selectors")]
struct CountingInner {
    vec: Vec<f32>,
    embedded: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "adaptive-selectors")]
impl CountingEmbedder {
    fn new(vec: Vec<f32>) -> Self {
        Self {
            inner: Arc::new(CountingInner {
                vec,
                embedded: std::sync::atomic::AtomicUsize::new(0),
            }),
        }
    }

    fn as_port(&self) -> Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort> {
        Arc::new(Self {
            inner: Arc::clone(&self.inner),
        })
    }

    fn embedded(&self) -> usize {
        self.inner
            .embedded
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(feature = "adaptive-selectors")]
impl webfang_core::domain::embedding_port::EmbeddingPort for CountingEmbedder {
    fn embed<'a>(
        &'a self,
        _text: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<f32>, webfang_core::error::SemanticError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.inner
                .embedded
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.inner.vec.clone())
        })
    }

    fn embedding_dim(&self) -> usize {
        self.inner.vec.len()
    }

    fn model_id(&self) -> &str {
        "counting-embedder"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Pins the SHAPE of the Tier 2 wiring: an inspector is built only from a
/// `Tier2EmbeddingPort`, and it embeds through the port that type carries.
///
/// What this test does NOT claim: it does not prove the inspector is
/// *prevented* from reaching the vault slot. `vault_slot` below is built
/// and then never handed to anything — `vault_provider.embedded() == 0` is
/// true by construction, not by an observed refusal. That invariant is
/// carried by the type system: `semantic_inspector` takes
/// `Option<Tier2EmbeddingPort>` while `VaultAiPorts::embedding_port` is
/// `Option<Arc<dyn EmbeddingPort>>`, so passing the slot is a compile
/// error (issue #1977). Compile-time evidence is the T1 swap experiment,
/// not this test.
///
/// What it does prove: the port the inspector embeds through is the one
/// handed in, all the way down to the adapter — the part that would break
/// first if `semantic_inspector` or `EmbeddingSemanticInspector::new`
/// started resolving its own port.
///
/// The LOCAL pool this test used to model is gone (ADR-0004 step 6); what
/// the test still pins is the TYPE separation — the Tier 2 slot and the
/// vault slot are different types, so the vault slot can never reach the
/// inspector by accident (issue #1977).
#[cfg(feature = "adaptive-selectors")]
#[test]
fn tier2_inspector_stays_bound_to_the_port_it_was_handed() {
    let handed = Arc::new(CountingEmbedder::new(vec![1.0, 0.0]));
    let vault_provider = Arc::new(CountingEmbedder::new(vec![0.0, 1.0]));

    let tier2_port =
        Some(webfang_core::domain::embedding_port::Tier2EmbeddingPort::new(handed.as_port()));

    // Models the composition order: the vault slot holds a DIFFERENT
    // provider than the one the inspector was handed. The inspector only
    // ever sees `tier2_port`.
    let vault_slot: Option<Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>> =
        Some(vault_provider.as_port());

    let inspector = semantic_inspector(tier2_port, &AdaptiveSelectorOptions::default())
        .expect("a wired Tier 2 port must produce an inspector");

    // Drive one Tier 2 call through the port-backed inspector.
    let ctx = SemanticContext {
        target_text: "actual content".to_owned(),
        dom_fragments: vec![".content".to_owned()],
        domain_hint: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    let outcome = runtime.block_on(inspector.find_semantic_match(ctx));

    assert!(
        handed.embedded() > 0,
        "the inspector must embed through the port it was handed"
    );
    assert_eq!(
        vault_provider.embedded(),
        0,
        "the inspector reached a port it was never given"
    );
    // `vault_slot` exists purely to model the overwrite; binding the
    // inspector must not consult it.
    assert!(vault_slot.is_some());
    assert!(outcome.is_ok(), "the Tier 2 call must not error");
}
