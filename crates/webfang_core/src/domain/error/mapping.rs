//! The mappings that turn a transport failure into a typed one (#2784).
//!
//! They live in the domain because BOTH fetch clients need them and the
//! application layer must not import infrastructure: the mapping is a
//! contract, not an implementation detail.

use crate::domain::downloader_port::DownloadError;
use crate::domain::error::cause::{
    cause_in_chain, classify, CauseCarrier, ClassifiedNetworkError, DnsResolutionError, FetchPhase,
    NetworkCause,
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
        HttpError::timeout()
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
    // The cause is decided on the RAW transport error: narrowing it into a
    // variant BELOW erases the resolver's typed failure and the I/O kinds
    // the classifier reads (#2784). The variant — and with it the class, the
    // retry rule and the exit code — still comes from the narrowing, so the
    // verdict never moves; only the cause is added.
    let timed_out = error.is_timeout();
    let cause = if timed_out {
        // The client's own deadline fired in this phase: the phase decides.
        phase_cause(phase)
    } else {
        classify(&error, phase)
    };
    let cause = match host {
        Some(host) => {
            refine_connect_cause(cause, host, request_started, phase == FetchPhase::Connect)
        },
        None => cause,
    };
    let original = if timed_out {
        DownloadError::Timeout {
            secs: timeout_secs,
            source: None,
        }
    } else {
        DownloadError::from(error)
    };
    attach_cause(original, cause)
}

/// Whether a transport failure is a NAME RESOLUTION failure.
///
/// The cause travels in the error's chain (#2784), so this is the one
/// predicate a retry policy reads to tell OUR broken resolver from a site
/// refusing us: a `dns_resolution` never reached the peer, so the
/// site-rejection ladder must not be spent on it (#2823).
///
/// A failure with NO classified cause is NOT a resolution failure: the
/// predicate fails open towards the historical behavior, never the other way
/// around — an unclassified error keeps the ladder it has today.
#[must_use]
pub fn is_resolution_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    cause_in_chain(error).is_some_and(NetworkCause::is_resolution)
}

/// Attach the classified cause to a transport failure, keeping every other
/// variant untouched.
///
/// A builder error stays `InvalidUrl`, a timeout keeps its own variant, a
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
    let cause = match &error {
        // A timeout carries no chain evidence: the phase decides.
        DownloadError::Timeout { .. } => phase_cause(phase),
        other => classify(other, phase),
    };
    attach_cause(error, cause)
}

/// Attach a KNOWN cause to a transport failure.
///
/// The cause travels in the error's CHAIN, beside the variant that decides
/// the class, the retry rule and the exit code — never instead of them
/// (#2784). Every transport variant has a place for it:
///
/// | variant | where the cause travels |
/// |---|---|
/// | `Network` | the carrier IS the payload |
/// | `Io` | the carrier is the I/O error's payload, behind a `CauseCarrier` (the kind survives) |
/// | `Dns` / `Tls` / `Timeout` | the carrier is the variant's `source` |
#[must_use]
pub fn attach_cause(error: DownloadError, cause: NetworkCause) -> DownloadError {
    use crate::domain::error::cause::ErrorSource;
    match error {
        DownloadError::Network(inner) => {
            DownloadError::Network(Box::new(ClassifiedNetworkError::new(cause, inner)))
        },
        DownloadError::Io(io_err) => {
            // An `io::Error` does not expose its payload as a source, but it
            // keeps the KIND — the retry rule's input — and renders the
            // payload, so the message names the cause (#2784). The payload is
            // a `CauseCarrier`, whose `source()` IS the classified error: the
            // plain `source()` walk reaches the typed cause without knowing
            // the `io::Error` quirk (#2824). The kind does not move.
            let kind = io_err.kind();
            DownloadError::Io(std::io::Error::new(
                kind,
                CauseCarrier::new(ClassifiedNetworkError::new(cause, Box::new(io_err))),
            ))
        },
        DownloadError::Dns { message, source } => {
            let original: ErrorSource =
                source.unwrap_or_else(|| Box::new(std::io::Error::other(message.clone())));
            DownloadError::Dns {
                message: compose(cause, &message),
                source: Some(Box::new(ClassifiedNetworkError::new(cause, original))),
            }
        },
        DownloadError::Tls { message, source } => {
            let original: ErrorSource =
                source.unwrap_or_else(|| Box::new(std::io::Error::other(message.clone())));
            DownloadError::Tls {
                message: compose(cause, &message),
                source: Some(Box::new(ClassifiedNetworkError::new(cause, original))),
            }
        },
        DownloadError::Timeout { secs, source } => {
            let original: ErrorSource = source.unwrap_or_else(|| {
                Box::new(std::io::Error::other(format!(
                    "request timed out after {secs}s"
                )))
            });
            DownloadError::Timeout {
                secs,
                source: Some(Box::new(ClassifiedNetworkError::new(cause, original))),
            }
        },
        other => other,
    }
}

