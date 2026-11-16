//! HTTP-specific errors with status code information
//!
//! Variants provide specific handling hints:
//! - `Forbidden`: 403 - retry with different UA
//! - `RateLimited`: 429 - respect Retry-After header
//! - `ClientError` / `ServerError`: other 4xx/5xx codes
//!
//! This type lives in the **domain** layer. The HTTP port contract
//! (`HttpClientPort`) is owned by the domain so that the application and
//! infrastructure layers depend inward on it, never the other way around.

/// Result type for HttpClient operations
pub type HttpResult<T> = Result<T, HttpError>;

/// HTTP-specific errors with status code information
///
/// Variants provide specific handling hints:
/// - `Forbidden`: 403 - retry with different UA
/// - `RateLimited`: 429 - respect Retry-After header
/// - `ClientError` / `ServerError`: other 4xx/5xx codes
/// - `BodyTooLarge`: response body exceeded the configured limit
#[derive(Debug)]
pub enum HttpError {
    /// 403 Forbidden - site blocking
    Forbidden,
    /// 429 Rate Limited - contains retry-after seconds
    RateLimited(u64),
    /// Other 4xx errors - contains status code
    ClientError(u16),
    /// 5xx server errors - contains status code
    ServerError(u16),
    /// Request timeout
    ///
    /// The classified cause travels as the `source` when the producer still holds
    /// the transport error (#2841): the verdict and the `Display` text do not
    /// move, the cause is ADDED.
    Timeout {
        /// The classified transport failure, when one is available.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },
    /// Connection error - contains error message
    ///
    /// The message is its own field so the [`Display`](std::fmt::Display) text
    /// stays byte-identical to the former `Connection(String)` shape. The
    /// original transport error, when the producer still holds it, travels as
    /// the `source`, so the typed cause survives the boundary instead of being
    /// flattened into the message (#2784).
    Connection {
        /// Human-readable message (unchanged from the former tuple payload).
        message: String,
        /// The original transport error, when one is available.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },
    /// Request building/error - contains error message
    ///
    /// Reserved for request CONSTRUCTION failures (unsupported scheme,
    /// malformed URL, a builder error raised at `send`). A transport failure is
    /// NOT a request error: it belongs to [`Self::Transport`] (#2841).
    Request(String),
    /// Transport failure that is neither a timeout nor a connection failure.
    ///
    /// The home of the failures the used-to-be `else` branch flattened into
    /// [`Self::Request`]: the transport error keeps its own text and, when the
    /// producer classified it, the typed cause travels as `source` (#2841).
    Transport {
        /// Human-readable message (the transport error's own text).
        message: String,
        /// The original transport error, when one is available.
        source: Option<crate::domain::error::cause::ErrorSource>,
    },
    /// WAF/CAPTCHA challenge detected in HTTP 200 (false positive)
    WafChallenge(String),
    /// Domain banned — no sessions available in the pool
    DomainBanned(String),
    /// Response body exceeded the configured limit
    BodyTooLarge {
        /// Maximum number of bytes allowed.
        limit: u64,
    },
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Forbidden => write!(f, "403 Forbidden - site blocking"),
            HttpError::RateLimited(retry_after) => {
                write!(f, "429 Rate Limited - retry after {retry_after} seconds")
            },
            HttpError::ClientError(code) => write!(f, "Client Error {code}"),
            HttpError::ServerError(code) => write!(f, "Server Error {code}"),
            HttpError::Timeout { .. } => write!(f, "Request Timeout"),
            HttpError::Connection { message, .. } => write!(f, "Connection Error: {message}"),
            HttpError::Request(msg) => write!(f, "Request Error: {msg}"),
            HttpError::Transport { message, .. } => {
                write!(f, "Transport Error: {message}")
            },
            HttpError::WafChallenge(provider) => {
                write!(f, "WAF/CAPTCHA challenge detected ({provider})")
            },
            HttpError::DomainBanned(domain) => {
                write!(f, "domain banned: {domain} — no sessions available")
            },
            HttpError::BodyTooLarge { limit } => {
                write!(f, "Response body exceeded the limit of {limit} bytes")
            },
        }
    }
}

