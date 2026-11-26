use crate::error::ScraperError;

// ============================================================================
// Embedding BLOB serialization (frozen design decision #7)
// ============================================================================
/// Serialize an `f32` slice to little-endian bytes (4 bytes per `f32`).
///
/// Frozen design decision #7: `embedding_vector` BLOB = raw little-endian `f32`
/// bytes. Uses explicit `f32::to_le_bytes` (no `unsafe`, no `bytemuck`
/// dependency) for portability across architectures.
pub(super) fn f32_slice_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for &f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Deserialize little-endian bytes back to `Vec<f32>`.
///
/// Validates the BLOB length is a multiple of 4; returns a
/// Decode a stored embedding BLOB back into `f32`s.
///
/// The length check is the LAST line of defence, not the schema's version
/// detection. A BLOB whose length is not a multiple of four proves corruption
/// and nothing more; before the [`crate::infrastructure::persistence::sqlite::SCHEMA_VERSION`] marker (#1617, M4/D4) this
/// check was the ONLY way a database's schema state could be judged, so a file
/// written by a build with a different column layout was indistinguishable
/// from a damaged one. A mismatched database is now refused at open, long
/// before a single BLOB is read.
///
/// # Errors
///
/// [`ScraperError::Persistence`] with a Spanish message on corruption (frozen
/// decision #4: no separate `StorageError` enum — maps to the existing
/// Display-based `Persistence(String)` variant).
pub(super) fn bytes_to_f32_vec(b: &[u8]) -> Result<Vec<f32>, ScraperError> {
    if !b.len().is_multiple_of(4) {
        return Err(ScraperError::persistence(format!(
            "vector BLOB corrupto: longitud {} no es múltiplo de 4",
            b.len()
        )));
    }
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| Ok(f32::from_le_bytes(*chunk)))
        .collect()
}
