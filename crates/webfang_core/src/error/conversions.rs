//! Conversions into the application error.
//!
//! Holds the `From` impls that map layer-specific errors — and the `wreq`
//! transport error — onto [`ScraperError`], plus the network-message
//! flattener used by the `CrawlError` conversion. A trait impl is legal
//! anywhere in the crate, so moving them here changes no call site (`?`,
//! `.into()`): only where the code sits.

use super::ScraperError;
use wreq::Error as WreqError;

/// Map a `CrawlError::Network` message to a synthetic `io::Error` with the
/// kind that best matches the transport failure (#537).
///
/// `CrawlError::Network` has already crossed the infrastructure → domain
/// boundary and lost its typed source; all that survives is a `String`
/// message. Re-deriving the io kind from that message text lets
/// `is_transient_network` (which downcasts on `io::Error` FIRST and never
/// reaches the text fallback for an io-typed boxed error) classify it
/// correctly. Unknown transport text conservatively maps to
/// `ErrorKind::Other`, which is NOT in the transient list and so falls back
/// to `InternalFatal` — preserving the safety net for genuinely unknown
/// internal failures.
fn io_error_for_network_message(message: &str) -> std::io::Error {
    let msg = message.to_ascii_lowercase();
    let kind = if msg.contains("connection refused")
        || msg.contains("client error (connect)")
        || msg.contains("connect error")
    {
        std::io::ErrorKind::ConnectionRefused
    } else if msg.contains("host unreachable") {
        std::io::ErrorKind::HostUnreachable
    } else if msg.contains("network unreachable") {
        std::io::ErrorKind::NetworkUnreachable
    } else if msg.contains("dns error")
        || msg.contains("dns lookup failed")
        || msg.contains("could not resolve")
        || msg.contains("failed to lookup address")
        || msg.contains("name or service not known")
    {
        // DNS failure: the service is unreachable — still a transport outage,
        // not a bug. AddrNotAvailable is the closest stable io kind in the
        // transient list.
        std::io::ErrorKind::AddrNotAvailable
    } else if msg.contains("timeout") || msg.contains("timed out") {
        std::io::ErrorKind::TimedOut
    } else if msg.contains("connection reset") {
        std::io::ErrorKind::ConnectionReset
    } else if msg.contains("connection aborted") {
        std::io::ErrorKind::ConnectionAborted
    } else if msg.contains("broken pipe") {
        std::io::ErrorKind::BrokenPipe
    } else {
        std::io::ErrorKind::Other
    };
    std::io::Error::new(kind, message.to_string())
}

impl From<WreqError> for ScraperError {
    /// Convert a `wreq::Error` into the single network variant, preserving the
    /// underlying cause chain (D4). Consolidates the former duplicate
    /// `NetworkFailure` variant (#153).
    fn from(e: WreqError) -> Self {
        ScraperError::Network(Box::new(e))
    }
}

// ============================================================================
// Error Map V2: From impls for layer-specific errors
// ============================================================================