impl std::error::Error for HttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // The preserved transport error is the cause, so a consumer that
            // walks `source()` reaches the same typed chain the producer saw.
            Self::Connection {
                source: Some(source),
                ..
            }
            | Self::Timeout {
                source: Some(source),
                ..
            }
            | Self::Transport {
                source: Some(source),
                ..
            } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl HttpError {
    /// A connection failure whose cause is not available.
    #[must_use]
    pub fn connection(message: impl Into<String>) -> Self {
        Self::Connection {
            message: message.into(),
            source: None,
        }
    }

    /// A copy of this error without its preserved cause.
    ///
    /// The fixtures and the mocks SIMULATE failures: they have no transport
    /// error to carry, and a boxed source cannot be cloned. One place decides
    /// what "the same error, without the cause" means for every variant.
    #[must_use]
    pub fn without_cause(&self) -> Self {
        match self {
            Self::Forbidden => Self::Forbidden,
            Self::RateLimited(seconds) => Self::RateLimited(*seconds),
            Self::ClientError(code) => Self::ClientError(*code),
            Self::ServerError(code) => Self::ServerError(*code),
            Self::Timeout { .. } => Self::timeout(),
            Self::Connection { message, .. } => Self::connection(message.clone()),
            Self::Request(message) => Self::Request(message.clone()),
            Self::Transport { message, .. } => Self::transport(message.clone(), None),
            Self::WafChallenge(provider) => Self::WafChallenge(provider.clone()),
            Self::DomainBanned(domain) => Self::DomainBanned(domain.clone()),
            Self::BodyTooLarge { limit } => Self::BodyTooLarge { limit: *limit },
        }
    }

    /// A connection failure that keeps the original transport error as its
    /// cause (#2784).
    #[must_use]
    pub fn connection_with_cause(
        message: impl Into<String>,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self::Connection {
            message: message.into(),
            source: Some(source),
        }
    }

    /// A timeout whose classified cause is not available.
    #[must_use]
    pub const fn timeout() -> Self {
        Self::Timeout { source: None }
    }

    /// A timeout that keeps the classified cause (#2841): the `Display` text
    /// stays `"Request Timeout"`, the chain gains the cause.
    #[must_use]
    pub fn timeout_with_cause(source: crate::domain::error::cause::ErrorSource) -> Self {
        Self::Timeout { source: Some(source) }
    }

    /// A transport failure, with its cause when the producer classified one.
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

    #[test]
    fn test_http_error_forbidden() {
        let err = HttpError::Forbidden;
        assert!(matches!(err, HttpError::Forbidden));
    }

    #[test]
    fn test_http_error_rate_limited() {
        let err = HttpError::RateLimited(60);
        assert!(matches!(err, HttpError::RateLimited(60)));

        let err2 = HttpError::RateLimited(30);
        assert!(!matches!(err2, HttpError::RateLimited(60)));
    }

    #[test]
    fn test_http_error_client_error() {
        let err = HttpError::ClientError(404);
        assert!(matches!(err, HttpError::ClientError(404)));
    }

    #[test]
    fn test_http_error_server_error() {
        let err = HttpError::ServerError(500);
        assert!(matches!(err, HttpError::ServerError(500)));
    }

    #[test]
    fn test_http_error_timeout() {
        let err = HttpError::timeout();
        assert!(matches!(err, HttpError::Timeout { .. }));
    }

    /// #2841: the `Display` text does not move when the cause is added.
    #[test]
    fn test_http_error_timeout_display_is_unchanged_with_a_cause() {
        let cause = std::io::Error::new(std::io::ErrorKind::TimedOut, "read timeout");
        let bare = HttpError::timeout();
        let with_cause = HttpError::timeout_with_cause(Box::new(cause));
        assert_eq!(bare.to_string(), "Request Timeout");
        assert_eq!(with_cause.to_string(), bare.to_string());
        // ...and the cause is reachable through `Error::source`.
        let source = std::error::Error::source(&with_cause).expect("the cause must be preserved");
        assert_eq!(source.to_string(), "read timeout");
    }

    #[test]
    fn test_http_error_connection() {
        let err = HttpError::connection("Connection refused");
        assert!(matches!(err, HttpError::Connection { .. }));
    }

    /// #2784: the `Display` is byte-identical to the former `Connection(String)`
    /// shape — the payload changed, the message did not.
    #[test]
    fn test_http_error_connection_display_is_unchanged() {
        let err = HttpError::connection("Connection refused");
        assert_eq!(err.to_string(), "Connection Error: Connection refused");
    }

    /// #2784: a preserved cause is reachable through `Error::source`.
    #[test]
    fn test_http_error_connection_exposes_its_cause() {
        let cause = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let err = HttpError::connection_with_cause("Connection refused", Box::new(cause));
        let source = std::error::Error::source(&err).expect("the cause must be preserved");
        assert_eq!(source.to_string(), "refused");
        // The message is unchanged by the presence of a cause.
        assert_eq!(err.to_string(), "Connection Error: Connection refused");
    }

    #[test]
    fn test_http_error_request() {
        let err = HttpError::Request("Invalid URL".into());
        assert!(matches!(err, HttpError::Request(_)));
    }

    #[test]
    fn test_http_error_display() {
        assert_eq!(
            format!("{}", HttpError::Forbidden),
            "403 Forbidden - site blocking"
        );
        assert_eq!(
            format!("{}", HttpError::RateLimited(30)),
            "429 Rate Limited - retry after 30 seconds"
        );
        assert_eq!(
            format!("{}", HttpError::ClientError(404)),
            "Client Error 404"
        );
        assert_eq!(
            format!("{}", HttpError::ServerError(500)),
            "Server Error 500"
        );
        assert_eq!(format!("{}", HttpError::timeout()), "Request Timeout");
    }

    #[test]
    fn test_http_error_domain_banned() {
        let err = HttpError::DomainBanned("example.com".into());
        assert!(matches!(err, HttpError::DomainBanned(_)));
        assert_eq!(
            format!("{err}"),
            "domain banned: example.com — no sessions available"
        );
    }
}
