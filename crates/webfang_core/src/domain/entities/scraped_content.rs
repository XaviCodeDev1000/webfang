//! The page-level content entity — the scraper's output type.
//!
//! Extracted from `entities/content.rs` (Ola 3 B4, issue #2966): one entity
//! per module, the convention the rest of `domain/entities/` already follows.
//! The RAG-pipeline chunk entity stays in [`super::content`]; the conversion
//! from `ScrapedContent` lives on the consuming side there.

use serde::{Deserialize, Serialize};

use crate::domain::{CorrelationId, ValidUrl};

use super::download::DownloadedAsset;

/// Represents scraped content from a web page
///
/// This is the main output type of the scraper, containing:
/// - Extracted content (title, text)
/// - Metadata (author, date, excerpt)
/// - Downloaded assets (images, documents)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrapedContent {
    /// Title of the page/article
    pub title: String,
    /// Main content extracted (clean, without ads/nav)
    pub content: String,
    /// Original URL (validated)
    pub url: ValidUrl,
    /// Excerpt/summary if available
    pub excerpt: Option<String>,
    /// Author if available
    pub author: Option<String>,
    /// Publication date if available (ISO 8601 format)
    pub date: Option<String>,
    /// The HTML source (optional, for debugging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Downloaded assets (images, documents)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub assets: Vec<DownloadedAsset>,
    /// Distributed tracing correlation ID (internal — not serialized).
    ///
    /// Populated natively on every scrape (issue #356) and propagated
    /// in-memory to `DocumentChunk` for the JSONL export correlation.
    /// Skipped from `ScrapedContent` serialization to keep the user-facing
    /// `--format json` output clean and deterministic.
    #[serde(skip)]
    pub correlation_id: Option<CorrelationId>,
    /// Extraction-quality hint (internal — not serialized).
    ///
    /// Populated when the adaptive repair yields a low structural score (#792).
    /// Skipped from serialization to keep `--format json` deterministic.
    #[serde(skip)]
    pub quality_hint: Option<crate::domain::extraction_quality::ExtractionQualityHint>,
}

impl std::fmt::Display for ScrapedContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let title = if self.title.is_empty() {
            "(untitled)"
        } else {
            self.title.as_str()
        };
        write!(f, "{title} — {}", self.url)
    }
}
