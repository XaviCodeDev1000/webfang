//! The mappings that turn a transport failure into a typed one (#2784).
//!
//! They live in the domain because BOTH fetch clients need them and the
//! application layer must not import infrastructure: the mapping is a
//! contract, not an implementation detail.

use crate::domain::downloader_port::DownloadError;
use crate::domain::error::cause::{
    cause_in_chain, classify, CauseCarrier, ClassifiedNetworkError, DnsResolutionError,
    ErrorSource, FetchPhase, NetworkCause,
};
use crate::domain::error::resolution::refine_connect_cause;

/// The hints a caller can read off its own transport error (#2841).
///
/// `wreq::Error` answers only these three predicates and says NOTHING about
/// the phase, so the classification seam takes them as DATA. That is what
/// makes the mapping testable: `wreq::Error` has no public constructor, so a
/// test cannot fabricate one, but it CAN fabricate a chain and the hints that
/// go with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportHints {
    /// `wreq::Error::is_timeout()` — "related to a timeout", nothing more: it
    /// is true for a connect deadline AND for a read deadline.
    pub is_timeout: bool,
    /// `wreq::Error::is_connect()` — "related to connect". The connector
    /// covers DNS + TCP + TLS, so a TLS handshake failure answers true here
    /// and must NOT be read as "this is not TLS".
    pub is_connect: bool,
    /// `wreq::Error::is_builder()` — the request could not be BUILT: a
    /// construction failure, which `Request` is reserved for.
    pub is_builder: bool,
}

impl TransportHints {
    /// Read the hints off a `wreq` error.
    #[must_use]
    pub fn of_wreq(error: &wreq::Error) -> Self {
        Self {
            is_timeout: error.is_timeout(),
            is_connect: error.is_connect(),
            is_builder: error.is_builder(),
        }
    }
}

/// Which HTTP failure a transport failure IS, decided BEFORE the variant is
/// built (#2841).
///
/// The verdict — the variant, its `Display`, its `ErrorClass`, its exit code —
/// is exactly the decision the three inline tri-branches used to make. It is a
/// value now so that decision and the cause it carries travel together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportVerdict {
    /// A timeout. The phase decides `ConnectTimeout` over `ReadTimeout`.
    Timeout {
        /// The classified timeout cause.
        cause: NetworkCause,
    },
    /// A connection / connector failure.
    Connection {
        /// The classified cause, read out of the error's own chain.
        cause: NetworkCause,
    },
    /// A transport failure that is neither a timeout nor a connection failure.
    Transport {
        /// The classified cause (`Unknown` when the chain says nothing).
        cause: NetworkCause,
    },
    /// The request could not be built: a construction failure, never a
    /// transport one. No cause travels with it.
    RequestConstruction,
}

