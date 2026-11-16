//! Downloader port — domain-owned trait for page fetching.
//!
//! Extracted from `infrastructure::downloader` so `application::*` can depend
//! on the trait without `application→infrastructure` (ADR-0010). Infrastructure
//! (`WreqDownloader`, `ObscuraDownloader`, `ChromiumoxideDownloader`,
//! `HybridRouter`) implements this trait; application orchestrates via
//! `Arc<dyn Downloader>` (cloned before `.await`, never held across lock).

use std::collections::HashMap;

use futures::future::BoxFuture;
use url::Url;

use crate::domain::error::cause::{CauseCarrier, ClassifiedNetworkError};

/// Downloader trait for fetching pages — domain-owned port.
///
/// Dyn-compatible via `BoxFuture` (like `VectorRepository` in
/// `domain::repository`). Concrete impls live in `infrastructure::downloader`.
pub trait Downloader: Send + Sync {
    /// Fetch a page from the given URL.
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>>;

    /// Whether this downloader supports JavaScript rendering / interactions.
    fn supports_interactions(&self) -> bool;

    /// Estimated memory cost in bytes (for scheduler budgeting).
    fn memory_cost(&self) -> usize;
}

/// A page fetched by a [`Downloader`].
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// Final URL after redirects.
    pub url: Url,
    /// Raw HTML content.
    pub html: String,
    /// HTTP status code.
    pub status: u16,
    /// Response headers (keys lowercased).
    pub headers: HashMap<String, String>,
    /// Cookies set by the server during this request.
    pub cookies: Vec<Cookie>,
}

/// An HTTP cookie extracted from a response.
#[derive(Debug, Clone)]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain the cookie applies to.
    pub domain: String,
    /// Path prefix the cookie applies to.
    pub path: String,
    /// Whether the cookie is HTTP-only.
    pub http_only: bool,
    /// Whether the cookie requires HTTPS.
    pub secure: bool,
}

