//! Crawl error types
//!
//! Following **err-thiserror-for-libraries**: Uses thiserror for library error types.
//! Following **api-non-exhaustive**: Can add variants without breaking changes.
//! Following **clean-architecture**: NO dependencies on reqwest/anyhow (Infra layer)
//!
//! # Architecture Note
//!
//! This error type does NOT contain `reqwest::Error` or `anyhow::Error`.
//! Those are infrastructure details. The Infrastructure layer converts
//! `reqwest::Error` → `CrawlError::Network` and `anyhow::Error` → specific variants.

use thiserror::Error;

mod classify;
mod conversions;

/// WAF detection classification for observability and retry decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WafDetectionKind {
    /// WAF detected via control/response headers
    ControlHeader,
    /// WAF detected via body signature patterns
    BodySignature,
    /// WAF detected via silent JavaScript challenge
    SilentChallenge,
    /// WAF detected via entropy anomaly in response
    EntropyAnomaly,
}

/// Resource type for resource exhaustion errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// Sitemap URL count limit
    SitemapUrls,
    /// Sitemap crawl depth limit
    SitemapDepth,
    /// RAM budget limit
    RamBudget,
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::SitemapUrls => "sitemap URLs",
            Self::SitemapDepth => "sitemap depth",
            Self::RamBudget => "RAM budget",
        };
        f.write_str(label)
    }
}

