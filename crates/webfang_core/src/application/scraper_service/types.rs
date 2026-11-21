//! Scrape outcome types — serializable results for MCP consumers.
//!
//! Pure data: no I/O, no async. Split out of the scraper-service hub so the
//! use-case logic and these DTOs can evolve independently (#2584 policy v3.3).

use crate::domain::{ExtractResult, ScrapedContent};

/// Maximum HTML body size to log/instrument (1MB)
/// Bodies larger than this are skipped to avoid performance issues
pub const MAX_INSTRUMENTED_BODY_SIZE: usize = 1_048_576;

/// Outcome of a scrape operation, including selector extraction metadata.
///
/// Contains both the scraped content results and the [`ExtractResult`] from
/// CSS selector extraction, allowing callers (e.g. the MCP handler) to
/// inspect whether the selector matched and access diagnostics.
#[derive(Debug)]
pub struct ScrapeOutcome {
    /// Scraped content results.
    pub results: Vec<ScrapedContent>,
    /// CSS selector extraction result (`Matched` or `Fallback` with optional diagnostic).
    pub extract_result: ExtractResult,
}

impl ScrapeOutcome {
    /// Get the scraped content results as a slice.
    #[must_use]
    pub fn as_results(&self) -> &[ScrapedContent] {
        &self.results
    }
}

/// A failed URL in a batch scrape operation.
///
/// Carries the URL and a serializable error representation so the MCP layer
/// can expose per-URL failures to the calling AI agent (issue #591).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScrapeFailed {
    /// The URL that failed to scrape.
    pub url: String,
    /// User-facing error message (Spanish, matching `ScraperError` Display).
    pub error: String,
    /// Operational classification of the error for retry/abort decisions.
    pub category: ScrapeErrorCategory,
}

/// Serializable classification of a [`ScraperError`](crate::error::ScraperError) for MCP consumers.
///
/// Mirrors [`crate::error::ErrorClass`] but owned (no lifetime) and
/// serializable, so the MCP layer can embed it in JSON responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrapeErrorCategory {
    /// Transient — safe to retry immediately (5xx, connection reset).
    TransientRetriable,
    /// Transient — wait before retry (429, rate limit, timeout).
    TransientBackoff,
    /// Permanent — retry will not help (404, invalid URL, WAF).
    PermanentFatal,
    /// Internal — indicates a bug, not a runtime condition.
    InternalFatal,
    /// Domain recoverable — single item failed but pipeline is healthy.
    DomainRecoverable,
}

impl From<crate::error::ErrorClass> for ScrapeErrorCategory {
    fn from(class: crate::error::ErrorClass) -> Self {
        match class {
            crate::error::ErrorClass::TransientRetriable => Self::TransientRetriable,
            crate::error::ErrorClass::TransientBackoff => Self::TransientBackoff,
            crate::error::ErrorClass::PermanentFatal => Self::PermanentFatal,
            crate::error::ErrorClass::InternalFatal => Self::InternalFatal,
            crate::error::ErrorClass::DomainRecoverable => Self::DomainRecoverable,
        }
    }
}

/// Outcome of [`scrape_multiple_with_limit`](crate::application::scraper_service::scrape_multiple_with_limit):
/// successes + per-URL failures.
///
/// Replaces the bare `Vec<ScrapedContent>` return so callers can report
/// exactly which URLs failed and why (issue #591 — `scrape_batch` observability).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScrapeBatchOutcome {
    /// Successfully scraped content.
    pub results: Vec<ScrapedContent>,
    /// URLs that failed, with error details preserved.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub failed: Vec<ScrapeFailed>,
}
