//! Relevance scoring for the semantic cleaner (ADR-0004 step 6).
//!
//! Mean-pooled centroid distances, Z-score adaptive filtering and the
//! over-aggressive-filter warning. Extracted from `semantic_cleaner.rs`
//! without behavior changes.

use crate::domain::embedding_port::cosine_similarity_fail_closed;
use crate::domain::entities::content::DocumentChunk;
use crate::error::SemanticError;

/// Threshold at which filtering is reported as aggressive (>50 % discarded).
const AGGRESSIVE_FILTER_RATIO: f64 = 0.5;

/// Mean-pooled centroid of `embeddings`, then the distance
/// (`1 - cosine similarity`) of every vector to it.
pub(super) fn distances_to_centroid(embeddings: &[Vec<f32>]) -> Result<Vec<f32>, SemanticError> {
    let dim = embeddings.first().map_or(0, Vec::len);
    if dim == 0 {
        return Err(SemanticError::Inference(
            "No embeddings available for relevance scoring".to_string(),
        ));
    }
    let mut centroid = vec![0.0_f32; dim];
    for embedding in embeddings {
        for (&val, slot) in embedding.iter().zip(centroid.iter_mut()) {
            *slot += val;
        }
    }
    let n = embeddings.len() as f32;
    for val in &mut centroid {
        *val /= n;
    }
    embeddings
        .iter()
        .map(|emb| {
            cosine_similarity_fail_closed(emb, &centroid)
                .map(|similarity| 1.0 - similarity)
                .ok_or_else(|| {
                    SemanticError::Inference(format!(
                        "Dimension mismatch in relevance scoring: vector {} vs centroid {dim}",
                        emb.len()
                    ))
                })
        })
        .collect()
}

/// Z-score adaptive filtering: chunks whose centroid distance sits more than
/// `z_limit` deviations from the mean distance are statistical outliers and
/// are dropped; survivors carry their embedding in `embeddings`.
///
/// Thresholds map to strictness: `1.0` → Z=0 (centroid-only), `0.0` → Z=3
/// (~99.7 % kept), the `0.3` default → Z=2.1.
pub(super) fn z_score_filter(
    chunks: Vec<DocumentChunk>,
    embeddings: Vec<Vec<f32>>,
    relevance_threshold: f32,
    distances: &[f32],
) -> Vec<DocumentChunk> {
    let sample_count = distances.len() as f32;
    let mean_dist = distances.iter().copied().sum::<f32>() / sample_count;
    let variance = distances
        .iter()
        .map(|d| (d - mean_dist).powi(2))
        .sum::<f32>()
        / sample_count;
    let std_dev = variance.sqrt();
    let z_limit = 3.0 * (1.0 - relevance_threshold);

    chunks
        .into_iter()
        .zip(embeddings)
        .zip(distances.iter())
        .filter(|(_, &distance)| {
            let z_score = (distance - mean_dist).abs() / std_dev.max(1e-6);
            z_score <= z_limit
        })
        .map(|((mut chunk, embedding), _)| {
            chunk.embeddings = Some(embedding);
            chunk
        })
        .collect()
}

/// `warn!` when the filter discards more than half of the chunks, so an
/// over-aggressive threshold or off-topic page is queryable from the trace.
pub(super) fn warn_aggressive_filter(url: &str, chunks_before: usize, chunks_after: usize) {
    let filtered_out = chunks_before.saturating_sub(chunks_after);
    let loss_ratio = filtered_out as f64 / chunks_before as f64;
    if chunks_before > 0 && loss_ratio > AGGRESSIVE_FILTER_RATIO {
        tracing::warn!(
            url = %url,
            chunks_before = chunks_before,
            chunks_after = chunks_after,
            filtered_out = filtered_out,
            loss_ratio = format!("{:.0}%", loss_ratio * 100.0),
            "Relevance filter discarded >50% of chunks for this page — possible over-aggressive threshold"
        );
    }
}
