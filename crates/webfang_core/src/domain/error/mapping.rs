//! The mappings that turn a transport failure into a typed one (#2784).
//!
//! They live in the domain because BOTH fetch clients need them and the
//! application layer must not import infrastructure: the mapping is a
//! contract, not an implementation detail.

use crate::domain::downloader_port::DownloadError;
use crate::domain::error::cause::{
    cause_in_chain, classify, ClassifiedNetworkError, DnsResolutionError, FetchPhase,
};
use crate::domain::error::resolution::refine_connect_cause;

/// Map a transport error to the HTTP failure it is.
#[must_use]
pub fn http_failure(
    error: wreq::Error,
    host: Option<&str>,
    request_started: std::time::Instant,
) -> crate::domain::http_error::HttpError {
    use crate::domain::http_error::HttpError;
    if error.is_timeout() {
        HttpError::Timeout
    } else if error.is_connect() {
        http_connection_failure(error, FetchPhase::Connect, host, request_started)
    } else {
        HttpError::Request(error.to_string())
    }
}

/// Map a transport error to the download failure it is.
#[must_use]
pub fn download_failure(
    error: wreq::Error,
    timeout_secs: u64,
    phase: FetchPhase,
    host: Option<&str>,
    request_started: std::time::Instant,
) -> DownloadError {
    use DownloadError;
    let original = if error.is_timeout() {
        DownloadError::Timeout(timeout_secs)
    } else {
        DownloadError::from(error)
    };
    let cause = classify(&original, phase);
    let cause = match host {
        Some(host) => refine_connect_cause(cause, host, request_started),
        None => cause,
    };
    let original = match original {
        DownloadError::Network(inner) => {
            DownloadError::Network(Box::new(ClassifiedNetworkError::new(cause, inner)))
        },
        other => other,
    };
    with_cause(original, phase)
}

/// Attach the classified cause to a transport failure, keeping every other
/// variant untouched.
///
/// A builder error stays `InvalidUrl`, a timeout stays `Timeout`, a
/// `BodyTooLarge` stays `BodyTooLarge`: the taxonomy ADDS knowledge, it never
/// moves a verdict (#2784).
#[must_use]
pub fn with_cause(
    error: crate::domain::downloader_port::DownloadError,
    phase: FetchPhase,
) -> crate::domain::downloader_port::DownloadError {
    use crate::domain::downloader_port::DownloadError;
    if cause_in_chain(&error).is_some() {
        return error;
    }
    let cause = classify(&error, phase);
    match error {
        DownloadError::Network(inner) => {
            DownloadError::Network(Box::new(ClassifiedNetworkError::new(cause, inner)))
        },
        other => other,
    }
}

/// Build the crawler failure for a transport error, with the cause attached.
///
/// The ORIGINAL transport error stays the cause's source, so the chain a
/// consumer walks reaches the error the client actually produced (#2784).
#[must_use]
pub fn crawl_network_failure(
    error: wreq::Error,
    phase: FetchPhase,
    url: &url::Url,
    request_started: std::time::Instant,
) -> crate::domain::error::CrawlError {
    use crate::domain::error::CrawlError;
    let message = error.to_string();
    let status_code = error.status().map(|status| status.as_u16());
    let cause = classify(&error, phase);
    let cause = match url.host_str() {
        Some(host) => refine_connect_cause(cause, host, request_started),
        None => cause,
    };
    CrawlError::Network {
        message,
        status_code,
        source: Some(Box::new(ClassifiedNetworkError::new(
            cause,
            Box::new(error),
        ))),
    }
}

/// Build the HTTP failure for a transport error, with the cause attached.
///
/// The message is rendered before the error is moved, so the text an operator
/// reads is unchanged; the cause travels beside it (#2784).
#[must_use]
pub fn http_connection_failure(
    error: wreq::Error,
    phase: FetchPhase,
    host: Option<&str>,
    request_started: std::time::Instant,
) -> crate::domain::http_error::HttpError {
    use crate::domain::http_error::HttpError;
    let message = error.to_string();
    let cause = classify(&error, phase);
    let cause = match host {
        Some(host) => refine_connect_cause(cause, host, request_started),
        None => cause,
    };
    HttpError::Connection {
        message,
        source: Some(Box::new(ClassifiedNetworkError::new(
            cause,
            Box::new(error),
        ))),
    }
}

/// Wrap a failed name lookup as a typed RESOLUTION failure.
///
/// A resolution that fails is not a connection failure, and the classifier
/// reads the TYPE, never the message (#2784). The SSRF scan's own rejection is
/// deliberately NOT wrapped: a refusal is not a DNS failure.
#[must_use]
pub fn dns_resolution_failure(
    error: Box<dyn std::error::Error + Send + Sync>,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(DnsResolutionError::new(error))
}