/// Classify a transport failure (#2841).
///
/// The ONE place the three mapping sites agree, so the classification cannot
/// drift between them. A timeout that also looks like a connect failure is a
/// CONNECT timeout; one that does not is a READ timeout; any other failure is
/// classified from its own chain, and a builder error is a construction
/// failure no matter what its chain says.
#[must_use]
pub fn classify_transport_failure(
    error: &(dyn std::error::Error + 'static),
    hints: TransportHints,
    phase: FetchPhase,
) -> TransportVerdict {
    if hints.is_builder {
        return TransportVerdict::RequestConstruction;
    }
    if hints.is_timeout {
        // `is_timeout()` is true for both deadlines, so the CONNECT hint is
        // what separates "the SYN never arrived" from "the server never
        // answered" — the same split `phase_cause` makes for the download
        // path.
        let phase = if hints.is_connect {
            FetchPhase::Connect
        } else {
            FetchPhase::Body
        };
        return TransportVerdict::Timeout {
            cause: phase_cause(phase),
        };
    }
    if hints.is_connect {
        TransportVerdict::Connection {
            cause: classify(error, phase),
        }
    } else {
        TransportVerdict::Transport {
            cause: classify(error, phase),
        }
    }
}

/// Build the HTTP failure for a verdict, attaching the cause (#2841).
///
/// Takes the original error as an owned source so a test can drive it with a
/// fabricated one: the verdict half is separately testable with a plain
/// `&dyn Error`, and `wreq::Error` cannot be constructed at all.
#[must_use]
pub fn build_http_failure(
    verdict: TransportVerdict,
    error: ErrorSource,
    host: Option<&str>,
    request_started: std::time::Instant,
    phase: FetchPhase,
) -> crate::domain::http_error::HttpError {
    use crate::domain::http_error::HttpError;
    match verdict {
        TransportVerdict::RequestConstruction => HttpError::Request(error.to_string()),
        TransportVerdict::Timeout { cause } => {
            HttpError::timeout_with_cause(Box::new(ClassifiedNetworkError::new(cause, error)))
        },
        TransportVerdict::Connection { cause } => {
            let cause = match host {
                Some(host) => {
                    refine_connect_cause(cause, host, request_started, phase == FetchPhase::Connect)
                },
                None => cause,
            };
            HttpError::Connection {
                message: compose(cause, &error.to_string()),
                source: Some(Box::new(ClassifiedNetworkError::new(cause, error))),
            }
        },
        TransportVerdict::Transport { cause } => HttpError::transport(
            compose(cause, &error.to_string()),
            Some(Box::new(ClassifiedNetworkError::new(cause, error))),
        ),
    }
}

/// Map a transport error to the HTTP failure it is (#2841).
///
/// The verdict does not move: a timeout is still a `Timeout`, a connect
/// failure is still a `Connection`, a construction failure is still a
/// `Request`. The cause now travels with all three, through the ONE seam the
/// other call sites share. The `phase` is the caller's stage of the fetch —
/// `Connect` for a send, `Body` for a response-body read — so a timeout is
/// named by the stage that actually hit its deadline.
#[must_use]
pub fn http_failure(
    error: wreq::Error,
    phase: FetchPhase,
    host: Option<&str>,
    request_started: std::time::Instant,
) -> crate::domain::http_error::HttpError {
    build_http_failure(
        classify_transport_failure(&error, TransportHints::of_wreq(&error), phase),
        Box::new(error),
        host,
        request_started,
        phase,
    )
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
    use std::io;

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

    // =================================================================
    // #2841 — the classification seam the three mapping sites share
    // =================================================================

    /// The hints for a fabricated error: `wreq::Error` has no public
    /// constructor, so the seam takes them as data instead.
    fn hints(is_timeout: bool, is_connect: bool) -> TransportHints {
        TransportHints {
            is_timeout,
            is_connect,
            is_builder: false,
        }
    }

    #[test]
    fn a_timeout_that_looks_like_connect_is_a_connect_timeout() {
        let error = std::io::Error::new(io::ErrorKind::TimedOut, "deadline");
        assert_eq!(
            classify_transport_failure(&error, hints(true, true), FetchPhase::Connect),
            TransportVerdict::Timeout {
                cause: NetworkCause::ConnectTimeout
            },
        );
    }

    #[test]
    fn a_timeout_that_does_not_is_a_read_timeout() {
        let error = std::io::Error::new(io::ErrorKind::TimedOut, "deadline");
        assert_eq!(
            classify_transport_failure(&error, hints(true, false), FetchPhase::Connect),
            TransportVerdict::Timeout {
                cause: NetworkCause::ReadTimeout
            },
        );
    }

    #[test]
    fn a_builder_error_is_a_construction_failure() {
        // A URL/scheme the client refuses to build is NOT a transport failure,
        // whatever its chain says.
        let error = std::io::Error::other("builder error: invalid URL");
        assert_eq!(
            classify_transport_failure(
                &error,
                TransportHints {
                    is_timeout: false,
                    is_connect: false,
                    is_builder: true,
                },
                FetchPhase::Connect,
            ),
            TransportVerdict::RequestConstruction,
        );
    }

    /// #2784 + #2841: the value the taxonomy declared and never observed. The
    /// `InvalidData` kind in the connect phase IS what the classifier reads as
    /// `TlsHandshake` — reachable here without a network.
    #[test]
    fn a_tls_handshake_failure_is_classified_from_its_chain() {
        let error = std::io::Error::new(io::ErrorKind::InvalidData, "handshake failed");
        let reported_as_connect =
            classify_transport_failure(&error, hints(false, true), FetchPhase::Connect);
        assert_eq!(
            reported_as_connect,
            TransportVerdict::Connection {
                cause: NetworkCause::TlsHandshake
            },
        );
        // The SAME failure, reported as not-connect, is a Transport failure
        // that still carries the classified cause.
        assert_eq!(
            classify_transport_failure(&error, hints(false, false), FetchPhase::Connect),
            TransportVerdict::Transport {
                cause: NetworkCause::TlsHandshake
            },
        );
    }

    #[test]
    fn a_refused_connection_keeps_the_connection_verdict() {
        let error = std::io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        assert_eq!(
            classify_transport_failure(&error, hints(false, true), FetchPhase::Connect),
            TransportVerdict::Connection {
                cause: NetworkCause::ConnectRefused
            },
        );
    }

    #[test]
    fn an_unclassifiable_transport_failure_is_unknown_but_still_carried() {
        let error = std::io::Error::other("mysterious transport failure");
        assert_eq!(
            classify_transport_failure(&error, hints(false, false), FetchPhase::Connect),
            TransportVerdict::Transport {
                cause: NetworkCause::Unknown
            },
        );
    }

    /// The boundary pin for the acceptance criteria: the cause survives
    /// verdict → `HttpError` → `CrawlError` → `ScraperError` with no network
    /// and no `wreq::Error`, and neither the message nor the class moves.
    #[test]
    fn the_cause_reaches_the_boundary_through_the_conversion_chain() {
        let original: ErrorSource = Box::new(std::io::Error::new(io::ErrorKind::TimedOut, "read"));
        let http = build_http_failure(
            TransportVerdict::Timeout {
                cause: NetworkCause::ReadTimeout,
            },
            original,
            None,
            std::time::Instant::now(),
            FetchPhase::Connect,
        );
        assert_eq!(http.to_string(), "Request Timeout");
        let crawl: crate::domain::error::CrawlError = http.into();
        let scraper: crate::error::ScraperError = crawl.into();
        assert_eq!(
            cause_in_chain(&scraper),
            Some(NetworkCause::ReadTimeout),
            "the cause must reach the boundary: {scraper}"
        );
        assert_eq!(scraper.to_string(), "error de red: request timeout");
        assert_eq!(
            scraper.classify(),
            crate::error::ErrorClass::TransientRetriable,
            "the class must not move with the cause"
        );
    }

    /// #2841: the failure that used to be flattened into `Request` reaches the
    /// boundary with its cause instead.
    #[test]
    fn an_unclassified_transport_failure_is_no_longer_flattened_into_request() {
        let original: ErrorSource = Box::new(std::io::Error::other("tls alert"));
        let http = build_http_failure(
            TransportVerdict::Transport {
                cause: NetworkCause::TlsHandshake,
            },
            original,
            None,
            std::time::Instant::now(),
            FetchPhase::Connect,
        );
        assert!(
            matches!(
                &http,
                crate::domain::http_error::HttpError::Transport { .. }
            ),
            "expected the Transport variant, got: {http:?}"
        );
        assert_eq!(
            http.to_string(),
            "Transport Error: falló el handshake TLS: tls alert"
        );
        let scraper: crate::error::ScraperError =
            crate::domain::error::CrawlError::from(http).into();
        assert_eq!(cause_in_chain(&scraper), Some(NetworkCause::TlsHandshake));
        assert_eq!(
            scraper.classify(),
            crate::error::ErrorClass::TransientRetriable,
            "an unclassified transport failure is a transport failure, not a bug"
        );
    }

    /// #2841 (S12): `ConnectTimeout` observed end to end with an injected
    /// failure — the cause walks verdict → `HttpError` → `CrawlError` →
    /// `ScraperError`, and the class stays the timeout's own.
    #[test]
    fn a_connect_timeout_reaches_the_boundary_through_the_conversion_chain() {
        let original: ErrorSource = Box::new(std::io::Error::new(
            io::ErrorKind::TimedOut,
            "connect deadline",
        ));
        let http = build_http_failure(
            TransportVerdict::Timeout {
                cause: NetworkCause::ConnectTimeout,
            },
            original,
            None,
            std::time::Instant::now(),
            FetchPhase::Connect,
        );
        assert_eq!(http.to_string(), "Request Timeout");
        let scraper: crate::error::ScraperError =
            crate::domain::error::CrawlError::from(http).into();
        assert_eq!(
            cause_in_chain(&scraper),
            Some(NetworkCause::ConnectTimeout),
            "the connect timeout cause must reach the boundary: {scraper}"
        );
        assert_eq!(
            scraper.classify(),
            crate::error::ErrorClass::TransientRetriable,
            "the boundary's TimedOut row — the class does not move with the cause"
        );
    }

    /// #2841 (S12): `TlsHandshake` observed end to end with an injected
    /// failure — the connector reports a handshake failure in the CONNECT
    /// phase, and the classified cause survives to the boundary.
    #[test]
    fn a_tls_handshake_reaches_the_boundary_through_the_conversion_chain() {
        let original: ErrorSource =
            Box::new(std::io::Error::new(io::ErrorKind::InvalidData, "tls alert"));
        let http = build_http_failure(
            TransportVerdict::Connection {
                cause: NetworkCause::TlsHandshake,
            },
            original,
            None,
            std::time::Instant::now(),
            FetchPhase::Connect,
        );
        let scraper: crate::error::ScraperError =
            crate::domain::error::CrawlError::from(http).into();
        assert_eq!(
            cause_in_chain(&scraper),
            Some(NetworkCause::TlsHandshake),
            "the TLS handshake cause must reach the boundary: {scraper}"
        );
        assert_eq!(
            scraper.classify(),
            crate::error::ErrorClass::TransientRetriable,
        );
    }

    /// A construction failure keeps the exact `Request` surface it always had.
    #[test]
    fn a_construction_failure_keeps_the_request_surface() {
        let original: ErrorSource = Box::new(std::io::Error::other("builder error: bad scheme"));
        let http = build_http_failure(
            TransportVerdict::RequestConstruction,
            original,
            None,
            std::time::Instant::now(),
            FetchPhase::Connect,
        );
        assert_eq!(http.to_string(), "Request Error: builder error: bad scheme");
        assert!(std::error::Error::source(&http).is_none());
    }
}
