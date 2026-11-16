//! The classification seam the three mapping sites share (#2841).
//!
//! Split out of `mapping.rs` (file size ratchet): the verdict types, the one
//! classification function and its boundary tests are one cohesive unit —
//! the wrapper `http_failure` and the sibling download mappings stay in the
//! parent and call into this seam.

use super::{compose, phase_cause};
use crate::domain::error::cause::{
    classify, ClassifiedNetworkError, ErrorSource, FetchPhase, NetworkCause,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::cause::cause_in_chain;
    use std::io;

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
