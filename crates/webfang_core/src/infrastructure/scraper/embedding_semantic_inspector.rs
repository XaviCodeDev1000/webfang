//! Embedding-backed Tier 2 semantic inspector — infrastructure layer.
//!
//! Implements [`SemanticInspectorPort`] over the erased [`EmbeddingPort`] seam
//! (ADR-0004). The target text and every DOM fragment go out in ONE
//! `embed_batch` call; the returned vectors are ranked locally. Symmetric with
//! [`DefaultDomInspector`](super::dom_inspector::DefaultDomInspector): Tier 1 is
//! lexical and CPU-bound, Tier 2 asks the embedding port.
//!
//! # Why this type exists
//!
//! Two properties are load-bearing, and both are regressions against the
//! `GraniteDomInspector` it replaces:
//!
//! 1. **No ONNX, and one round trip.** Nothing here knows which backend
//!    produces the vectors, so Tier 2 survives the removal of the local ONNX
//!    stack and works unchanged against a remote endpoint. The old inspector
//!    issued one `infer` per DOM fragment — an N+1 round trip over the
//!    inference pool, paid again on every failed selector.
//! 2. **Fail-closed cosine.** The comparison never truncates to the shorter
//!    vector, and never treats a mismatch as "similarity 0". A dimension
//!    mismatch means the port broke its contract (a mixed-model index, a
//!    profile whose width changed mid-run), and letting such a vector rank is
//!    how corrupt data wins a comparison. Mismatched vectors are dropped from
//!    the ranking — see [`cosine_similarity_fail_closed`], the single domain
//!    copy of that contract, shared with `vault_search`.
//!
//! # Dimension-mismatch policy (degraded, not fatal)
//!
//! A mismatched fragment is SKIPPED, not turned into an error and not scored as
//! `0.0`. A mismatch is a property of one vector, and one bad vector in a batch
//! says nothing about its neighbours, so failing the whole call would discard
//! repair candidates that were perfectly comparable. When NO fragment remains
//! comparable the inspector degrades to `Ok(None)`: "Tier 2 found nothing",
//! which is the same signal a below-threshold result carries and which the
//! adaptive engine already handles by falling back to Tier 1
//! (`adaptive_engine/mod.rs`). The invariant this buys is the one that matters:
//! a mismatched vector can NEVER be reported as a match, and is never
//! indistinguishable from a genuine low score.
//!
//! # Batch cap
//!
//! [`ModelProfile::batch_max`](crate::domain::model_profile::ModelProfile::batch_max)
//! is honoured when the constructor is given one: a `[target, …fragments]`
//! batch longer than the cap is sent as consecutive windows of at most `cap`
//! texts and reassembled by position, so the index of every vector still
//! matches the index of its text. `None` (the default, and what every profile
//! declares today) means no cap and therefore exactly one round trip.
//! Declaring the cap is this type's job; deciding WHICH profile a run uses is
//! the composition edge's — no profile is wired to the CLI yet.

use std::sync::Arc;

use tracing::{debug, info_span, Instrument};

use crate::domain::dom_inspector::SelectorErrorKind;
use crate::domain::embedding_port::{cosine_similarity_fail_closed, EmbeddingPort};
use crate::domain::semantic_inspector::{
    BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
};
use crate::error::SemanticError;

/// Tier 2 semantic inspector over an erased embedding port.
///
/// The whole type is two fields: which port to ask, and the confidence bar a
/// candidate must clear. Everything backend-shaped — tokenizer, inference
/// engine, model resolution — belongs to the port implementation, which is why
/// this type compiles with no ONNX in the graph.
pub struct EmbeddingSemanticInspector {
    embedding: Arc<dyn EmbeddingPort>,
    threshold: f32,
    batch_max: Option<usize>,
}

impl EmbeddingSemanticInspector {
    /// Create an inspector over `embedding` that reports a match at or above
    /// `threshold`.
    ///
    /// No batch cap: `batch_max` stays `None` until
    /// [`with_batch_max`](Self::with_batch_max) says otherwise.
    #[must_use]
    pub fn new(embedding: Arc<dyn EmbeddingPort>, threshold: f32) -> Self {
        Self {
            embedding,
            threshold,
            batch_max: None,
        }
    }