/// The cause of a failure that carries no chain evidence: the phase decides.
///
/// A deadline that fired in the connect phase is a connect timeout; one that
/// fired while the response was on its way is a read timeout (#2784).
fn phase_cause(phase: FetchPhase) -> NetworkCause {
    match phase {
        FetchPhase::Connect => NetworkCause::ConnectTimeout,
        FetchPhase::Body => NetworkCause::ReadTimeout,
    }
}

/// The user-facing text of a classified failure: the cause in Spanish, then
/// the transport detail behind it.
///
/// The operator reads WHAT failed before the transport's own words, and the
/// two causes a run mixes are never the same sentence (#2784).
fn compose(cause: NetworkCause, detail: &str) -> String {
    format!("{}: {}", cause.message(), detail)
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
        Some(host) => {
            refine_connect_cause(cause, host, request_started, phase == FetchPhase::Connect)
        },
        None => cause,
    };
    CrawlError::Network {
        // The message names the cause: the batch report must tell "the site
        // refused us" from "our DNS failed" (#2784).
        message: compose(cause, &message),
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
/// reads keeps the transport's own words; the cause travels beside it, and
/// the message names it (#2784).
#[must_use]
/// The host of a URL, for the resolver's marker.
pub fn url_host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
}

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
        Some(host) => {
            refine_connect_cause(cause, host, request_started, phase == FetchPhase::Connect)
        },
        None => cause,
    };
    HttpError::Connection {
        message: compose(cause, &message),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::downloader_port::DownloadError;

    /// #2784: EVERY transport variant carries the cause.
    ///
    /// The cause used to travel only in the `Network` variant, so the
    /// narrowing of the transport error discarded it for exactly the
    /// failures that matter most: a refused connection (`Io`), a name that
    /// does not resolve (`Dns`), a handshake that fails (`Tls`) and a
    /// deadline (`Timeout`). Each one is pinned here.
    #[test]
    fn every_transport_variant_carries_the_cause() {
        let cases = [
            DownloadError::Network(Box::new(std::io::Error::other("boom"))),
            DownloadError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "connection refused",
            )),
            DownloadError::dns("name resolution failed"),
            DownloadError::tls("handshake failed"),
            DownloadError::timeout(30),
        ];

        for error in cases {
            let classified = attach_cause(error, NetworkCause::ConnectRefused);
            assert_eq!(
                cause_in_chain(&classified),
                Some(NetworkCause::ConnectRefused),
                "the cause must travel with the variant: {classified}"
            );
        }
    }

    /// #2784: the message names the cause, in Spanish, before the detail.
    #[test]
    fn the_message_names_the_cause_before_the_detail() {
        let error = attach_cause(
            DownloadError::dns("name resolution failed"),
            NetworkCause::DnsResolution,
        );
        assert_eq!(
            error.to_string(),
            "DNS error: no se pudo resolver el nombre del dominio: name resolution failed"
        );
    }
}