impl From<crate::domain::error::CrawlError> for ScraperError {
    fn from(e: crate::domain::error::CrawlError) -> Self {
        use crate::domain::error::CrawlError;
        match e {
            CrawlError::Network {
                message,
                status_code,
            } => {
                if let Some(status) = status_code {
                    ScraperError::Http {
                        status,
                        url: message,
                    }
                } else {
                    // #537: do NOT flatten transport failures to Internal —
                    // that bypasses `is_transient_network` and masquerades a
                    // service outage (connect refused, DNS, timeout) as a bug.
                    // Wrap the message in a synthetic `io::Error` whose kind
                    // is sniffed from the transport text so `classify()` can
                    // route to TransientRetriable when appropriate.
                    ScraperError::Network(Box::new(io_error_for_network_message(&message)))
                }
            },
            CrawlError::Http { status, url } => ScraperError::Http { status, url },
            CrawlError::InvalidUrl(msg) => ScraperError::InvalidUrl(msg),
            CrawlError::Io(e) => ScraperError::Io(e),
            CrawlError::WafChallenge { provider, url, .. } => {
                ScraperError::WafBlocked { url, provider }
            },
            // LCOV_EXCL_LINE defensive: error-pass-through — Internal maps 1:1 and is invariant-guarded upstream
            CrawlError::Internal(msg) => ScraperError::Internal(msg),
            // Row 33 (P6-2 slice 2): invalid run description — Config keeps
            // the exit-78 contract (same contract family as matrix row 31).
            CrawlError::InvalidSession(msg) => ScraperError::Config(msg),
            // LCOV_EXCL_LINE defensive: error-pass-through — RequestFailed collapses into Internal and is invariant-guarded upstream
            CrawlError::RequestFailed(msg) => ScraperError::Internal(msg),
            CrawlError::Download(e) => ScraperError::Download(e),
            CrawlError::SitemapNotFound(url) => ScraperError::SitemapNotFound(url),
            CrawlError::Parse(msg) => ScraperError::Internal(format!("parse: {msg}")),
            CrawlError::MaxDepthExceeded { current, max } => {
                ScraperError::CrawlLimit(format!("maximum depth {max} exceeded at depth {current}"))
            },
            CrawlError::MaxPagesExceeded { max } => {
                ScraperError::CrawlLimit(format!("maximum pages {max} exceeded"))
            },
            CrawlError::UrlExcluded(url) => ScraperError::Internal(format!("URL excluded: {url}")),
            CrawlError::InvalidContentType(ct) => {
                ScraperError::Internal(format!("invalid content type: {ct}"))
            },
            CrawlError::Storage(msg) => ScraperError::Internal(format!("storage: {msg}")),
            CrawlError::Checkpoint(msg) => ScraperError::Internal(format!("checkpoint: {msg}")),
            CrawlError::SessionPool(msg) => ScraperError::Internal(format!("session pool: {msg}")),
            CrawlError::Discovery(msg) => ScraperError::Internal(format!("discovery: {msg}")),
            CrawlError::RetryExhausted { url, attempts } => ScraperError::Internal(format!(
                "retry exhausted for {url} after {attempts} attempts"
            )),
            CrawlError::TransientHttp { status, url } => ScraperError::Http { status, url },
            CrawlError::RateLimited(retry_after) => ScraperError::RateLimited(retry_after),
            // Timeout/Connection carry a synthetic io::Error source so
            // is_transient_network's downcast recognizes them as transient
            // (a bare message would fall through to InternalFatal).
            CrawlError::Timeout => ScraperError::Network(Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request timeout",
            ))),
            CrawlError::Connection(msg) => ScraperError::Network(Box::new(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                msg,
            ))),
            // Rows 11/12 (#957): typed 1:1 pass-through. Flattening every
            // kind into `Internal` collapsed row 11 (DomainRecoverable) and
            // row 12 (TransientBackoff) to InternalFatal at this layer.
            CrawlError::ResourceExhausted {
                resource,
                limit,
                actual,
            } => ScraperError::ResourceExhausted {
                resource,
                limit,
                actual,
            },
            CrawlError::SitemapEmpty => ScraperError::SitemapEmpty,
            // Row 11: sitemap-depth is a budget stop, same family as
            // max-depth/max-pages, so it shares `CrawlLimit`, which
            // classifies `DomainRecoverable` at this layer (#957).
            CrawlError::SitemapDepthExceeded => {
                ScraperError::CrawlLimit("sitemap depth exceeded".to_string())
            },
            CrawlError::SemaphoreInanition => ScraperError::SemaphoreInanition,
            // Cancellation is an engine control signal (#509), not a failure —
            // the crawl engine consumes it before any error surfaces here.
            // Defensive mapping only.
            // LCOV_EXCL_START defensive: engine-control-signal — cancellation is consumed by the engine before it surfaces here
            CrawlError::Cancelled => {
                ScraperError::Internal("task cancelled by engine shutdown".to_string())
            },
            // LCOV_EXCL_STOP
            // #2338: the CLI intercepts `PartialRun` BEFORE this conversion to
            // persist the partial results, so this arm only serves consumers
            // that do not match the typed variant (batch, MCP fallthrough).
            // Mapping to `Internal` preserves the exact ScraperError the same
            // failure produced before the variant existed.
            CrawlError::PartialRun { message, .. } => ScraperError::Internal(message),
        }
    }
}

