//! The connection-failure classifier (#2784).
//!
//! The classifier walks the error chain **by type** — an `io::Error`'s
//! `ErrorKind`, the domain's own typed errors — and never compares message
//! strings. The messages change between dependency versions, so a
//! text-matching taxonomy would break silently and the breakage would look
//! like a network change.
//!
//! The phase matters as much as the kind: a timeout while the name is still
//! being resolved is a **resolution** failure, and a timeout while the TCP
//! connect is in flight is a **connect** failure. The phase is known at the
//! place the error happened, so the caller passes it in — it is never guessed
//! from the message.

use std::error::Error;
use std::io;

use crate::domain::error::{DnsResolutionError, NetworkCause};

/// Which phase of a fetch a failure happened in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchPhase {
    /// Before the response existed: the name, the TCP connect, the TLS
    /// handshake.
    Connect,
    /// After the response started: the body.
    Body,
}

/// Classify a failure by TYPE.
///
/// `Unknown` is the honest answer whenever the chain does not decide: an
/// unknown `unknown` beats a supposed cause.
#[must_use]
pub fn classify(error: &(dyn Error + 'static), phase: FetchPhase) -> NetworkCause {
    // The resolver's own typed failure decides the DNS class on its own: it
    // exists only because a resolution was attempted and did not succeed.
    if has_in_chain::<DnsResolutionError>(error) {
        return NetworkCause::DnsResolution;
    }

    let Some(kind) = io_kind_in_chain(error) else {
        return NetworkCause::Unknown;
    };

    match kind {
        io::ErrorKind::ConnectionRefused => NetworkCause::ConnectRefused,
        io::ErrorKind::ConnectionReset
        | io::ErrorKind::UnexpectedEof
        | io::ErrorKind::BrokenPipe => NetworkCause::ConnectionClosed,
        io::ErrorKind::TimedOut => match phase {
            FetchPhase::Connect => NetworkCause::ConnectTimeout,
            FetchPhase::Body => NetworkCause::ReadTimeout,
        },
        // The TLS stack reports a handshake violation as invalid data, and a
        // handshake only happens in the connect phase.
        io::ErrorKind::InvalidData => match phase {
            FetchPhase::Connect => NetworkCause::TlsHandshake,
            FetchPhase::Body => NetworkCause::Unknown,
        },
        _ => NetworkCause::Unknown,
    }
}

/// The `io::ErrorKind` anywhere in the chain, if there is one.
fn io_kind_in_chain(error: &(dyn Error + 'static)) -> Option<io::ErrorKind> {
    let mut current: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(err) = current {
        if let Some(io_err) = err.downcast_ref::<io::Error>() {
            return Some(io_err.kind());
        }
        current = err.source();
    }
    None
}

/// Whether the chain contains an error of type `T`.
fn has_in_chain<T: Error + 'static>(error: &(dyn Error + 'static)) -> bool {
    let mut current: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(err) = current {
        if err.downcast_ref::<T>().is_some() {
            return true;
        }
        current = err.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::ClassifiedNetworkError;

    #[test]
    fn a_refused_connection_is_classified_by_its_io_kind() {
        let err = std::io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        assert_eq!(
            classify(&err, FetchPhase::Connect),
            NetworkCause::ConnectRefused
        );
    }

    #[test]
    fn a_reset_or_an_eof_is_a_closed_connection() {
        for kind in [
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::BrokenPipe,
        ] {
            let err = std::io::Error::new(kind, "closed");
            assert_eq!(
                classify(&err, FetchPhase::Connect),
                NetworkCause::ConnectionClosed,
                "kind {kind:?} must classify as a closed connection"
            );
        }
    }

    #[test]
    fn the_phase_decides_which_timeout_it_is() {
        let err = std::io::Error::new(io::ErrorKind::TimedOut, "timed out");
        assert_eq!(
            classify(&err, FetchPhase::Connect),
            NetworkCause::ConnectTimeout
        );
        assert_eq!(classify(&err, FetchPhase::Body), NetworkCause::ReadTimeout);
    }

    #[test]
    fn a_handshake_failure_is_invalid_data_in_the_connect_phase() {
        let err = std::io::Error::new(io::ErrorKind::InvalidData, "handshake");
        assert_eq!(
            classify(&err, FetchPhase::Connect),
            NetworkCause::TlsHandshake
        );
        assert_eq!(classify(&err, FetchPhase::Body), NetworkCause::Unknown);
    }

    #[test]
    fn the_resolvers_own_failure_decides_the_dns_class() {
        let err = DnsResolutionError::new(Box::new(std::io::Error::other("no such host")));
        assert_eq!(
            classify(&err, FetchPhase::Connect),
            NetworkCause::DnsResolution
        );
    }

    #[test]
    fn an_undecidable_failure_is_unknown_not_a_guess() {
        let err = std::io::Error::other("something entirely new");
        assert_eq!(classify(&err, FetchPhase::Connect), NetworkCause::Unknown);
    }

    #[test]
    fn the_classification_survives_a_wrapper_that_hides_the_type() {
        #[derive(Debug)]
        struct Wrapper(Box<dyn Error + Send + Sync>);
        impl std::fmt::Display for Wrapper {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("wrapped")
            }
        }
        impl Error for Wrapper {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(self.0.as_ref())
            }
        }

        let inner = std::io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        let wrapped = Wrapper(Box::new(inner));
        assert_eq!(
            classify(&wrapped, FetchPhase::Connect),
            NetworkCause::ConnectRefused,
            "the chain walk must reach the io leaf through the wrapper"
        );
    }

    #[test]
    fn a_classified_error_keeps_its_cause_through_the_chain() {
        let cause = NetworkCause::ConnectRefused;
        let inner = std::io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        let classified = ClassifiedNetworkError::new(cause, Box::new(inner));
        assert_eq!(
            crate::domain::error::cause_in_chain(&classified),
            Some(cause)
        );
    }
}
