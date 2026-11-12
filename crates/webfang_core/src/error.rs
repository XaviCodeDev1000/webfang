//! Error handling module for webfang.
//!
//! Uses thiserror for library error types (err-thiserror-lib).
//! This provides type-safe, structured error handling instead of anyhow.

use crate::domain::error::ResourceKind;
use thiserror::Error;

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

mod classification;
mod conversions;

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