impl From<crate::domain::error::DomainError> for ScraperError {
    fn from(e: crate::domain::error::DomainError) -> Self {
        match e {
            crate::domain::error::DomainError::InvalidUrl(msg) => ScraperError::InvalidUrl(msg),
            crate::domain::error::DomainError::Readability(msg) => ScraperError::Readability(msg),
            crate::domain::error::DomainError::Extraction(msg) => ScraperError::Extraction(msg),
            crate::domain::error::DomainError::ExtractionFailed { url, reason } => {
                ScraperError::ExtractionFailed { url, reason }
            },
            crate::domain::error::DomainError::Validation(msg) => ScraperError::Validation(msg),
            crate::domain::error::DomainError::FeatureGated(msg) => ScraperError::FeatureGated(msg),
            crate::domain::error::DomainError::Conversion(msg) => ScraperError::Conversion(msg),
        }
    }
}

impl From<crate::infrastructure::error::InfraError> for ScraperError {
    fn from(e: crate::infrastructure::error::InfraError) -> Self {
        match e {
            crate::infrastructure::error::InfraError::Http { status, url } => {
                ScraperError::Http { status, url }
            },
            crate::infrastructure::error::InfraError::Network(e) => ScraperError::Network(e),
            crate::infrastructure::error::InfraError::Middleware(msg) => {
                ScraperError::Middleware(msg)
            },
            crate::infrastructure::error::InfraError::WafBlocked { url, provider } => {
                ScraperError::WafBlocked { url, provider }
            },
            crate::infrastructure::error::InfraError::Download(e) => ScraperError::Download(e),
            crate::infrastructure::error::InfraError::GlobalTimeout => ScraperError::GlobalTimeout,
            crate::infrastructure::error::InfraError::SlowlorisTimeout => {
                ScraperError::SlowlorisTimeout
            },
            crate::infrastructure::error::InfraError::PayloadTooLarge => {
                ScraperError::PayloadTooLarge
            },
            crate::infrastructure::error::InfraError::SemaphoreInanition => {
                ScraperError::SemaphoreInanition
            },
            crate::infrastructure::error::InfraError::Persistence(msg) => {
                ScraperError::Persistence(msg)
            },
            crate::infrastructure::error::InfraError::Ingestion(msg) => {
                ScraperError::Ingestion(msg)
            },
            crate::infrastructure::error::InfraError::H2Config(msg) => ScraperError::H2Config(msg),
            crate::infrastructure::error::InfraError::UrlParse(e) => ScraperError::UrlParse(e),
            crate::infrastructure::error::InfraError::Io(e) => ScraperError::Io(e),
            crate::infrastructure::error::InfraError::FeatureGated(msg) => {
                ScraperError::FeatureGated(msg)
            },
        }
    }
}

impl From<crate::domain::UnknownProfileError> for ScraperError {
    /// An unrecognized H2/TLS profile name is a configuration error. The
    /// Spanish user-facing message from `UnknownProfileError` is preserved
    /// verbatim inside the [`ScraperError::Config`] payload.
    fn from(e: crate::domain::UnknownProfileError) -> Self {
        ScraperError::Config(e.to_string())
    }
}

impl From<crate::infrastructure::downloader::DownloadError> for ScraperError {
    /// Page-download failures map to [`ScraperError::Download`], preserving the
    /// full cause chain (`#[source]`) for `Error::source()` traversal (D4).
    /// `FeatureGated` is mapped directly to `FeatureGated` (PermanentFatal,
    /// Spanish, exit 78) instead of being erased into `Download`.
    fn from(e: crate::infrastructure::downloader::DownloadError) -> Self {
        match e {
            crate::infrastructure::downloader::DownloadError::FeatureGated(msg) => {
                ScraperError::FeatureGated(msg)
            },
            other => ScraperError::Download(Box::new(other)),
        }
    }
}
