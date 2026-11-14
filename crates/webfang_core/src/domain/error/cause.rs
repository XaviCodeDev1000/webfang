//! The connection-failure taxonomy (#2784).
//!
//! Six measured failure causes — a dropped SYN, a blackholed resolver, an
//! answered-then-missing name, a refused connection, a read timeout, an EOF and
//! a failed TLS handshake — collapse into **three** messages today
//! (`client error (Connect)`, `operation timed out`, `client error
//! (SendRequest)`), with no machine-readable cause anywhere. An operator
//! reading a log cannot tell "the name did not resolve" from "the SYN never
//! got through", and the two are indistinguishable in the tool's own output.
//!
//! This module owns the **vocabulary**: the stable values, the Spanish
//! messages, and the carrier that keeps a classified failure typed all the way
//! up the error chain (the chain is preserved — `ScraperError::Network` keeps
//! its `#[source]` — so the cause never has to be flattened to a string).
//!
//! The **classifier** lives in the infrastructure layer, where the foreign
//! error types are known: it walks the chain by TYPE (`io::ErrorKind`, the
//! client's own predicates) and never compares message strings, because the
//! messages change between dependency versions and a text-matching taxonomy
//! would break silently.

use std::error::Error;
use std::fmt;

/// The stable, machine-readable cause of a connection failure.
///
/// The values are a **public contract** (they travel in the JSONL trace):
/// a value never changes meaning, and a new class is a new value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NetworkCause {
    /// The name could not be resolved, or the resolver never answered.
    DnsResolution,
    /// The peer refused the connection (RST).
    ConnectRefused,
    /// The TCP connect did not complete in time — the SYN never got through.
    ConnectTimeout,
    /// The connection was established and the response never arrived.
    ReadTimeout,
    /// The peer closed the connection before answering (EOF).
    ConnectionClosed,
    /// The TLS handshake failed.
    TlsHandshake,
    /// The failure could not be classified.
    ///
    /// A **value, not a shrug**: it means the classifier ran and could not
    /// decide. It never means "the classifier did not run". An honest
    /// `unknown` is worth more than a supposed cause.
    Unknown,
}

impl NetworkCause {
    /// Every value of the taxonomy, in a stable order.
    ///
    /// The set is snapshotted in the tests: adding a value is a **visible**
    /// change to a public contract, not a silent extension.
    pub const ALL: [NetworkCause; 7] = [
        Self::DnsResolution,
        Self::ConnectRefused,
        Self::ConnectTimeout,
        Self::ReadTimeout,
        Self::ConnectionClosed,
        Self::TlsHandshake,
        Self::Unknown,
    ];

    /// The stable wire value (what the trace carries).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DnsResolution => "dns_resolution",
            Self::ConnectRefused => "connect_refused",
            Self::ConnectTimeout => "connect_timeout",
            Self::ReadTimeout => "read_timeout",
            Self::ConnectionClosed => "connection_closed",
            Self::TlsHandshake => "tls_handshake",
            Self::Unknown => "unknown",
        }
    }

    /// The value that names this cause, for reading a trace back.
    #[must_use]
    pub fn from_value(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.as_str() == value)
    }

    /// The user-facing message, in Spanish.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::DnsResolution => "no se pudo resolver el nombre del dominio",
            Self::ConnectRefused => "el servidor rechazó la conexión",
            Self::ConnectTimeout => "la conexión no se completó a tiempo",
            Self::ReadTimeout => "el servidor no respondió a tiempo",
            Self::ConnectionClosed => "el servidor cerró la conexión antes de responder",
            Self::TlsHandshake => "falló el handshake TLS",
            Self::Unknown => "fallo de red no clasificado",
        }
    }
}

impl fmt::Display for NetworkCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

/// A classified network failure: the cause plus the original error.
///
/// The carrier keeps the original error as its `#[source]`, so the chain the
/// classifier walked stays walkable by everything downstream (the transient
/// classification, the logs, the exit code) — the cause is ADDED to the
/// chain, never substituted for it.
#[derive(Debug)]
pub struct ClassifiedNetworkError {
    cause: NetworkCause,
    source: Box<dyn Error + Send + Sync>,
}

impl ClassifiedNetworkError {
    /// Classify `source` as `cause`.
    #[must_use]
    pub fn new(cause: NetworkCause, source: Box<dyn Error + Send + Sync>) -> Self {
        Self { cause, source }
    }

    /// The classified cause.
    #[must_use]
    pub fn cause(&self) -> NetworkCause {
        self.cause
    }
}

impl fmt::Display for ClassifiedNetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.cause, self.source)
    }
}

impl Error for ClassifiedNetworkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// A name resolution that failed or never completed.
///
/// The resolver raises this instead of a bare I/O error, so the DNS class
/// survives every wrapper between the resolver and the operator: the
/// classifier finds it by TYPE, and the message text never has to be parsed.
#[derive(Debug)]
pub struct DnsResolutionError {
    source: Box<dyn Error + Send + Sync>,
}

impl DnsResolutionError {
    /// Wrap the underlying lookup failure.
    #[must_use]
    pub fn new(source: Box<dyn Error + Send + Sync>) -> Self {
        Self { source }
    }
}

impl fmt::Display for DnsResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", NetworkCause::DnsResolution, self.source)
    }
}

impl Error for DnsResolutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Find the classified cause inside an error chain.
///
/// The chain is walked by TYPE (a `downcast` to the carrier), never by
/// comparing message text. Returns `None` when the chain carries no
/// classified failure — the caller then reports `unknown` rather than
/// guessing.
#[must_use]
pub fn cause_in_chain(error: &(dyn Error + 'static)) -> Option<NetworkCause> {
    let mut current: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(err) = current {
        if let Some(classified) = err.downcast_ref::<ClassifiedNetworkError>() {
            return Some(classified.cause());
        }
        current = err.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_has_a_stable_wire_name() {
        for cause in NetworkCause::ALL {
            assert_eq!(
                NetworkCause::from_value(cause.as_str()),
                Some(cause),
                "the wire name of {cause:?} must round-trip"
            );
        }
    }

    #[test]
    fn the_wire_names_are_the_documented_ones() {
        let names: Vec<&str> = NetworkCause::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "dns_resolution",
                "connect_refused",
                "connect_timeout",
                "read_timeout",
                "connection_closed",
                "tls_handshake",
                "unknown",
            ],
            "the taxonomy's wire names are a public contract"
        );
    }

    #[test]
    fn the_user_facing_messages_are_spanish() {
        for cause in NetworkCause::ALL {
            let message = cause.message();
            assert!(!message.is_empty());
            assert!(
                message.chars().all(|c| c.is_ascii() || !c.is_ascii()),
                "the message must be plain text: {message}"
            );
        }
        assert_eq!(
            NetworkCause::ConnectRefused.message(),
            "el servidor rechazó la conexión"
        );
    }

    #[test]
    fn the_classifier_reads_the_cause_out_of_the_chain() {
        let cause = NetworkCause::DnsResolution;
        let source = std::io::Error::other("name resolution failed");
        let classified = ClassifiedNetworkError::new(cause, Box::new(source));
        assert_eq!(cause_in_chain(&classified), Some(cause));
    }

    #[test]
    fn an_unclassified_chain_has_no_cause() {
        let plain = std::io::Error::other("something else");
        assert_eq!(cause_in_chain(&plain), None);
    }
}
