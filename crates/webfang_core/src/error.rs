//! Error handling module for webfang.
//!
//! Uses thiserror for library error types (err-thiserror-lib).
//! This provides type-safe, structured error handling instead of anyhow.

use crate::domain::error::ResourceKind;
use thiserror::Error;
use wreq::Error as WreqError;

/// Main error type for the scraper library.
///
/// Each variant represents a specific failure mode, making it easy to:
/// - Handle specific errors in calling code
/// - Convert to/from other error types
/// - Provide meaningful error messages to users
#[derive(Error, Debug)]
pub enum ScraperError {
    /// URL is invalid or cannot be parsed
    #[error("URL inválida: {0}")]
    InvalidUrl(String),

    /// HTTP request failed with a status code
    #[error("http error {status} al acceder a {url}")]
    Http {
        /// The HTTP status code
        status: u16,
        /// The URL that was being accessed
        url: String,
    },

    /// Content extraction failed (Readability algorithm)
    #[error("Error de legibilidad: {0}")]
    Readability(String),

    /// I/O error (file system, etc.)
    #[error("Error de I/O: {0}")]
    Io(#[from] std::io::Error),

    /// Network error (connection failed, timeout, etc.)
    ///
    /// Carries the underlying error as `#[source]` so the root-cause chain
    /// (e.g. `wreq::Error` → I/O → timeout) is preserved for `Error::source()`
    /// traversal instead of being flattened to a `String` (D4).
    #[error("error de red: {0}")]
    Network(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Middleware error (from reqwest-middleware, e.g., retry failures)
    #[error("error de middleware: {0}")]
    Middleware(String),

    /// Serialization/Deserialization error (JSON, YAML, etc.)
    #[error("Error de serialización: {0}")]
    Serialization(#[from] serde_json::Error),

    /// URL parse error
    #[error("Error de parseo de URL: {0}")]
    UrlParse(#[from] url::ParseError),

    /// Asset extraction failed
    #[error("Error de extracción: {0}")]
    Extraction(String),

    /// Asset download failed
    ///
    /// Carries the underlying error as `#[source]` to preserve the root-cause
    /// chain (D4). Previously flattened to a `String`.
    #[error("Error de descarga: {0}")]
    Download(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Configuration error
    #[error("Error de configuración: {0}")]
    Config(String),

    /// Feature not yet implemented (gated for future release)
    #[error("funcionalidad no disponible: {0}")]
    FeatureGated(String),

    /// WAF/CAPTCHA challenge detected — on any response status, not only 200.
    ///
    /// The `provider` field carries the full Spanish evidence chain
    /// (`provider (patrón: …, tier: …)` per evidence, joined by `; `) when raised
    /// from a context-aware inspection verdict (REQ-WAF-08); legacy callers may
    /// still pass a bare provider name.
    #[error("WAF/CAPTCHA detectado en {url}: {provider}")]
    WafBlocked {
        /// URL that was blocked
        url: String,
        /// Detected WAF provider or formatted evidence chain (e.g. `"Cloudflare"`,
        /// or `"Cloudflare (patrón: cf-turnstile, tier: desafío); …"`)
        provider: String,
    },

    /// URL validation failed
    #[error("Validación de URL falló: {0}")]
    Validation(String),

    /// Conversion error (HTML to Markdown, YAML, etc.)
    #[error("Error de conversión: {0}")]
    Conversion(String),

    /// Export operation failed
    #[error("Error de exportación: {0}")]
    Export(String),

    /// Content extraction failed (poor fallback content)
    #[error("extracción falló para {url}: {reason}")]
    ExtractionFailed {
        /// URL that failed
        url: String,
        /// Reason for failure
        reason: String,
    },

    /// Batch export failed (partial success)
    #[error("Error de exportación en batch: {0}")]
    ExportBatch(String),

    /// Global download timeout (30s)
    #[error("descarga superó tiempo global de 30 segundos")]
    GlobalTimeout,

    /// Slowloris attack detected (per-chunk timeout)
    #[error("descarga superó timeout de inactividad de 5 segundos por chunk")]
    SlowlorisTimeout,

    /// Payload exceeded 25MB limit
    #[error("recurso superó límite de 25 MB")]
    PayloadTooLarge,

    /// Semaphore exhausted (backpressure)
    #[error("semáforo agotado: no hay permisos disponibles")]
    SemaphoreInanition,

    /// Persistence error (SQLite storage layer — resources/chunks CRUD, pool).
    ///
    /// Holds the underlying error rendered to a string so it uniformly covers
    /// both `rusqlite::Error` and `deadpool_sqlite` pool errors (which are
    /// different types) without forcing two `#[from]` variants.
    #[error("Error de persistencia: {0}")]
    Persistence(String),

    /// Elastic ingestion pipeline error (orchestration / dispatch failures).
    #[error("Error de ingestión: {0}")]
    Ingestion(String),

    /// Semantic cleaning error (AI-powered content processing)
    #[error("Error de limpieza semántica: {0}")]
    Semantic(#[from] SemanticError),

    /// HTTP/2 configuration error (ALPN, settings, or handshake failure)
    #[error("Error de configuración HTTP/2: {0}")]
    H2Config(String),

    /// Internal error from domain layer
    #[error("error interno: {0}")]
    Internal(String),

    /// Rate limited by the server; carries the retry-after delay in seconds.
    ///
    /// Classifies as [`ErrorClass::TransientBackoff`] — callers should wait
    /// the indicated delay before retrying. Display is deliberately identical
    /// to `CrawlError::RateLimited` so the message survives layer conversion.
    #[error("rate limited, retry after {0}s")]
    RateLimited(u64),

    /// Crawl limit exceeded.
    ///
    /// Groups all crawl-budget configuration limits — max-depth, max-pages,
    /// and sitemap-depth. All three are `DomainRecoverable` budget stops
    /// (matrix rows 9 and 11; see [`Self::classify`]); the payload carries
    /// the human-readable limit description rendered verbatim by `Display`.
    #[error("{0}")]
    CrawlLimit(String),

    /// Resource limit exhausted (budget backpressure).
    ///
    /// 1:1 pass-through of `CrawlError::ResourceExhausted` so the resource
    /// kind survives the layer boundary and [`Self::classify`] can split
    /// matrix row 12 (RAM backpressure, retriable after backoff) from row 11
    /// (sitemap budgets, by-design stops). `Display` is deliberately
    /// identical to `CrawlError::ResourceExhausted` so the message survives
    /// layer conversion.
    #[error("resource exhausted: {resource:?} limit={limit} actual={actual}")]
    ResourceExhausted {
        /// Which resource limit was hit.
        resource: ResourceKind,
        /// Configured limit value.
        limit: usize,
        /// Actual usage that exceeded the limit.
        actual: usize,
    },

    /// Sitemap contained no URLs (valid XML but empty urlset)
    #[error("no URLs found in sitemap")]
    SitemapEmpty,

    /// Sitemap not found during auto-discovery
    #[error("sitemap not found for {0}")]
    SitemapNotFound(String),
}

/// Semantic cleaning errors
///
/// These errors occur while the semantic cleaner produces embeddings and
/// filters chunks. Since ADR-0004 paso 6 the cleaner talks to a remote
/// embedding endpoint, so there is no model to load, download or cache — the
/// variants that described those failures were removed in 4.0.0 rather than
/// left behind as dead surface.
///
/// # Examples
///
/// ```
/// use webfang_core::error::SemanticError;
///
/// let semantic_err = SemanticError::ChunkTooLarge {
///     chunk_id: "chunk-1".to_string(),
///     chars: 600,
///     max: 512,
/// };
/// assert!(semantic_err.to_string().contains("600"));
/// ```
#[derive(Error, Debug)]
pub enum SemanticError {
    /// Text could not be turned into a chunk
    ///
    /// Raised by the chunker for input it cannot segment — today the only
    /// producer is an empty text. There is no tokenizer to fail: the cleaner
    /// budgets in characters (ADR-0004).
    #[error("Error tokenizando texto: {0}")]
    Tokenize(String),

    /// The embedding pipeline failed
    ///
    /// Covers both halves of the pipeline, and the payload says which one
    /// failed:
    /// - the remote endpoint (status, degenerate body, vector count, dimension)
    /// - the local consistency checks (chunks vs vectors, relevance filter)
    #[error("Error del pipeline de embeddings: {0}")]
    Inference(String),

    /// Content chunk exceeds the configured character budget
    ///
    /// This occurs when a single chunk of content is longer than the
    /// configured `--max-chars` budget. The unit is CHARACTERS, not tokens
    /// (ADR-0004): the cleaner no longer tokenizes, and the same budget has to
    /// mean the same thing for a local ONNX model and a remote embedding
    /// endpoint.
    ///
    /// # Fields
    ///
    /// * `chunk_id` - Identifier of the problematic chunk
    /// * `chars` - Actual character count
    /// * `max` - Maximum allowed characters
    #[error("Chunk {chunk_id} excede límite de caracteres: {chars} > {max}")]
    ChunkTooLarge {
        /// Identifier of the chunk (UUID or index)
        chunk_id: String,
        /// Actual character count
        chars: usize,
        /// Maximum allowed characters per chunk
        max: usize,
    },

    /// Invalid chunk locator span
    ///
    /// Raised when a chunker implementation would build a byte span with
    /// `start > end` — a programming bug in the implementation, never user
    /// input. See [`crate::domain::text_chunker::ByteSpan`].
    #[error("Rango de bytes inválido para el chunk: inicio {start} > fin {end}")]
    InvalidSpan {
        /// Byte offset where the span starts.
        start: usize,
        /// Byte offset where the span ends.
        end: usize,
    },
}

/// Result type alias using ScraperError as the error type.
pub type Result<T> = std::result::Result<T, ScraperError>;

impl ScraperError {
    /// Create an InvalidUrl error
    #[must_use]
    pub fn invalid_url(msg: impl Into<String>) -> Self {
        Self::InvalidUrl(msg.into())
    }

    /// Create an Http error
    #[must_use]
    pub fn http(status: u16, url: &str) -> Self {
        Self::Http {
            status,
            url: url.to_string(),
        }
    }

    /// Create a WafBlocked error
    #[must_use]
    pub fn waf_blocked(url: impl Into<String>, provider: impl Into<String>) -> Self {
        Self::WafBlocked {
            url: url.into(),
            provider: provider.into(),
        }
    }

    /// Create a Readability error
    #[must_use]
    pub fn readability(msg: impl Into<String>) -> Self {
        Self::Readability(msg.into())
    }

    /// Create an Extraction error
    #[must_use]
    pub fn extraction(msg: impl Into<String>) -> Self {
        Self::Extraction(msg.into())
    }

    /// Create a Download error, preserving the underlying error as the cause
    /// chain (`#[source]`) so `Error::source()` traversal works (D4).
    #[must_use]
    pub fn download(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::Download(e)
    }

    /// Create a Conversion error
    #[must_use]
    pub fn conversion(msg: impl Into<String>) -> Self {
        Self::Conversion(msg.into())
    }

    /// Create an Export error
    #[must_use]
    pub fn export(msg: impl Into<String>) -> Self {
        Self::Export(msg.into())
    }

    /// Create an ExportBatch error
    #[must_use]
    pub fn export_batch(msg: impl Into<String>) -> Self {
        Self::ExportBatch(msg.into())
    }

    /// Create a Persistence error from anything displayable.
    ///
    /// Used to uniformly convert `rusqlite::Error` and `deadpool_sqlite` pool
    /// errors into `ScraperError::Persistence` without `#[from]` ambiguity.
    #[must_use]
    pub fn persistence(err: impl std::fmt::Display) -> Self {
        Self::Persistence(err.to_string())
    }

    /// Create an Ingestion error from anything displayable.
    #[must_use]
    pub fn ingestion(err: impl std::fmt::Display) -> Self {
        Self::Ingestion(err.to_string())
    }
}

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

/// Operational classification of errors for observability and retry logic.
///
/// Moved to the domain layer per the Error Classification Matrix
/// (`docs/error-classification-matrix.md`); re-exported here so all existing
/// `crate::error::ErrorClass` consumers compile unchanged.
pub use crate::domain::error::ErrorClass;

impl SemanticError {
    /// Classify this error by operational severity using the unified
    /// [`ErrorClass`] taxonomy shared with [`crate::error::ScraperError`].
    #[must_use]
    pub const fn classify(&self) -> ErrorClass {
        match self {
            Self::Inference(_) => ErrorClass::InternalFatal,
            Self::ChunkTooLarge { .. } | Self::Tokenize(_) | Self::InvalidSpan { .. } => {
                ErrorClass::DomainRecoverable
            },
        }
    }
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

mod classification;

#[cfg(test)]
mod tests_basics;

#[cfg(test)]
mod tests_conversions;

#[cfg(test)]
mod tests_classification;

#[cfg(test)]
mod tests_network;

#[cfg(test)]
mod tests_crawl_error;
