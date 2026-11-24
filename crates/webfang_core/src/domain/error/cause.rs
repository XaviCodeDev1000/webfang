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
use std::sync::Arc;

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

    /// Whether this cause is a NAME RESOLUTION failure.
    ///
    /// A resolution failure is OURS: the site never saw the request, so
    /// waiting longer cannot help and the peer is not refusing us (#2823).
    /// Every other cause is the site's answer (or the network path to it),
    /// which is what the exponential backoff is for.
    #[must_use]
    pub const fn is_resolution(self) -> bool {
        matches!(self, Self::DnsResolution)
    }

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

    /// The user-facing message.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::DnsResolution => "could not resolve the domain name",
            Self::ConnectRefused => "the server refused the connection",
            Self::ConnectTimeout => "the connection did not complete in time",
            Self::ReadTimeout => "the server did not respond in time",
            Self::ConnectionClosed => "the server closed the connection before responding",
            Self::TlsHandshake => "the TLS handshake failed",
            Self::Unknown => "unclassified network failure",
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
///
/// The carrier is **clonable** (#2824): the original error is shared behind an
/// `Arc`, so a copy keeps the cause AND the chain behind it. It could not be
/// cloned before (the error was boxed), and the copy was rebuilt from the
/// rendered message — which silently dropped the cause the transport had
/// already classified.
#[derive(Clone, Debug)]
pub struct ClassifiedNetworkError {
    cause: NetworkCause,
    source: Arc<dyn Error + Send + Sync>,
}

impl ClassifiedNetworkError {
    /// Classify `source` as `cause`.
    #[must_use]
    pub fn new(cause: NetworkCause, source: Box<dyn Error + Send + Sync>) -> Self {
        Self {
            cause,
            source: Arc::from(source),
        }
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
        let inner: &(dyn Error + 'static) = self.source.as_ref();
        Some(inner)
    }
}

/// The carrier an `io::Error` carries in its payload (#2824).
///
/// `std::io::Error` does NOT publish its payload as a source: `source()`
/// returns the payload's OWN source. A classified cause travelling in the
/// payload is therefore invisible to a plain `source()` walk — reaching it
/// needs the `get_ref()` special case, and a consumer outside this crate
/// (the trace, `CrawlErrorCategory`, an external reader) does not know that
/// quirk: it only sees the cause rendered in the `Display`.
///
/// This type closes the gap. The payload's `source()` IS the classified
/// error, so the plain walk reaches the typed cause. It is clonable, so the
/// copy of an `Io` failure keeps it — the kind stays untouched either way.
#[derive(Clone, Debug)]
pub struct CauseCarrier {
    classified: ClassifiedNetworkError,
}

impl CauseCarrier {
    /// Carry `classified` as the payload of an `io::Error`.
    #[must_use]
    pub fn new(classified: ClassifiedNetworkError) -> Self {
        Self { classified }
    }

    /// The classified cause.
    #[must_use]
    pub fn cause(&self) -> NetworkCause {
        self.classified.cause()
    }
}

impl fmt::Display for CauseCarrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.classified)
    }
}

impl Error for CauseCarrier {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.classified)
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
        // returns the payload's OWN source, so the cause living in the payload
        // is invisible to this walk. `get_ref()` is where the payload actually
        // lives (#2784). The plain walk reaches the cause through the
        // `CauseCarrier` (#2824); this hop covers a payload that carries the
        // carrier directly.
        if let Some(io_err) = err.downcast_ref::<io::Error>() {
            if let Some(cause) = io_err.get_ref().and_then(classified_cause) {
                return Some(cause);
            }
        }
        current = err.source();
    }
    None
}

/// The cause a carrier payload carries, if it carries one.
fn classified_cause(payload: &(dyn Error + Send + Sync + 'static)) -> Option<NetworkCause> {
    if let Some(classified) = payload.downcast_ref::<ClassifiedNetworkError>() {
        return Some(classified.cause());
    }
    payload
        .downcast_ref::<CauseCarrier>()
        .map(CauseCarrier::cause)
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
mod classify_tests;
#[cfg(test)]
mod tests;