    /// Cap how many texts go into a single `embed_batch` request.
    ///
    /// Mirrors
    /// [`ModelProfile::batch_max`](crate::domain::model_profile::ModelProfile::batch_max):
    /// `None` (and a cap of `0`, which is not a meaningful batch width) means
    /// no cap, so one request carries the whole `[target, …fragments]` batch. With a cap, the batch is split into
    /// consecutive windows and reassembled by position, so the returned vector
    /// at index `i` is still the embedding of the text at index `i`.
    #[must_use]
    pub fn with_batch_max(mut self, batch_max: Option<usize>) -> Self {
        self.batch_max = batch_max;
        self
    }

    /// Embed the whole batch, honouring the configured cap.
    ///
    /// Returns one vector per input text, in input order.
    async fn embed_all(&self, batch: &[String]) -> Result<Vec<Vec<f32>>, SelectorErrorKind> {
        match self.batch_max {
            Some(cap) if cap > 0 && batch.len() > cap => {
                let mut vectors = Vec::with_capacity(batch.len());
                for window in batch.chunks(cap) {
                    vectors.extend(
                        self.embedding
                            .embed_batch(window)
                            .await
                            .map_err(embedding_error)?,
                    );
                }
                Ok(vectors)
            },
            _ => self
                .embedding
                .embed_batch(batch)
                .await
                .map_err(embedding_error),
        }
    }
}

impl SemanticInspectorPort for EmbeddingSemanticInspector {
    fn find_semantic_match<'a>(
        &'a self,
        ctx: SemanticContext,
    ) -> BoxFuture<'a, Result<Option<SemanticMatch>, SelectorErrorKind>> {
        let span = info_span!(
            "semantic_inspector",
            fragments = ctx.dom_fragments.len(),
            threshold = self.threshold,
            batch_max = ?self.batch_max,
        );
        Box::pin(
            async move {
                if ctx.dom_fragments.is_empty() {
                    debug!("no dom fragments to compare against");
                    return Ok(None);
                }

                // One batch: target first, then the fragments in DOM order. The
                // vector at index i is the embedding of the text at index i,
                // which is what keeps the ranking below positional.
                let mut batch = Vec::with_capacity(ctx.dom_fragments.len() + 1);
                batch.push(ctx.target_text);
                batch.extend(ctx.dom_fragments);

                let vectors = self.embed_all(&batch).await?;
                if vectors.len() != batch.len() {
                    // Index-blind ranking would attribute a vector to the wrong
                    // fragment, so refuse the whole batch instead.
                    return Err(SelectorErrorKind::InvalidSelector(format!(
                        "embedding port returned {} vectors for {} inputs",
                        vectors.len(),
                        batch.len()
                    )));
                }

                let target = &vectors[0];
                let mut best: Option<(f32, &String)> = None;
                let mut skipped = 0usize;

                for (i, vector) in vectors.iter().enumerate().skip(1) {
                    // `None` = dimensions disagree: skip, never rank as 0.0.
                    let Some(score) = cosine_similarity_fail_closed(target, vector) else {
                        skipped += 1;
                        continue;
                    };
                    if best.is_none_or(|(top, _)| score > top) {
                        best = Some((score, &batch[i]));
                    }
                }

                debug!(
                    best_score = best.map_or(0.0, |(score, _)| score),
                    skipped_mismatched = skipped,
                    threshold = self.threshold,
                    "semantic match evaluated"
                );

                match best {
                    Some((score, selector)) if score >= self.threshold => Ok(Some(SemanticMatch {
                        selector: selector.clone(),
                        confidence: score,
                        source: TierSource::Semantic,
                    })),
                    _ => Ok(None),
                }
            }
            .instrument(span),
        )
    }
}

/// Map a port failure onto the inspector's error surface.
fn embedding_error(err: SemanticError) -> SelectorErrorKind {
    SelectorErrorKind::InvalidSelector(format!("embedding failed: {err}"))
}
