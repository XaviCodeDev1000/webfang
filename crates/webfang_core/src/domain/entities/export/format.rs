//! Format and schema-version types for the export pipeline.
//!
//! Extracted from `export.rs` (Ola 0, #2597); behavior unchanged.

use serde::{Deserialize, Serialize};

/// Export format variants for RAG pipeline
///
/// Defines the supported output formats when exporting scraped content
/// for use in retrieval-augmented generation systems.
///
/// These formats are designed for RAG/embedding pipelines, NOT for
/// individual file output (see OutputFormat for that).
///
/// | Format | Extension | Use Case |
/// |--------|-----------|----------|
/// | Jsonl | .jsonl | One JSON object per line, optimal for RAG |
/// | Auto | .auto | Auto-detect from existing files |
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum, Default,
)]
pub enum ExportFormat {
    /// JSONL format (JSON Lines - one JSON object per line)
    /// Optimal for RAG pipelines and vector database ingestion
    #[default]
    Jsonl,
    /// Vector format (JSON with metadata header)
    /// Supports embeddings and cosine similarity
    Vector,
    /// Auto-detect format from existing export files
    Auto,
}

impl ExportFormat {
    /// Parse from string (case-insensitive).
    /// Note: Named `parse_str` to avoid confusion with `FromStr::from_str`.
    pub fn parse_str(s: &str) -> Result<Self, &'static str> {
        match s.to_lowercase().as_str() {
            "jsonl" => Ok(ExportFormat::Jsonl),
            "vector" => Ok(ExportFormat::Vector),
            "auto" => Ok(ExportFormat::Auto),
            _ => Err("Invalid export format. Use 'jsonl', 'vector', or 'auto'"),
        }
    }
    /// Returns the file extension for this format
    #[must_use]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Vector => "json",
            Self::Auto => "auto",
        }
    }

    /// Returns a human-readable name for this format
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Jsonl => "JSONL",
            Self::Vector => "Vector",
            Self::Auto => "Auto",
        }
    }
}

/// Schema version of the persisted export state as a type-state (#1162).
///
/// A raw `u32` accepted any value at the serde boundary; now only versions the
/// reader understands cross it, via [`TryFrom`]: [`StateVersion::LEGACY`] (`0`,
/// readable so stale files still reach the `load_or_default` discard path) and
/// [`StateVersion::CURRENT`]. Anything newer fails fast with a Spanish error
/// instead of loading as maybe-compatible state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct StateVersion(u32);

impl StateVersion {
    /// Current schema version written by
    /// [`ExportState::new`](crate::domain::entities::export::ExportState::new).
    pub const CURRENT: Self = Self(1);
    /// Legacy version: readable but never written.
    pub const LEGACY: Self = Self(0);

    /// The raw version number (for comparisons, logs, and compat shims).
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for StateVersion {
    type Error = crate::ScraperError;

    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        if raw == Self::CURRENT.get() || raw == Self::LEGACY.get() {
            Ok(Self(raw))
        } else {
            Err(crate::ScraperError::Config(format!(
                "versión de estado de exportación no soportada: {raw} (versión actual: {})",
                Self::CURRENT.get()
            )))
        }
    }
}

impl From<StateVersion> for u32 {
    fn from(version: StateVersion) -> Self {
        version.get()
    }
}