/// Crawl errors
///
/// Following **err-thiserror-for-libraries**: Uses thiserror for library error types.
/// Following **error-classification-matrix**: workspace-internal enum, NOT
/// `#[non_exhaustive]` — every variant must be classified in [`Self::classify`]
/// and in the matrix doc in the same change; the compiler enforces it.
/// Following **clean-architecture**: NO dependencies on reqwest/anyhow (Infra layer)
#[derive(Debug, Error)]
pub enum CrawlError {
    /// Network error during HTTP request
    ///
    /// Note: Does NOT contain reqwest::Error (that's Infra detail).
    /// Infrastructure layer converts reqwest::Error → this variant.
    #[error("network error: {message} (status: {status_code:?})")]
    Network {
        /// Human-readable error description.
        message: String,
        /// HTTP status code if the server responded; `None` for connection failures.
        status_code: Option<u16>,
        /// The original transport error (#2784): without it the cause dies.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },

    /// HTTP error with structured status code and URL
    #[error("HTTP error {status} at {url}")]
    Http {
        /// HTTP status code (e.g. 403, 429, 500)
        status: u16,
        /// URL that triggered the error
        url: String,
    },

    /// URL parsing error
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// HTML parsing error
    #[error("parse error: {0}")]
    Parse(String),

    /// Maximum depth exceeded
    #[error("maximum depth {max} exceeded at depth {current}")]
    MaxDepthExceeded {
        /// Depth at which the limit was exceeded.
        current: u8,
        /// Configured maximum crawl depth.
        max: u8,
    },

    /// Maximum pages exceeded
    #[error("maximum pages {max} exceeded")]
    MaxPagesExceeded {
        /// Configured maximum page count.
        max: usize,
    },

    /// URL excluded by pattern
    #[error("URL excluded: {0}")]
    UrlExcluded(String),

    /// Invalid content type
    #[error("invalid content type: {0}")]
    InvalidContentType(String),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Internal error (unspecified)
    #[error("internal error: {0}")]
    Internal(String),

    /// Session-derived invalid run description (P6-2 slice 2, matrix row
    /// 33). Surfaces a `CrawlSessionError` build failure: a run that
    /// cannot be described validly fails before any worker spawns, never
    /// falling back to a legacy path. The payload is the rendered session
    /// error ("invalid crawl session: …", "checkpoint unwritable …") so
    /// the cause is not double-prefixed. Maps to `ScraperError::Config`
    /// (exit 78) upstream.
    #[error("{0}")]
    InvalidSession(String),

    /// Sitemap not found during auto-discovery
    #[error("no sitemap found for {0}")]
    SitemapNotFound(String),

    /// Storage error (append-only log corruption, backpressure, serialization)
    #[error("error de almacenamiento: {0}")]
    Storage(String),

    /// Checkpoint serialization/deserialization error
    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    /// Session pool error (connection or lifecycle failure)
    #[error("session pool error: {0}")]
    SessionPool(String),

    /// Discovery error (robots.txt or sitemap auto-discovery failure)
    #[error("discovery error: {0}")]
    Discovery(String),

    /// Download error (fetch failed, SPA detected, or WAF blocked during download)
    ///
    /// Carries the underlying error as `#[source]` so the cause chain is
    /// preserved through `Error::source()` (D4).
    #[error("download error: {0}")]
    Download(#[source] Box<dyn std::error::Error + Send + Sync>),

    // === New variants (Error Map V2) ===
    /// WAF challenge detected during crawl
    #[error("WAF challenge: {provider} ({kind:?}) at {url}")]
    WafChallenge {
        /// Name of the WAF provider (e.g., "Cloudflare", "AWS WAF").
        provider: String,
        /// How the WAF challenge was detected.
        kind: WafDetectionKind,
        /// URL that triggered the WAF challenge.
        url: String,
    },

    /// Retry attempts exhausted for a URL
    #[error("retry exhausted for {url} after {attempts} attempts")]
    RetryExhausted {
        /// URL that failed after exhausting retries.
        url: String,
        /// Total number of attempts made.
        attempts: usize,
    },

    /// Transient HTTP error (5xx, retryable)
    #[error("transient HTTP {status} at {url}")]
    TransientHttp {
        /// HTTP status code (typically 5xx).
        status: u16,
        /// URL that returned the transient error.
        url: String,
    },

    /// Rate limited with retry-after duration in seconds
    #[error("rate limited, retry after {0}s")]
    RateLimited(u64),

    /// Request timeout — the `Display` text stays `"request timeout"`.
    ///
    /// The classified cause (`ConnectTimeout` / `ReadTimeout`) travels as the
    /// `source` when the producer still holds the transport error (#2841): the
    /// verdict and the `Display` text do not move, the cause is ADDED.
    #[error("request timeout")]
    Timeout {
        /// The classified transport failure, when one is available.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },

    /// Connection error
    ///
    /// The cause travels as the `source` (#2784).
    #[error("connection error: {message}")]
    Connection {
        /// Human-readable message (unchanged from the former tuple payload).
        message: String,
        /// The original transport error, when one is available.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },

    /// Request construction or body-read failure (non-transient)
    ///
    /// Produced when the HTTP request itself cannot be built or its body
    /// cannot be read — as opposed to timeout/connection failures, which are
    /// transient. Maps to `ScraperError::Internal` (InternalFatal).
    #[error("request failed: {0}")]
    RequestFailed(String),

    /// Transport failure that is neither a timeout nor a connection failure.
    ///
    /// The home of the failures the used-to-be `else` branch flattened into
    /// [`Self::RequestFailed`]: the transport error keeps its own text and the
    /// classified cause travels as `source` (#2841).
    #[error("transport error: {message}")]
    Transport {
        /// Human-readable message (the transport error's own text).
        message: String,
        /// The original transport error, when one is available.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },

    /// Resource limit exhausted
    #[error("resource exhausted: {resource:?} limit={limit} actual={actual}")]
    ResourceExhausted {
        /// Which resource limit was hit.
        resource: ResourceKind,
        /// Configured limit value.
        limit: usize,
        /// Actual usage that exceeded the limit.
        actual: usize,
    },

    /// No sitemap found (empty sitemap)
    #[error("no sitemap found")]
    SitemapEmpty,

    /// Sitemap crawl depth exceeded
    #[error("sitemap depth exceeded")]
    SitemapDepthExceeded,

    /// Semaphore exhausted (backpressure)
    #[error("semáforo agotado: no hay permisos disponibles")]
    SemaphoreInanition,

    /// Task cancelled by engine shutdown (#509)
    ///
    /// Control signal, not an operational failure: the crawl engine cancelled
    /// this task while it waited for a rate-limit permit or a resource-governor
    /// permit. Handlers must NOT count it as a crawl error.
    #[error("task cancelled by engine shutdown")]
    Cancelled,

    /// The run ended early (stall, unrecovered pause) with pages already
    /// collected (#2338)
    ///
    /// The run is still FAILED — this variant preserves the #2313 contract
    /// (error exit, checkpoint kept for resume) while carrying the results
    /// the engine had already gathered, so the caller can persist them
    /// instead of discarding completed work. `message` is the rendered
    /// original failure; `partial` is the same summary a successful run
    /// would have returned, truncated to what completed before the end.
    #[error("crawl ended early with partial results: {message}")]
    PartialRun {
        /// Rendered reason the run ended early.
        message: String,
        /// Results collected before the run ended.
        ///
        /// BOXED (#2824): the summary grew a field per taxonomy counter, and
        /// embedding it by value made the whole error — and every `Result`
        /// that carried it — too big to move cheaply (clippy's
        /// `result_large_err`).
        partial: Box<crate::domain::CrawlResult>,
    },
}

impl CrawlError {
    /// A transport `Network` failure with no classified cause (#2784).
    #[must_use]
    pub fn network(message: impl Into<String>, status_code: Option<u16>) -> Self {
        Self::Network {
            message: message.into(),
            status_code,
            source: None,
        }
    }

    /// A timeout whose classified cause is not available.
    #[must_use]
    pub const fn timeout() -> Self {
        Self::Timeout { source: None }
    }

    /// A timeout that keeps the classified cause (#2841).
    #[must_use]
    pub fn timeout_with_cause(source: crate::domain::error::cause::ErrorSource) -> Self {
        Self::Timeout {
            source: Some(source),
        }
    }

    /// An unclassified transport failure (#2841).
    #[must_use]
    pub fn transport(
        message: impl Into<String>,
        source: Option<crate::domain::error::cause::ErrorSource>,
    ) -> Self {
        Self::Transport {
            message: message.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CrawlResult;

    #[test]
    fn test_partial_run_classifies_internal_fatal_and_displays_reason() {
        let error = CrawlError::PartialRun {
            message: "crawl stalled: test".to_string(),
            partial: Box::new(CrawlResult::empty()),
        };
        assert!(matches!(
            error.classify(),
            crate::domain::error::ErrorClass::InternalFatal
        ));
        let rendered = error.to_string();
        assert!(rendered.contains("partial results"));
        assert!(rendered.contains("crawl stalled: test"));
    }

    #[test]
    fn test_crawl_error_network_no_reqwest() {
        let error = CrawlError::network("timeout", Some(408));
        assert!(error.to_string().contains("timeout"));
        assert!(error.to_string().contains("408"));
    }

    #[test]
    fn test_crawl_error_network_no_status() {
        let error = CrawlError::network("connection refused", None);
        assert!(error.to_string().contains("connection refused"));
        assert!(error.to_string().contains("None"));
    }

    #[test]
    fn test_crawl_error_io() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let error = CrawlError::from(io_error);
        assert!(matches!(error, CrawlError::Io(_)));
        assert!(error.to_string().contains("file not found"));
    }

    #[test]
    fn test_crawl_error_semaphore_inanition() {
        let error = CrawlError::SemaphoreInanition;
        assert!(error.to_string().contains("semáforo agotado"));
    }

    #[test]
    fn test_crawl_error_cancelled_display() {
        let error = CrawlError::Cancelled;
        assert_eq!(error.to_string(), "task cancelled by engine shutdown");
    }

    #[test]
    fn test_crawl_error_internal() {
        let error = CrawlError::Internal("something went wrong".to_string());
        assert!(error.to_string().contains("something went wrong"));
    }

    #[test]
    fn test_crawl_error_storage_display() {
        let error = CrawlError::Storage("archivo corrupto".to_string());
        assert!(
            error.to_string().contains("error de almacenamiento"),
            "expected Storage display to contain 'error de almacenamiento', got: {error}"
        );
        assert!(error.to_string().contains("archivo corrupto"));
    }

    #[test]
    fn test_crawl_error_storage_empty_message() {
        let error = CrawlError::Storage(String::new());
        assert!(error.to_string().contains("error de almacenamiento"));
    }

    #[test]
    fn test_crawl_error_display_all_variants() {
        let error = CrawlError::InvalidUrl("bad-url".to_string());
        assert!(error.to_string().contains("bad-url"));

        let error = CrawlError::Parse("html parse failed".to_string());
        assert!(error.to_string().contains("html parse failed"));

        let error = CrawlError::RateLimited(60);
        assert!(error.to_string().contains("60"));

        let error = CrawlError::MaxDepthExceeded { current: 5, max: 3 };
        assert_eq!(error.to_string(), "maximum depth 3 exceeded at depth 5");

        let error = CrawlError::MaxPagesExceeded { max: 100 };
        assert_eq!(error.to_string(), "maximum pages 100 exceeded");

        let error = CrawlError::UrlExcluded("https://evil.com".to_string());
        assert!(error.to_string().contains("evil.com"));

        let error = CrawlError::InvalidContentType("image/png".to_string());
        assert!(error.to_string().contains("image/png"));
    }

    #[test]
    fn test_crawl_error_checkpoint() {
        let error = CrawlError::Checkpoint("json decode failed".to_string());
        assert!(error.to_string().contains("checkpoint error"));
        assert!(error.to_string().contains("json decode failed"));
    }

    #[test]
    fn test_crawl_error_session_pool() {
        let error = CrawlError::SessionPool("pool exhausted".to_string());
        assert!(error.to_string().contains("session pool error"));
        assert!(error.to_string().contains("pool exhausted"));
    }

    #[test]
    fn test_crawl_error_discovery() {
        let error = CrawlError::Discovery("robots.txt unreachable".to_string());
        assert!(error.to_string().contains("discovery error"));
        assert!(error.to_string().contains("robots.txt unreachable"));
    }

    #[test]
    fn test_crawl_error_download() {
        let error = CrawlError::Download(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        )));
        assert!(error.to_string().contains("download error"));
        assert!(error.to_string().contains("connection reset"));
    }

    #[test]
    fn test_crawl_error_waf_challenge() {
        let error = CrawlError::WafChallenge {
            provider: "Cloudflare".to_string(),
            kind: WafDetectionKind::BodySignature,
            url: "https://example.com".to_string(),
        };
        assert!(error.to_string().contains("Cloudflare"));
        assert!(error.to_string().contains("example.com"));
    }

    #[test]
    fn test_crawl_error_retry_exhausted() {
        let error = CrawlError::RetryExhausted {
            url: "https://example.com".to_string(),
            attempts: 3,
        };
        assert!(error.to_string().contains("retry exhausted"));
        assert!(error.to_string().contains("3"));
    }

    #[test]
    fn test_crawl_error_transient_http() {
        let error = CrawlError::TransientHttp {
            status: 503,
            url: "https://example.com".to_string(),
        };
        assert!(error.to_string().contains("503"));
    }

    #[test]
    fn test_crawl_error_rate_limited() {
        let error = CrawlError::RateLimited(30);
        assert!(error.to_string().contains("30"));
    }

    #[test]
    fn test_crawl_error_timeout() {
        let error = CrawlError::timeout();
        assert!(error.to_string().contains("timeout"));
    }

    #[test]
    fn test_crawl_error_connection() {
        let error = CrawlError::Connection {
            message: "refused".to_string(),
            source: None,
        };
        assert!(error.to_string().contains("connection error"));
        assert!(error.to_string().contains("refused"));
    }

    #[test]
    fn test_crawl_error_resource_exhausted() {
        let error = CrawlError::ResourceExhausted {
            resource: ResourceKind::RamBudget,
            limit: 1024,
            actual: 2048,
        };
        assert!(error.to_string().contains("RamBudget"));
        assert!(error.to_string().contains("1024"));
        assert!(error.to_string().contains("2048"));
    }

    #[test]
    fn test_resource_kind_display() {
        // EC-RESOURCE-DISPLAY: human-friendly Display per variant.
        assert_eq!(ResourceKind::SitemapUrls.to_string(), "sitemap URLs");
        assert_eq!(ResourceKind::SitemapDepth.to_string(), "sitemap depth");
        assert_eq!(ResourceKind::RamBudget.to_string(), "RAM budget");

        // ResourceExhausted must keep rendering the resource via Debug
        // (`{resource:?}`), so the machine-readable variant name stays in the
        // error string even though Display now exists (type-display-vs-debug).
        let error = CrawlError::ResourceExhausted {
            resource: ResourceKind::RamBudget,
            limit: 1024,
            actual: 2048,
        };
        assert!(
            error.to_string().contains("RamBudget"),
            "ResourceExhausted must keep Debug rendering of the resource: {error}"
        );
    }

    #[test]
    fn test_crawl_error_sitemap_empty() {
        let error = CrawlError::SitemapEmpty;
        assert!(error.to_string().contains("no sitemap found"));
    }

    #[test]
    fn test_crawl_error_sitemap_depth_exceeded() {
        let error = CrawlError::SitemapDepthExceeded;
        assert!(error.to_string().contains("sitemap depth exceeded"));
    }

    #[test]
    fn test_crawl_error_sitemap_not_found() {
        let error = CrawlError::SitemapNotFound("https://example.com".to_string());
        assert!(error.to_string().contains("no sitemap found"));
        assert!(error.to_string().contains("example.com"));
    }

    #[test]
    fn test_http_error_to_crawl_error_conversion() {
        let http_err = crate::domain::http_error::HttpError::Forbidden;
        let crawl_err: CrawlError = http_err.into();
        assert!(matches!(crawl_err, CrawlError::Http { status: 403, .. }));
    }

    #[test]
    fn test_http_error_rate_limited_to_crawl_error() {
        let http_err = crate::domain::http_error::HttpError::RateLimited(60);
        let crawl_err: CrawlError = http_err.into();
        assert!(matches!(crawl_err, CrawlError::RateLimited(60)));
    }

    #[test]
    fn test_http_error_timeout_to_crawl_error() {
        let http_err = crate::domain::http_error::HttpError::timeout();
        let crawl_err: CrawlError = http_err.into();
        assert!(matches!(crawl_err, CrawlError::Timeout { .. }));
    }

    #[test]
    fn test_http_error_waf_to_crawl_error() {
        let http_err = crate::domain::http_error::HttpError::WafChallenge("CF".to_string());
        let crawl_err: CrawlError = http_err.into();
        assert!(matches!(
            crawl_err,
            CrawlError::WafChallenge {
                provider,
                kind: WafDetectionKind::BodySignature,
                ..
            } if provider == "CF"
        ));
    }
}
