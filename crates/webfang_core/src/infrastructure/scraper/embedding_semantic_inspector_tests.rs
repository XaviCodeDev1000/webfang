//! Tests for [`EmbeddingSemanticInspector`].
//!
//! Extracted from `embedding_semantic_inspector.rs` to bring it under the
//! ~500-line file-size rule. Movement is verbatim; imports were adjusted from
//! `use super::*` (which resolved against the production file) to explicit
//! paths against `super::embedding_semantic_inspector` and the domain ports.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::embedding_semantic_inspector::EmbeddingSemanticInspector;
use crate::domain::dom_inspector::SelectorErrorKind;
use crate::domain::embedding_port::{cosine_similarity_fail_closed, EmbeddingPort};
use crate::domain::semantic_inspector::{
    BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
};
use crate::error::SemanticError;

/// Deterministic embedding port with call accounting.
///
/// `table` maps an exact text to the vector it must embed to; anything
/// else falls back to a one-hot of width `fallback_dim` at
/// `text.len() % fallback_dim`. `drop_last` makes `embed_batch` return one
/// vector short of its input, which no honest port does — it exists so the
/// short-output guard has a test.
#[derive(Default)]
struct MockEmbedder {
    table: HashMap<String, Vec<f32>>,
    fallback_dim: usize,
    drop_last: bool,
    embed_calls: AtomicUsize,
    batch_calls: AtomicUsize,
    batches: std::sync::Mutex<Vec<Vec<String>>>,
}

impl MockEmbedder {
    fn new(fallback_dim: usize, table: &[(&str, &[f32])]) -> Self {
        Self {
            table: table
                .iter()
                .map(|(text, vec)| ((*text).to_owned(), (*vec).to_vec()))
                .collect(),
            fallback_dim,
            ..Self::default()
        }
    }

    fn vector_for(&self, text: &str) -> Vec<f32> {
        if let Some(vector) = self.table.get(text) {
            return vector.clone();
        }
        let mut vector = vec![0.0f32; self.fallback_dim];
        if !vector.is_empty() {
            vector[text.len() % self.fallback_dim] = 1.0;
        }
        vector
    }

    fn embed_calls(&self) -> usize {
        self.embed_calls.load(Ordering::SeqCst)
    }

    fn batch_calls(&self) -> usize {
        self.batch_calls.load(Ordering::SeqCst)
    }

