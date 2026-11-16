//! Crawl error categorization for observability (issue #374).
//!
//! Maps the 20+ `CrawlError` variants into a fixed set of operational
//! categories suitable for metrics, structured logging, and dashboards.

use serde::Serialize;

use crate::domain::CrawlError;

/// Operational category for a crawl error.
///
/// Reduces the full `CrawlError` variant space to a fixed set of
/// categories that are meaningful in dashboards and alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CrawlErrorCategory {
    /// WAF challenge detected (Cloudflare, AWS WAF, etc.)
    Waf,
    /// HTTP status errors (4xx/5xx, transient)
    Http,
    /// Request timeout
    Timeout,
    /// Network-level failures (DNS, connection, download)
    Network,
    /// Rate limiting (429 / retry-after)
    RateLimit,
    /// Content extraction / parsing failures
    Extraction,
    /// Internal / catch-all (storage, checkpoint, config, etc.)
    Internal,
    /// Task panicked (JoinError)
    Panic,
}

impl CrawlErrorCategory {
    /// All categories in stable order (for array indexing and iteration).
    pub const ALL: [Self; 8] = [
        Self::Waf,
        Self::Http,
        Self::Timeout,
        Self::Network,
        Self::RateLimit,
        Self::Extraction,
        Self::Internal,
        Self::Panic,
    ];

    /// Index for array-based counting.
    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Waf => 0,
            Self::Http => 1,
            Self::Timeout => 2,
            Self::Network => 3,
            Self::RateLimit => 4,
            Self::Extraction => 5,
            Self::Internal => 6,
            Self::Panic => 7,
        }
    }

    /// Tracing field name for this category.
    #[inline]
    #[must_use]
    pub const fn tracing_field(self) -> &'static str {
        match self {
            Self::Waf => "errors_waf",
            Self::Http => "errors_http",
            Self::Timeout => "errors_timeout",
            Self::Network => "errors_network",
            Self::RateLimit => "errors_rate_limit",
            Self::Extraction => "errors_extraction",
            Self::Internal => "errors_internal",
            Self::Panic => "errors_panic",
        }
    }
}

impl std::fmt::Display for CrawlErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Waf => "waf",
            Self::Http => "http",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::RateLimit => "rate_limit",
            Self::Extraction => "extraction",
            Self::Internal => "internal",
            Self::Panic => "panic",
        };
        f.write_str(label)
    }
}

/// HTTP status codes that signal a WAF-style challenge rather than a generic
/// HTTP error. These are the access-denied / rate-limit family of responses
/// (401 Unauthorized, 403 Forbidden, 429 Too Many Requests) that operators
/// should attribute to WAF blocking in `errors_waf` (issue #603).
#[inline]
#[must_use]
fn is_waf_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 429)
}

impl From<&CrawlError> for CrawlErrorCategory {
    fn from(err: &CrawlError) -> Self {
        match err {
            CrawlError::WafChallenge { .. } => Self::Waf,
            CrawlError::Http { status, .. } | CrawlError::TransientHttp { status, .. } => {
                if is_waf_status(*status) {
                    Self::Waf
                } else {
                    Self::Http
                }
            },
            // The `source` is deliberately NOT read here (#2841): the
            // category of a timeout stays `Timeout` whatever the cause, so a
            // `ConnectTimeout` cannot silently become a `Network` row.
            CrawlError::Timeout { .. } => Self::Timeout,
            // The `source` is deliberately NOT matched: a transport failure
            // now carries the classified cause there (#2784), so requiring
            // `source: None` would silently drop the WAF/RateLimit attribution
            // for every real 401/403/429. The status is what decides.
            CrawlError::Network {
                status_code: Some(status),
                ..
            } if matches!(*status, 401 | 403) => Self::Waf,
            CrawlError::Network {
                status_code: Some(429),
                ..
            } => Self::RateLimit,
            // The cause decides the bucket: the vocabulary and this enum are
            // ONE taxonomy, and `NetworkCause::category` is the only place
            // that maps between them (#2784).
            CrawlError::Network { .. }
            | CrawlError::Connection { .. }
            | CrawlError::Transport { .. }
            | CrawlError::Download(..) => crate::domain::error::cause::cause_in_chain(err).map_or(
                Self::Network,
                crate::domain::error::cause::NetworkCause::category,
            ),
            CrawlError::RateLimited(..) => Self::RateLimit,
            CrawlError::Parse(..) => Self::Extraction,
            // Every arm below was previously the `_ => Self::Internal`
            // catch-all. Each is now mapped explicitly per the Error
            // Classification Matrix (docs/error-classification-matrix.md);
            // the comment cites the matrix row driving the choice.
            // Row 15 (validation): no dedicated validation category exists,
            // so the Internal catch-all remains the closest match.
            CrawlError::InvalidUrl(_) => Self::Internal,
            // Row 33 (P6-2 slice 2): invalid run description — a
            // validation failure, same Internal catch-all as row 15.
            CrawlError::InvalidSession(_) => Self::Internal,
            // Rows 9/10/11/12 (limits/budget family): budget stops are not
            // operational failures and there is no budget category; the
            // Internal catch-all is the closest existing category.
            CrawlError::MaxDepthExceeded { .. }
            | CrawlError::MaxPagesExceeded { .. }
            | CrawlError::UrlExcluded(_)
            | CrawlError::ResourceExhausted { .. }
            | CrawlError::SitemapDepthExceeded => Self::Internal,
            // Row 18 (domain/content family): a rejected content type is a
            // content-processing failure, so it lands in Extraction next to
            // `Parse`.
            CrawlError::InvalidContentType(_) => Self::Extraction,
            // Rows 21/22 (io by kind): infrastructure failures → Internal.
            CrawlError::Io(_) => Self::Internal,
            CrawlError::Internal(_) => Self::Internal,
            // Row 20 (sitemap discovery family): no dedicated discovery
            // category exists; Internal remains the catch-all.
            CrawlError::SitemapNotFound(_) | CrawlError::Discovery(_) => Self::Internal,
            CrawlError::SitemapEmpty => Self::Internal,
            // Row 23 (persistence/internal infrastructure).
            CrawlError::Storage(_) | CrawlError::Checkpoint(_) => Self::Internal,
            // Row 24 (internal infrastructure: session pool lifecycle).
            CrawlError::SessionPool(_) => Self::Internal,
            // Taxonomy: per-URL retry-budget terminal state; job continues.
            // No dedicated retry category exists → Internal catch-all.
            CrawlError::RetryExhausted { .. } => Self::Internal,
            // Family 1 (HTTP request construction/body-read failure):
            // closest existing category is Http.
            CrawlError::RequestFailed(_) => Self::Http,
            // Row 13 (backpressure configuration bug) → Internal.
            CrawlError::SemaphoreInanition => Self::Internal,
            // Special cell — Cancelled: control signal, not an operational
            // failure category; Internal catch-all if it ever surfaces.
            CrawlError::Cancelled => Self::Internal,
            // #2338: a run that ended early is an engine-level failure; the
            // Internal catch-all is the closest existing category (same cell
            // the stall error occupied as `Internal` before the variant).
            CrawlError::PartialRun { .. } => Self::Internal,
        }
    }
}

#[cfg(test)]
mod tests;
