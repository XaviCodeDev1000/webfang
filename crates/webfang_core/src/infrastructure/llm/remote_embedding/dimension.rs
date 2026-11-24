//! Dimension guard at the remote embedding adapter boundary (S6/S9/T7).
//!
//! The width comes from the CONTRACT — [`EmbeddingPort::embedding_dim`] as
//! implemented by [`RemoteEmbeddingAdapter`] — never from the first vector of
//! a response. Every vector in a successful response is checked against that
//! width, and a rejection is terminal: the cleaner's bisection arm must not
//! turn one bad response into more provider calls.

use crate::error::SemanticError;

const REJECTION_TAG: &str = "\n[ai-dimension rejected]";

/// Maximum tolerated rejected vectors in one batch. Zero: a single bad
/// vector aborts the whole batch, because the port must return one vector per
/// input and silently dropping one would corrupt the caller's 1:1 mapping.
pub(super) const MAX_TOLERATED_REJECTIONS: usize = 0;

/// Number of vectors whose width disagrees with the declared width.
#[must_use]
pub(super) fn rejected_count(vectors: &[Vec<f32>], declared: usize) -> usize {
    vectors
        .iter()
        .filter(|vector| vector.len() != declared)
        .count()
}

/// Build the terminal, tagged error for a dimension-invalid response.
#[must_use]
pub(crate) fn rejection_error(provider_id: &str, declared: usize) -> SemanticError {
    SemanticError::Inference(format!(
        "embedding width disagrees with the declared {declared} for provider '{provider_id}'{REJECTION_TAG}"
    ))
}

/// Whether an error is the terminal dimension rejection.
///
/// The cleaner calls this before its generic bisection arm so a bad width
/// cannot be retried with smaller batches.
#[must_use]
pub(crate) fn is_dimension_rejection(error: &SemanticError) -> bool {
    matches!(error, SemanticError::Inference(message) if message.contains(REJECTION_TAG))
}
