//! JSONL record schema and wire-format types for the vector sink.
//!
//! These types describe one emitted line and the CLI sink-path input. They are
//! re-exported from the parent module, so every
//! `infrastructure::stream::{VectorRecord, EmbeddingProvenance, SinkPath}` path
//! keeps resolving exactly as before the extraction.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{Sha256Hex, ValidUrl};
use crate::error::ScraperError;

/// A single JSONL vector record emitted by [`StreamRepository`](super::StreamRepository).
///
/// Mirrors the fields surfaced by the elastic ingestion pipeline: the source
/// URL, the content `sha256_hex` (content-hash dedup key), an optional title,
/// the cleaned `chunk_text`, the raw `embedding` vector, arbitrary `metadata`,
/// and an RFC3339 `timestamp`.
///
/// Both key fields are validated newtypes (#1118): `url` carries the
/// `ValidUrl` hardening (http(s) only, credentials stripped) and
/// `sha256_hex` is a real 64-char lowercase digest — the malformed dedup
/// keys that used to pass as raw `String`s are rejected at the sink
/// boundary and at (de)serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorRecord {
    /// Source URL the chunk was extracted from.
    pub url: ValidUrl,
    /// Content hash (SHA-256 hex) of the resource — the dedup key.
    pub sha256_hex: Sha256Hex,
    /// Best-effort title (first ≤200 chars of the first chunk line), or `null`.
    pub title: Option<String>,
    /// Cleaned chunk text.
    pub chunk_text: String,
    /// Raw embedding vector (e.g. 384 floats for all-MiniLM-L6-v2). Empty only
    /// when the record would have been omitted (Q3).
    pub embedding: Vec<f32>,
    /// Stable identifier of the model that produced `embedding` (#2429), or
    /// `null` when the run had no model identity available. Vectors from
    /// different models are never comparable: consumers must group by this
    /// field before running similarity search.
    #[serde(default)]
    pub embedding_model: Option<String>,
    /// Dimensionality declared for `embedding_model` (#2429), or `null`.
    /// A record whose `embedding.len()` disagrees with this is rejected at
    /// both the read ([`VectorRecord::from_line`]) and the write boundary.
    #[serde(default)]
    pub embedding_dim: Option<usize>,
    /// Zero-based position of the chunk within its source document (#2429),
    /// as handed to [`VectorRepository::save_chunk`](crate::domain::repository::VectorRepository::save_chunk) by the ingestion
    /// pipeline. Lets a consumer reconstruct document order from an unordered
    /// result set.
    #[serde(default)]
    pub chunk_index: Option<i64>,
    /// First byte of the chunk's region inside the source document (#2429
    /// AC2), half-open and paired with `byte_end`: the enclosing range of the
    /// chunk's spans in the text the chunker segmented. `null` when the run
    /// carried no locator information. Offsets are bytes (sliceable, UTF-8
    /// safe), not chars — the same unit the `ByteSpan` contract uses.
    #[serde(default)]
    pub byte_start: Option<u64>,
    /// Byte after the last byte of the chunk's region (#2429 AC2); paired
    /// with `byte_start`. `from_line` rejects a range with `byte_start >
    /// byte_end`, or exactly one of the two present.
    #[serde(default)]
    pub byte_end: Option<u64>,
    /// Optional arbitrary metadata (reserved for future use).
    #[serde(default)]
    pub metadata: Value,
    /// RFC3339 timestamp of emission.
    pub timestamp: String,
}

/// Model identity attached to every record a [`StreamRepository`](super::StreamRepository) emits (#2429).
///
/// Captured once, at sink construction, from the container's
/// [`EmbeddingPort`](crate::domain::embedding_port::EmbeddingPort): one run
/// embeds with exactly one model, so the identity is a property of the sink,
/// not of each chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProvenance {
    /// Stable model identifier (e.g. `"granite-97m"`, or the
    /// provider-declared model name for remote endpoints).
    pub model_id: String,
    /// Dimensionality the model emits. The sink rejects any vector whose
    /// length disagrees with this.
    pub embedding_dim: usize,
}

impl VectorRecord {
    /// Parse and validate one JSONL line — the read boundary of the format.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Serialization`] for malformed JSON, and
    /// [`ScraperError::Ingestion`] when a present `embedding_dim` disagrees
    /// with `embedding.len()` (#2429), or when the byte range is inverted or
    /// half-present (#2429 AC2): such a record cannot name a real region of
    /// any document and must not enter a store.
    pub fn from_line(line: &str) -> Result<Self, ScraperError> {
        let record: Self = serde_json::from_str(line)?;
        if let Some(dim) = record.embedding_dim {
            if record.embedding.len() != dim {
                return Err(ScraperError::Ingestion(format!(
                    "el record declara embedding_dim {dim} pero su vector tiene {} dimensiones",
                    record.embedding.len()
                )));
            }
        }
        match (record.byte_start, record.byte_end) {
            (Some(start), Some(end)) if start > end => {
                return Err(ScraperError::Ingestion(format!(
                    "el record declara un rango de bytes invertido: byte_start {start} > byte_end {end}"
                )));
            },
            (Some(_), None) | (None, Some(_)) => {
                return Err(ScraperError::Ingestion(
                    "el record debe declarar byte_start y byte_end juntos, o ninguno de los dos"
                        .to_string(),
                ));
            },
            _ => {},
        }
        Ok(record)
    }
}

/// Where a [`StreamRepository`](super::StreamRepository) writes (#1118).
///
/// The `"-"` stdout sentinel used to be a magic string compared inside the
/// constructor; it is now a variant, and an empty path is rejected at the
/// boundary instead of reaching `File::create("")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkPath {
    /// Buffered stdout — the wire sentinel is `"-"`.
    Stdout,
    /// A file path, created (truncated) by the sink.
    File(std::path::PathBuf),
}

impl SinkPath {
    /// Parse the CLI wire form (`--output-vectors <path|->`).
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`] for an empty or whitespace-only
    /// path — there is no valid sink there.
    pub fn parse(raw: &str) -> Result<Self, ScraperError> {
        if raw == "-" {
            return Ok(Self::Stdout);
        }
        if raw.trim().is_empty() {
            return Err(ScraperError::Config(
                "la ruta de salida de vectores no puede estar vacía".to_string(),
            ));
        }
        Ok(Self::File(std::path::PathBuf::from(raw)))
    }
}

impl std::str::FromStr for SinkPath {
    type Err = ScraperError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}
