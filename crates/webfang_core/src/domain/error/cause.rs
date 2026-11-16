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
//! The **classifier** lives here too, next to the vocabulary, because both
//! fetch clients need it and the application layer must not import
//! infrastructure (inward-only layering): the application HTTP client classifies
//! at its own send boundary, and so does the infrastructure downloader. It walks
//! the chain by TYPE (`io::ErrorKind`, the domain's own typed errors) and never
//! compares message strings, because the messages change between dependency
//! versions and a text-matching taxonomy would break silently.

use std::error::Error;
use std::fmt;
use std::io;

/// A preserved transport error — the original failure, carried opaquely.
///
/// The alias exists so the contract that carries the cause reads the same
/// everywhere it appears (`Option<ErrorSource>`), instead of repeating the
/// `Box<dyn Error + Send + Sync>` spelling at every boundary.
pub type ErrorSource = Box<dyn Error + Send + Sync>;

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
    ///
    /// NOTE: declared, not yet observed end to end — the transport does not
    /// expose a readable kind for a stalled response, so it arrives as
    /// [`NetworkCause::Unknown`] today. The name and its meaning are already
    /// fixed (#2784).
    ReadTimeout,
    /// The peer closed the connection before answering (EOF).
    ///
    /// NOTE: declared, not yet observed end to end — same reason as
    /// [`NetworkCause::ReadTimeout`]: the transport erases the kind before
    /// the classifier can read it (#2784).
    ConnectionClosed,
    /// The TLS handshake failed.
    ///
    /// NOTE: declared, not yet observed end to end — the handshake violation
    /// is inferred from an invalid-data kind, which no measured failure has
    /// produced yet (#2784).
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

    /// The metric bucket this cause belongs to.
    ///
    /// The cause vocabulary and `CrawlErrorCategory`
    /// were two PARALLEL
    /// taxonomies: one said `Network`, the other said `dns_resolution`, and
    /// nothing pinned them together. A dashboard counting the category and a
    /// trace counting the cause could disagree, with no test to catch it
    /// (#2784). This is the ONE place the two meet: every cause is a network
    /// failure, so they all fold into the same bucket, and
    /// `CrawlErrorCategory::from(&CrawlError)` routes through here instead of
    /// deciding on its own.
    #[must_use]
    pub const fn category(self) -> crate::domain::error::crawl_error_category::CrawlErrorCategory {
        crate::domain::error::crawl_error_category::CrawlErrorCategory::Network
    }

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
        // An I/O error does NOT expose its payload as a source: `source()`
        // returns the payload's OWN source, so a carrier travelling in the
        // payload is invisible to this walk. `get_ref()` is where the payload
        // actually lives (#2784). The carrier's own source stays reachable
        // through the normal walk, so the chain loses nothing.
        if let Some(io_err) = err.downcast_ref::<io::Error>() {
            if let Some(classified) = io_err
                .get_ref()
                .and_then(|payload| payload.downcast_ref::<ClassifiedNetworkError>())
            {
                return Some(classified.cause());
            }
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
        let messages: Vec<&str> = NetworkCause::ALL
            .iter()
            .map(|cause| cause.message())
            .collect();
        assert!(messages.iter().all(|message| !message.is_empty()));
        // The issue's first acceptance criterion, pinned here: two causes must
        // never produce the same sentence, or the message stops being evidence.
        let unique: std::collections::BTreeSet<&str> = messages.iter().copied().collect();
        assert_eq!(
            unique.len(),
            messages.len(),
            "two causes must never share a message: {messages:?}"
        );
        assert_eq!(
            NetworkCause::ConnectRefused.message(),
            "el servidor rechazó la conexión"
        );
    }

    /// #2784: a carrier travelling in an I/O error's PAYLOAD is still found.
    ///
    /// An `io::Error` does not expose its payload as a source: `source()`
    /// returns the payload's own source. The carrier is therefore invisible to
    /// a plain chain walk, and the walk has to look into `get_ref()`. The
    /// `Io` variant relies on this to keep the error's `kind` — the retry
    /// rule's input — so the mechanism is pinned here.
    #[test]
    fn a_carrier_in_an_io_errors_payload_is_still_found() {
        let carrier = ClassifiedNetworkError::new(
            NetworkCause::ConnectRefused,
            Box::new(io::Error::other("connection refused")),
        );
        let error = crate::domain::downloader_port::DownloadError::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            carrier,
        ));

        assert_eq!(
            cause_in_chain(&error),
            Some(NetworkCause::ConnectRefused),
            "the carrier travels in the I/O error's payload"
        );
        assert!(
            error
                .to_string()
                .contains("el servidor rechazó la conexión"),
            "the message must name the cause: {error}"
        );
        assert_eq!(
            error.classify(),
            crate::domain::error::ErrorClass::InternalFatal,
            "the kind survives the carrier: a refused connection is not retried"
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

/// Which phase of a fetch a failure happened in.
///
/// The phase is known at the place the error happened, so the caller passes it
/// in — it is never guessed from a message.
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
mod classify_tests {
    use super::*;

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
    fn the_phase_decides_a_timeout() {
        let err = std::io::Error::new(io::ErrorKind::TimedOut, "timed out");
        assert_eq!(
            classify(&err, FetchPhase::Connect),
            NetworkCause::ConnectTimeout
        );
        assert_eq!(classify(&err, FetchPhase::Body), NetworkCause::ReadTimeout);
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
    fn an_unreadable_chain_is_unknown() {
        let err = std::io::Error::other("something else entirely");
        assert_eq!(classify(&err, FetchPhase::Connect), NetworkCause::Unknown);
    }

    #[test]
    fn a_plain_error_without_io_is_unknown() {
        #[derive(Debug)]
        struct Plain;
        impl fmt::Display for Plain {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("plain")
            }
        }
        impl Error for Plain {}

        assert_eq!(classify(&Plain, FetchPhase::Connect), NetworkCause::Unknown);
    }
}