    fn batched_texts(&self) -> Vec<Vec<String>> {
        self.batches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EmbeddingPort for MockEmbedder {
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
        self.embed_calls.fetch_add(1, Ordering::SeqCst);
        let vector = self.vector_for(text);
        Box::pin(async move { Ok(vector) })
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
        self.batch_calls.fetch_add(1, Ordering::SeqCst);
        self.batches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(texts.to_vec());
        let mut vectors: Vec<Vec<f32>> = texts.iter().map(|t| self.vector_for(t)).collect();
        if self.drop_last {
            vectors.pop();
        }
        Box::pin(async move { Ok(vectors) })
    }

    fn embedding_dim(&self) -> usize {
        self.fallback_dim
    }

    fn model_id(&self) -> &str {
        "mock-embedder"
    }
}

fn context(target: &str, fragments: &[&str]) -> SemanticContext {
    SemanticContext {
        target_text: target.to_owned(),
        dom_fragments: fragments.iter().map(|f| (*f).to_owned()).collect(),
        domain_hint: None,
    }
}

/// Erase the mock for the inspector while the test keeps a handle to count
/// its calls. `Arc::clone` would pick the generic signature and block the
/// unsized coercion, so the clone is a plain method call on the argument.
fn inspector_over(embedder: &Arc<MockEmbedder>, threshold: f32) -> EmbeddingSemanticInspector {
    EmbeddingSemanticInspector::new(embedder.clone(), threshold)
}

/// The four-entry table both batch tests arrange with: one target plus
/// three fragments of which only `third` is parallel to the target, so a
/// reordered batch is visible as the wrong winner rather than as a tie.
/// Named because the two tests assert DIFFERENT properties over it (order
/// preservation vs. window reassembly) and must keep their own assertions —
/// only the arrange is shared.
fn target_with_one_winner() -> [(&'static str, &'static [f32]); 4] {
    [
        ("target", &[1.0, 0.0]),
        ("first", &[0.0, 1.0]),
        ("second", &[0.0, 1.0]),
        ("third", &[1.0, 0.0]),
    ]
}

#[tokio::test]
async fn match_above_threshold_returns_the_best_fragment() {
    let embedder = MockEmbedder::new(2, &[("target", &[1.0, 0.0]), ("match", &[1.0, 0.0])]);
    let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

    let hit = inspector
        .find_semantic_match(context("target", &["other", "match"]))
        .await
        .expect("inspection must not error")
        .expect("cosine 1.0 clears the 0.75 threshold");

    assert_eq!(hit.selector, "match");
    assert!(
        (hit.confidence - 1.0).abs() < 1e-5,
        "got {}",
        hit.confidence
    );
    assert_eq!(hit.source, TierSource::Semantic);
}

#[tokio::test]
async fn no_match_below_threshold() {
    let embedder = MockEmbedder::new(2, &[("target", &[1.0, 0.0]), ("other", &[0.0, 1.0])]);
    let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

    let result = inspector
        .find_semantic_match(context("target", &["other"]))
        .await
        .expect("inspection must not error");

    assert_eq!(result, None, "cosine 0.0 must not clear the threshold");
}

/// The regression this whole slice exists for: a dimension mismatch used to
/// be truncated to the shorter vector, so a 2-wide fragment identical to a
/// 4-wide target scored a perfect 1.0 and WON. Fail-closed means the
/// mismatch can never produce a match.
#[tokio::test]
async fn dimension_mismatch_is_skipped_never_scored_as_a_match() {
    let embedder = MockEmbedder::new(
        4,
        &[("target", &[1.0, 0.0, 0.0, 0.0]), ("short", &[1.0, 0.0])],
    );
    let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

    let result = inspector
        .find_semantic_match(context("target", &["short"]))
        .await
        .expect("a mismatch degrades, it does not fail the call");

    assert_eq!(
        result, None,
        "a 2-wide fragment must not be truncated into a match against a 4-wide target"
    );
}

/// Triangulates the skip policy: one broken vector must not discard the
/// candidates that ARE comparable.
#[tokio::test]
async fn dimension_mismatch_does_not_discard_comparable_fragments() {
    let embedder = MockEmbedder::new(
        4,
        &[
            ("target", &[1.0, 0.0, 0.0, 0.0]),
            ("match", &[1.0, 0.0, 0.0, 0.0]),
            ("short", &[1.0, 0.0]),
        ],
    );
    let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

    let hit = inspector
        .find_semantic_match(context("target", &["short", "match"]))
        .await
        .expect("inspection must not error")
        .expect("the comparable fragment still matches");

    assert_eq!(hit.selector, "match");
}

#[tokio::test]
async fn empty_dom_fragments_returns_none_without_calling_the_port() {
    let embedder = Arc::new(MockEmbedder::new(2, &[]));
    let inspector = inspector_over(&embedder, 0.75);

    let result = inspector
        .find_semantic_match(context("target", &[]))
        .await
        .expect("inspection must not error");

    assert_eq!(result, None);
    assert_eq!(
        embedder.batch_calls(),
        0,
        "an empty candidate set must not reach the embedding port"
    );
    assert_eq!(embedder.embed_calls(), 0);
}

#[tokio::test]
async fn batch_order_is_preserved() {
    // Reversed embedding order would attribute "first" to "third".
    let embedder = MockEmbedder::new(2, &target_with_one_winner());
    let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

    let hit = inspector
        .find_semantic_match(context("target", &["first", "second", "third"]))
        .await
        .expect("inspection must not error")
        .expect("only 'third' is parallel to the target");

    assert_eq!(hit.selector, "third");
}

#[tokio::test]
async fn one_embed_batch_replaces_per_fragment_inference() {
    let embedder = Arc::new(MockEmbedder::new(2, &[("target", &[1.0, 0.0])]));
    let inspector = inspector_over(&embedder, 0.75);

    inspector
        .find_semantic_match(context("target", &["a", "b", "c", "d", "e"]))
        .await
        .expect("inspection must not error");

    assert_eq!(
        embedder.batch_calls(),
        1,
        "target + 5 fragments is ONE round trip, not N+1"
    );
    assert_eq!(
        embedder.embed_calls(),
        0,
        "the per-fragment path this slice removes must not run"
    );
    assert_eq!(
        embedder.batched_texts(),
        vec![vec![
            "target".to_owned(),
            "a".to_owned(),
            "b".to_owned(),
            "c".to_owned(),
            "d".to_owned(),
            "e".to_owned(),
        ]]
    );
}

#[tokio::test]
async fn batch_max_splits_into_capped_windows_and_keeps_order() {
    let embedder = Arc::new(MockEmbedder::new(2, &target_with_one_winner()));
    let inspector = inspector_over(&embedder, 0.75).with_batch_max(Some(2));

    let hit = inspector
        .find_semantic_match(context("target", &["first", "second", "third"]))
        .await
        .expect("inspection must not error")
        .expect("windowed batching must rank exactly like a single batch");

    assert_eq!(hit.selector, "third");
    assert_eq!(
        embedder.batched_texts(),
        vec![
            vec!["target".to_owned(), "first".to_owned()],
            vec!["second".to_owned(), "third".to_owned()],
        ],
        "windows must be consecutive and cap-sized, reassembled by position"
    );
}

#[tokio::test]
async fn short_port_output_is_an_error_not_a_wrong_ranking() {
    let embedder = MockEmbedder {
        drop_last: true,
        ..MockEmbedder::new(2, &[("target", &[1.0, 0.0])])
    };
    let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

    let result = inspector
        .find_semantic_match(context("target", &["first", "second"]))
        .await;

    assert!(
        result.is_err(),
        "a vector count that does not match the input must not be ranked"
    );
}

#[test]
fn cosine_similarity_fail_closed_reports_mismatch_as_none() {
    assert_eq!(
        cosine_similarity_fail_closed(&[1.0, 0.0], &[1.0, 0.0]),
        Some(1.0)
    );
    assert_eq!(
        cosine_similarity_fail_closed(&[1.0, 0.0], &[0.0, 1.0]),
        Some(0.0)
    );
    assert_eq!(cosine_similarity_fail_closed(&[], &[]), Some(0.0));
    assert_eq!(
        cosine_similarity_fail_closed(&[0.0, 0.0], &[1.0, 0.0]),
        Some(0.0)
    );
    assert_eq!(cosine_similarity_fail_closed(&[1.0, 0.0], &[1.0]), None);
    assert_eq!(
        cosine_similarity_fail_closed(&[1.0, 0.0], &[1.0, 0.0, 0.0]),
        None
    );
}

/// Positive control for the skip policy: equal widths with a real score are
/// compared, never rejected.
#[test]
fn equal_width_vectors_are_compared_even_when_not_parallel() {
    let score = cosine_similarity_fail_closed(&[1.0, 1.0], &[1.0, 0.0])
        .expect("equal widths must produce a score, not a skip");
    assert!(
        (score - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
        "got {score}"
    );
}
