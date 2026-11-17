//! The answer-set lookup seam behind [`ValidatingResolver`] — extracted
//! verbatim from the `ssrf` parent (ADR-0021 Slice 1 headroom; pure move, no
//! behavior change).

/// Boxed error shared by the lookup seam and the scan: the same
/// `Box<dyn Error + Send + Sync>` wreq's `Resolving` hands back.
pub(super) type BoxedLookupError = Box<dyn std::error::Error + Send + Sync>;

/// Boxed future returned by [`NameSource::lookup`].
pub(super) type LookupFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<Vec<std::net::SocketAddr>, BoxedLookupError>>
            + Send,
    >,
>;

/// The answer-set lookup seam behind the validating resolver.
///
/// Production uses [`GaiNameSource`] — the same blocking `getaddrinfo`
/// (through `tokio::net::lookup_host`) that `gai_lookup` called inline before.
/// The trait exists so the wiring from `resolve` to `fail_closed_scan` can be
/// exercised by a test with a fixed answer set: a real LAN resolution is not
/// reproducible in CI (it depends on the machine's resolver/routing), and the
/// two halves — the lookup and the scan — were previously only tested in
/// isolation, so a bug in how `gai_lookup` joins them could not be observed.
///
/// It is deliberately NOT public API: the only non-test implementor is the
/// private default below, and the injection point is `#[cfg(test)]`.
pub(super) trait NameSource: Send + Sync + std::fmt::Debug {
    /// Resolve `host` to its answer set, without policy (the caller applies
    /// the validating resolver's `fail_closed_scan`).
    fn lookup(&self, host: String) -> LookupFuture;
}

/// The production [`NameSource`]: blocking `getaddrinfo` via
/// `tokio::net::lookup_host`, exactly the call `gai_lookup` used inline.
///
/// `tokio::net::lookup_host` resolves IP literals locally (no network, no
/// resolver daemon) and delegates real names to the system resolver.
#[derive(Debug)]
pub(super) struct GaiNameSource;

impl NameSource for GaiNameSource {
    fn lookup(&self, host: String) -> LookupFuture {
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            Ok(addrs)
        })
    }
}