/// Errors that can occur during page download.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// Network-level failure (DNS, connection, timeout).
    #[error("network error: {0}")]
    Network(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// DNS resolution failure (NXDOMAIN).
    ///
    /// The message names the classified cause in Spanish and the transport
    /// detail behind it; the original error travels as the `source`, so the
    /// cause is in the chain and not only in the text (#2784).
    #[error("DNS error: {message}")]
    Dns {
        /// Human-readable description: the classified cause, then the detail.
        message: String,
        /// The original transport error: the classified cause rides in its chain.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },

    /// TLS/SSL failure (expired cert, self-signed).
    ///
    /// Same contract as [`DownloadError::Dns`]: the message names the cause
    /// and the original error travels as the `source` (#2784).
    #[error("TLS error: {message}")]
    Tls {
        /// Human-readable description: the classified cause, then the detail.
        message: String,
        /// The original transport error: the classified cause rides in its chain.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },

    /// I/O error with preserved `std::io::ErrorKind`.
    ///
    /// The kind is the retry rule's input and it survives the classified
    /// carrier: the carrier travels in the error's payload, where an
    /// `io::Error` keeps it without touching the kind (#2784).
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),

    /// HTTP error response (non-2xx status).
    #[error("HTTP {status}: {message}")]
    #[allow(missing_docs)]
    Http { status: u16, message: String },

    /// WAF challenge detected.
    #[error("WAF challenge detected: {0}")]
    WafChallenge(String),

    /// SPA detected — page requires JavaScript rendering.
    #[error("SPA detected: {0}")]
    SpaDetected(String),

    /// URL is invalid or has an unsupported scheme.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// Request timed out.
    ///
    /// The text stays the one the tool has always printed; the classified
    /// cause (a connect deadline or a read deadline) travels as the `source`,
    /// so the trace tells the two apart (#2784).
    #[error("request timed out after {secs}s")]
    Timeout {
        /// The deadline that elapsed, in seconds.
        secs: u64,
        /// The original transport error: the classified cause rides in its chain.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },

    /// Internal error (should not happen in normal operation).
    #[error("internal error: {0}")]
    Internal(String),

    /// System resources (RAM) insufficient to spawn a heavyweight downloader layer.
    #[error("insufficient resources: {0}")]
    ResourceExhausted(String),

    /// Feature not compiled in — typed so classification does not rely on string matching.
    #[error("funcionalidad no disponible: {0}")]
    FeatureGated(String),

    /// Acquisition cancelled by the engine's cancellation token.
    #[error("operation cancelled while waiting for resources")]
    Cancelled,

    /// Response body exceeded the configured size cap while streaming
    /// (FIX-1, #1231 F-12). The read is aborted mid-body, so memory stays
    /// bounded at ~cap + one chunk regardless of what the server sends —
    /// including decompression bombs announced with a tiny Content-Length.
    /// User-facing text in Spanish per the error-stratification convention.
    #[error("el cuerpo de la respuesta excede el límite de {limit} bytes")]
    BodyTooLarge {
        /// Configured cap in bytes (`--max-file-size`).
        limit: u64,
    },
}

/// A copy that keeps the classified cause (#2824).
///
/// The cause is part of the diagnosis: the retry clones this error, so a copy
/// that dropped the cause would turn a classified refusal into an anonymous
/// network error at the application boundary. The carrier is clonable (the
/// original error is shared behind an `Arc`), so the copy keeps the cause AND
/// the chain behind it.
///
/// The class never moves — it is what the retry rule reads — and a source that
/// is not the classified carrier cannot be cloned: the copy carries none, and
/// the variant, the message and the class survive.
impl Clone for DownloadError {
    fn clone(&self) -> Self {
        match self {
            DownloadError::Network(e) => {
                if let Some(carrier) = e.downcast_ref::<ClassifiedNetworkError>() {
                    DownloadError::Network(Box::new(carrier.clone()))
                } else if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                    DownloadError::Network(Box::new(std::io::Error::new(
                        io_err.kind(),
                        io_err.to_string(),
                    )))
                } else {
                    // Not a classified carrier: keep the variant (the class
                    // does not move) and the rendered message.
                    DownloadError::Network(Box::new(std::io::Error::other(e.to_string())))
                }
            },
            DownloadError::Dns { message, source } => DownloadError::Dns {
                message: message.clone(),
                source: clone_carrier(source.as_deref()),
            },
            DownloadError::Tls { message, source } => DownloadError::Tls {
                message: message.clone(),
                source: clone_carrier(source.as_deref()),
            },
            DownloadError::Io(e) => {
                // The kind is the retry rule's input: it survives the copy
                // untouched. The payload keeps the classified cause when it
                // carries one (#2824).
                let kind = e.kind();
                match clone_io_payload(e) {
                    Some(payload) => DownloadError::Io(std::io::Error::new(kind, payload)),
                    None => DownloadError::Io(std::io::Error::new(kind, e.to_string())),
                }
            },
            DownloadError::Http { status, message } => DownloadError::Http {
                status: *status,
                message: message.clone(),
            },
            DownloadError::WafChallenge(s) => DownloadError::WafChallenge(s.clone()),
            DownloadError::SpaDetected(s) => DownloadError::SpaDetected(s.clone()),
            DownloadError::InvalidUrl(s) => DownloadError::InvalidUrl(s.clone()),
            DownloadError::Timeout { secs, source } => DownloadError::Timeout {
                secs: *secs,
                source: clone_carrier(source.as_deref()),
            },
            DownloadError::Internal(s) => DownloadError::Internal(s.clone()),
            DownloadError::ResourceExhausted(s) => DownloadError::ResourceExhausted(s.clone()),
            DownloadError::FeatureGated(s) => DownloadError::FeatureGated(s.clone()),
            DownloadError::Cancelled => DownloadError::Cancelled,
            DownloadError::BodyTooLarge { limit } => DownloadError::BodyTooLarge { limit: *limit },
        }
    }
}

/// Clone the classified carrier a transport variant carries, when it has one.
///
/// A source that is not the classified carrier cannot be cloned (it is a boxed
/// error): the copy carries none.
fn clone_carrier(
    source: Option<&(dyn std::error::Error + Send + Sync + 'static)>,
) -> Option<crate::domain::error::cause::ErrorSource> {
    source
        .and_then(|source| source.downcast_ref::<ClassifiedNetworkError>())
        .map(|classified| -> crate::domain::error::cause::ErrorSource {
            Box::new(classified.clone())
        })
}

/// Clone the payload of an `io::Error` when it carries a classified cause.
///
/// The payload of a classified `Io` failure is a [`CauseCarrier`], whose
/// `source()` is the classified error; a payload that carries the carrier
/// directly is covered too. Any other payload cannot be cloned.
fn clone_io_payload(error: &std::io::Error) -> Option<crate::domain::error::cause::ErrorSource> {
    let payload = error.get_ref()?;
    if let Some(carrier) = payload.downcast_ref::<CauseCarrier>() {
        return Some(Box::new(carrier.clone()));
    }
    payload.downcast_ref::<ClassifiedNetworkError>().map(
        |classified| -> crate::domain::error::cause::ErrorSource { Box::new(classified.clone()) },
    )
}

impl DownloadError {
    /// A DNS resolution failure with no transport error behind it.
    #[must_use]
    pub fn dns(message: impl Into<String>) -> Self {
        Self::Dns {
            message: message.into(),
            source: None,
        }
    }

    /// A TLS failure with no transport error behind it.
    #[must_use]
    pub fn tls(message: impl Into<String>) -> Self {
        Self::Tls {
            message: message.into(),
            source: None,
        }
    }

    /// A timeout with no transport error behind it.
    #[must_use]
    pub fn timeout(secs: u64) -> Self {
        Self::Timeout { secs, source: None }
    }

    /// Classify this download error by operational severity.
    #[must_use]
    pub fn classify(&self) -> crate::domain::error::ErrorClass {
        use crate::domain::error::ErrorClass;

        match self {
            Self::Io(e) if is_transient_io(e) => ErrorClass::TransientRetriable,
            Self::Io(_) => ErrorClass::InternalFatal,
            Self::Dns { .. } | Self::Tls { .. } => ErrorClass::PermanentFatal,
            Self::Timeout { .. } => ErrorClass::TransientBackoff,
            Self::Http { status, .. } if (500..=599).contains(status) => {
                ErrorClass::TransientRetriable
            },
            Self::Http { status: 429, .. } => ErrorClass::TransientBackoff,
            Self::Http { status, .. } if (400..=499).contains(status) => ErrorClass::PermanentFatal,
            Self::Http { .. } => ErrorClass::PermanentFatal,
            Self::WafChallenge(_) | Self::SpaDetected(_) => ErrorClass::PermanentFatal,
            Self::InvalidUrl(_) => ErrorClass::PermanentFatal,
            Self::Network(_) => ErrorClass::InternalFatal,
            Self::Internal(_) => ErrorClass::InternalFatal,
            Self::ResourceExhausted(_) => ErrorClass::InternalFatal,
            Self::FeatureGated(_) => ErrorClass::PermanentFatal,
            Self::Cancelled => ErrorClass::InternalFatal,
            Self::BodyTooLarge { .. } => ErrorClass::PermanentFatal,
        }
    }
}

fn is_transient_io(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{
        BrokenPipe, ConnectionAborted, ConnectionReset, TimedOut, UnexpectedEof,
    };

    matches!(
        e.kind(),
        ConnectionReset | ConnectionAborted | BrokenPipe | TimedOut | UnexpectedEof
    )
}

impl From<DownloadError> for crate::domain::CrawlError {
    fn from(err: DownloadError) -> Self {
        crate::domain::CrawlError::Download(Box::new(err))
    }
}

#[cfg(test)]
mod tests;
