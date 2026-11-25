//! Connect-time SSRF enforcement: the [`ValidatingResolver`] DNS guard and the
//! [`SsrfGuard`](crate::domain::ssrf_guard::SsrfGuard) trait impl.
//!
//! Pure policy (the deny-list functions and `redirect_policy`) lives in
//! `crate::domain::ssrf_guard`. This module owns everything that performs real
//! I/O and keeps only the `pub(crate)` re-exports its own bodies consume —
//! production callers never reach policy through this path: every HTTP client
//! obtains both layers from one place, the domain port
//! [`ssrf_guard()`](crate::domain::ssrf_guard::ssrf_guard) /
//! [`SsrfGuard::secure_client`](crate::domain::ssrf_guard::SsrfGuard).
//!
//! 1. **Entry validation** — the shared literal-IP guard
//!    ([`reject_forbidden_literal_url`](crate::domain::ssrf_guard::reject_forbidden_literal_url))
//!    rejects forbidden IP literals in every encoding before any request
//!    leaves, on both the CLI request path and the MCP entry validator
//!    (`validate_url_no_ssrf`, which additionally resolves hostnames). This
//!    stays as fast-fail typed UX.
//! 2. **Connect-time enforcement** — [`ValidatingResolver`] is installed via
//!    `wreq::ClientBuilder::dns_resolver` on every scrape client and re-checks
//!    every DNS answer against the shared deny list. Because redirects are
//!    followed inside the same client stack, each redirect hop's connection
//!    also resolves through this resolver, closing the gap where a hostname
//!    redirect target could reach an address that was never validated at
//!    entry (DNS rebinding / TOCTOU included).
//! 3. **Belt-and-suspenders literal guard** —
//!    [`redirect_policy`] still
//!    stops redirects whose target is a *literal* forbidden IP synchronously,
//!    before any resolution happens.
//!
//! Both layers 2 and 3 are applied together, in a single choke point, by
//! `impl SsrfGuard for DefaultSsrfGuard` below — the domain-owned
//! `DefaultSsrfGuard` type cannot reference infrastructure I/O, so the impl
//! lives here (infrastructure → domain is the allowed direction). No production
//! call site may hand-wire either layer: `secure_client` and its parameterized
//! sibling `secure_client_with_loopback` are the only places the redirect
//! policy and the validating resolver are composed.
//!
//! Layer boundaries verified against wreq 6.0.0-rc.29: its HTTP connector
//! parses IP-literal hosts directly (`dns::SocketAddrs::try_parse`) and never
//! invokes a custom resolver for them (`src/conn/http.rs`, `HttpConnector::call`),
//! so [`ValidatingResolver`] only sees *hostname* connections — precisely the
//! gap left open by layers 1 and 3, with no overlap and no hole. The same
//! boundary is why an IP literal needs the ENTRY gate to become policy-aware
//! (B2 slice 2): a literal is invisible here, so an allowlisted LAN literal is
//! reachable only if [`crate::domain::ssrf_guard::is_forbidden_literal_host_with_policy`]
//! lets it through upstream.
//!
//! Per-client policy (B2 slice 2, #1947): besides `allow_loopback`, a
//! resolver can carry this provider's [`NetworkPolicy`] allowlist, cloned
//! once at construction. Its `allow_hosts` half is enforced ONLY in
//! `ValidatingResolver::fail_closed_scan` — the sole place that knows both
//! the requested name and the resolved answer set — as an AND with
//! `allow_cidrs`.
//!
//! All layers share
//! [`is_forbidden_ip`](crate::domain::ssrf_guard::is_forbidden_ip) as the
//! single deny list.

// Re-exports of the pure policy this module's own bodies consume. The canonical
// home of that surface is `crate::domain::ssrf_guard` (ADR-0012 sub-slice 3.C);
// everything outside this module imports it from there, so nothing here is
// `pub`. `DISABLE_REDIRECT_GUARD_ENV` stays `pub(crate)` because crate tests in
// other modules set it before building clients; the rest have in-module
// consumers only.
use crate::domain::network_policy::NetworkPolicy;
use crate::domain::proxy_config::ClientSecurityProfile;
use crate::domain::ssrf_guard::{
    is_forbidden_ip_with, is_forbidden_resolved_address, redirect_policy,
    redirect_policy_with_loopback, redirect_policy_with_policy,
    validating_resolver_disabled_by_env,
};
#[cfg(test)]
pub(crate) use crate::domain::ssrf_guard::{
    DISABLE_REDIRECT_GUARD_ENV, DISABLE_VALIDATING_RESOLVER_ENV,
};

use std::net::IpAddr;
use std::sync::Arc;
use wreq::dns::{Addrs, Name, Resolve, Resolving};

mod resolution_errors;
pub use resolution_errors::{ForbiddenHostNotAllowlistedError, ForbiddenResolutionError};

/// Connect-time SSRF-enforcing DNS resolver installed on every scrape client.
///
/// Implements [`wreq::dns::Resolve`] using the same mechanism wreq uses by
/// default when hickory-dns is disabled — the GAI path: blocking
/// `getaddrinfo` through [`tokio::net::lookup_host`]. Every address in the
/// answer set is validated with
/// [`is_forbidden_ip`](crate::domain::ssrf_guard::is_forbidden_ip); if ANY
/// address is forbidden, the entire resolution fails (fail-closed, no
/// safe-subset filtering).
///
/// Because wreq follows redirects inside the same client stack
/// (`FollowRedirectLayer`), every redirect hop's connection resolves through
/// this resolver too — one choke point closes both owner-approved gaps:
/// hostname redirect targets and DNS-rebinding TOCTOU between entry
/// validation and connect.
///
/// The port in each resolved `SocketAddr` is `0`: per the `wreq::dns::Resolve`
/// contract, an explicit port in the request URI overrides it, and port 0 is
/// replaced with the scheme's conventional port otherwise. This mirrors
/// wreq's own `GaiResolver`, which resolves `(name, 0)`.
///
/// The test-only escape hatch is captured **once, at construction time** in
/// unit-test builds (stored as a plain `bool`); non-test builds always
/// validate (S9). Long-lived clients keep a consistent policy for their whole
/// lifetime (no half-disarmed states where some pooled connections validate
/// and others don't), and there is no per-request env syscall on the resolve
/// hot path.
///
/// Note: wreq short-circuits IP-literal hosts before calling any custom
/// resolver, so this type only ever sees hostname connections; literal-IP
/// targets are covered by entry validation and
/// [`redirect_policy`].
///
/// The per-client loopback permit (#1462) travels as a plain `bool` captured
/// at construction alongside the escape hatch: a resolver built with
/// [`ValidatingResolver::with_allow_loopback`] permits answers that resolve
/// to exactly `127.0.0.1` / `::1`, every other forbidden range stays denied.
/// The B2 allowlist ([`ValidatingResolver::with_policy`]) travels the same
/// way, per client and never through the process registry.
///
/// The `name_source` field is the lookup seam: production always holds the
/// private `GaiNameSource`, and only the `#[cfg(test)]` construction can hold
/// a fixed answer set, so the `resolve` → `fail_closed_scan` join is testable
/// without a real LAN.
#[derive(Debug, Clone)]
pub struct ValidatingResolver {
    validation_enabled: bool,
    allow_loopback: bool,
    /// Per-client B2 allowlist (slice 2, #1947). Cloned ONCE here, at
    /// client-build time, and shared behind an `Arc` with each in-flight
    /// resolution — the resolve path hands the `Arc` to the boxed future
    /// (which must be `'static`), so a connection costs a refcount bump and
    /// never a deep clone. Still per-client state, never process state.
    policy: Arc<NetworkPolicy>,
    /// Answer-set lookup seam. `Arc` because the boxed resolve future must be
    /// `'static`; in production this is always [`GaiNameSource`].
    name_source: Arc<dyn NameSource>,
}

mod name_source;

use self::name_source::{BoxedLookupError, GaiNameSource, NameSource};

impl ValidatingResolver {
    /// Builds a resolver with validation always active in non-test builds.
    /// Unit tests capture the exact `"1"` hatch value here; any other value
    /// (including `"0"`, `"true"`, `"yes"`) keeps validation active.
    #[must_use]
    pub fn new() -> Self {
        Self {
            validation_enabled: !validating_resolver_disabled_by_env(),
            allow_loopback: false,
            policy: Arc::new(NetworkPolicy::default()),
            name_source: Arc::new(GaiNameSource),
        }
    }

    /// Builds a resolver with the per-client loopback permit (#1462).
    ///
    /// The test-only hatch is still read once at construction in unit-test
    /// builds (same semantics as [`new`](Self::new)); `allow_loopback`
    /// additionally permits answers
    /// resolving to exactly `127.0.0.1` / `::1` for self-hosted endpoints.
    /// The flag lives on this instance — never in the process registry.
    #[must_use]
    pub fn with_allow_loopback(allow_loopback: bool) -> Self {
        Self {
            validation_enabled: !validating_resolver_disabled_by_env(),
            allow_loopback,
            policy: Arc::new(NetworkPolicy::default()),
            name_source: Arc::new(GaiNameSource),
        }
    }

    /// Builds a resolver with the loopback permit AND this provider's B2
    /// network allowlist (slice 2, #1947).
    ///
    /// Same lifecycle as its siblings: the test-only hatch is read once at
    /// construction, and both parameters are copied into THIS resolver —
    /// per-client state, never registry state, so two providers with
    /// different policies in one process cannot influence each other.
    ///
    /// The policy is cloned exactly once, here; `gai_lookup` takes it by
    /// reference on every resolution.
    #[must_use]
    pub fn with_policy(allow_loopback: bool, policy: &NetworkPolicy) -> Self {
        Self {
            validation_enabled: !validating_resolver_disabled_by_env(),
            allow_loopback,
            policy: Arc::new(policy.clone()),
            name_source: Arc::new(GaiNameSource),
        }
    }

    /// Resolves `host` through the injected [`NameSource`] (production: the
    /// GAI path, `getaddrinfo`), optionally validating every address in the
    /// answer set.
    ///
    /// Taking the source by value behind an `Arc` is what lets a test drive a
    /// fixed answer set through this exact function — and therefore exercise
    /// the join to [`Self::fail_closed_scan`] that the isolated unit tests
    /// cannot reach.
    ///
    /// # Errors
    ///
    /// Returns [`ForbiddenResolutionError`] when validating and any address
    /// in the answer set is forbidden, [`ForbiddenHostNotAllowlistedError`]
    /// when the address is allowlisted but the hostname is not declared, or
    /// the underlying `io::Error` if resolution itself fails.
    async fn gai_lookup(
        host: String,
        validate: bool,
        allow_loopback: bool,
        policy: Arc<NetworkPolicy>,
        name_source: Arc<dyn NameSource>,
    ) -> Result<Addrs, BoxedLookupError> {
        // #2784: the resolution is MARKED while it is in flight, so a failure
        // that arrives before it finishes is a RESOLUTION failure.
        let mut mark = crate::domain::error::resolution::resolution_tracker().begin(&host);
        let addrs = match name_source.lookup(host.clone()).await {
            Ok(addrs) => {
                // The name came back: record it, so a CONCURRENT request whose
                // own resolution hangs is not mistaken for this one's (#2784).
                mark.finished(true);
                addrs
            },
            Err(err) => {
                mark.finished(false);
                return Err(
                    crate::domain::error::mapping::dns_resolution_failure(err) as BoxedLookupError
                );
            },
        };

        if validate {
            Self::fail_closed_scan(&host, &addrs, allow_loopback, &policy)?;
        }

        Ok(Box::new(addrs.into_iter()) as Addrs)
    }

    /// Fail-closed answer-set scan: ANY forbidden record rejects the whole
    /// resolution (no safe-subset filtering), and an EMPTY answer set is an
    /// error too — an empty set can never yield a connectable address, so it
    /// must not masquerade as success.
    ///
    /// Pure function over the answer slice so the empty-set edge is unit-
    /// testable without depending on nondeterministic `getaddrinfo` behavior.
    /// `allow_loopback` narrows the deny list to the exact loopback addresses
    /// (#1462); `policy` adds the B2 allowlist, whose hostname half is
    /// enforced HERE and nowhere else — this is the only place in the system
    /// that knows the requested name and the resolved addresses at the same
    /// time.
    fn fail_closed_scan(
        host: &str,
        addrs: &[std::net::SocketAddr],
        allow_loopback: bool,
        policy: &NetworkPolicy,
    ) -> Result<(), BoxedLookupError> {
        if addrs.is_empty() {
            tracing::warn!(host = %host, "Empty DNS answer rejected (SSRF validating resolver)");
            return Err(Box::new(std::io::Error::other(format!(
                "empty DNS answer for '{host}'"
            ))));
        }
        let Some(offending) = addrs
            .iter()
            .find(|addr| is_forbidden_resolved_address(&addr.ip(), host, allow_loopback, policy))
        else {
            return Ok(());
        };
        let ip = offending.ip();
        // Two denial classes, told apart by WHAT is missing. The address may
        // well be allowlisted — what is missing is the declared hostname, a
        // config omission with an obvious fix, which is exactly why it gets
        // its own message naming `allow_hosts`.
        if is_missing_allow_hosts_declaration(&ip, host, allow_loopback, policy) {
            tracing::warn!(
                host = %host,
                ip = %ip,
                allow_cidrs = policy.allow_cidrs.len(),
                allow_hosts_declared = !policy.allow_hosts.is_empty(),
                "DNS answer inside an allowlisted network rejected: hostname not declared in allow_hosts (SSRF validating resolver)"
            );
            return Err(Box::new(ForbiddenHostNotAllowlistedError {
                host: host.to_owned(),
                ip,
            }));
        }
        tracing::warn!(
            host = %host,
            ip = %ip,
            "Forbidden address in DNS answer rejected (SSRF validating resolver)"
        );
        Err(Box::new(ForbiddenResolutionError {
            host: host.to_owned(),
            ip,
        }))
    }
}

/// Whether the dial is denied **only** because the hostname was never
/// declared: the resolved address is inside an allowlisted CIDR, the host is
/// a real name (a literal skips the check), and the plain deny list — the one
/// that runs without any policy — would have forbidden it.
///
/// That last clause is what keeps the message honest: an address in a
/// never-allowable range (FIN-017) can never be described as "you forgot
/// `allow_hosts`", because declaring it would change nothing.
///
/// The always-denied check is load-bearing against a HAND-BUILT policy: such a
/// policy can contain `169.254.0.0/16` (config validation rejects it, but a
/// literal `NetworkPolicy` bypasses validation), and without this clause the
/// message would point an operator at a field that cannot help. Pinned by
/// `missing_declaration_class_is_distinct_from_the_ssrf_one`.
fn is_missing_allow_hosts_declaration(
    ip: &IpAddr,
    host: &str,
    allow_loopback: bool,
    policy: &NetworkPolicy,
) -> bool {
    is_forbidden_ip_with(ip, allow_loopback)
        && !crate::domain::ssrf_guard::is_always_denied_ip(ip)
        && policy.allows_ip(ip)
        && crate::domain::ssrf_guard::parse_ip_literal(host).is_none()
        && !policy.allows_host(host)
}

impl Default for ValidatingResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolve for ValidatingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        // `Name` carries only the hostname (`Name::as_str`); the request URI's
        // explicit port overrides whatever we return, and port 0 falls back to
        // the scheme default — see trait docs in wreq.
        let fut = Self::gai_lookup(
            name.as_str().to_owned(),
            self.validation_enabled,
            self.allow_loopback,
            Arc::clone(&self.policy),
            Arc::clone(&self.name_source),
        );
        Box::pin(fut)
    }
}

// Trait impl for the domain-owned `DefaultSsrfGuard` (type defined in
// `crate::domain::ssrf_guard`): the registry fallback must be constructible in
// domain, but applying the full guard requires `ValidatingResolver`, whose
// construction is infrastructure I/O a domain body may not reference. Same-crate
// impl placement keeps both halves satisfied (infrastructure → domain is the
// allowed direction). The resolver is constructed PER `secure_client` call, so
// the test-only hatch is captured at client-build time (non-test builds always
// validate) exactly like the previous inline wiring.
impl crate::domain::ssrf_guard::sealed::Sealed for crate::domain::ssrf_guard::DefaultSsrfGuard {}

impl crate::domain::ssrf_guard::SsrfGuard for crate::domain::ssrf_guard::DefaultSsrfGuard {
    fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder {
        builder
            .redirect(redirect_policy())
            // Direct egress only (#2500): wreq honours HTTP_PROXY/HTTPS_PROXY/
            // ALL_PROXY by default (`auto_sys_proxy`), which would resolve the
            // PROXY host through the validating resolver instead of the
            // destination — silently rejecting RFC1918 corporate proxies at
            // the guard and delegating destination IP checks to the proxy.
            // Proxy support is a declared non-feature; see docs/src/data-flow.md.
            .no_proxy()
            .dns_resolver(ValidatingResolver::new())
    }

    fn secure_client_with_loopback(
        &self,
        builder: wreq::ClientBuilder,
        allow_loopback: bool,
    ) -> wreq::ClientBuilder {
        builder
            .redirect(redirect_policy_with_loopback(allow_loopback))
            .no_proxy()
            .dns_resolver(ValidatingResolver::with_allow_loopback(allow_loopback))
    }

    fn secure_client_with_policy(
        &self,
        builder: wreq::ClientBuilder,
        allow_loopback: bool,
        policy: &NetworkPolicy,
    ) -> wreq::ClientBuilder {
        builder
            .redirect(redirect_policy_with_policy(allow_loopback, policy.clone()))
            .no_proxy()
            .dns_resolver(ValidatingResolver::with_policy(allow_loopback, policy))
    }

    fn secure_client_with_profile(
        &self,
        builder: wreq::ClientBuilder,
        profile: &ClientSecurityProfile<'_>,
    ) -> wreq::ClientBuilder {
        let dest = &profile.destination;
        // Stage 3b: the redirect stop always carries the DESTINATION policy.
        // Allowing a private CIDR for the proxy hop never opens literal
        // redirects into that CIDR (ADR-0021 Decision 2).
        let builder = builder
            .redirect(redirect_policy_with_policy(
                dest.allow_loopback,
                dest.network_policy.clone(),
            ))
            // #2500 pin: ambient HTTP_PROXY/HTTPS_PROXY/ALL_PROXY stay
            // ignored on every client. The explicit config-only declaration
            // is attached AFTER this call, so the environment never
            // participates in routing (ADR-0021 Decision 1/2).
            .no_proxy();
        let builder = match profile.via_proxy {
            // The matcher was built once at `ValidatedProxy` construction from
            // the validated origin; infallible here by construction. The
            // resolved Basic-auth credential is installed by the provider
            // startup wiring (Slice 2), which resolves `ProxyAuth` once.
            Some(proxy) => builder.proxy(proxy.wreq_proxy().clone()),
            None => builder,
        };
        // Stage 3c: without a proxy the resolver validates the DESTINATION —
        // byte-for-byte today's behavior. With one, the resolver validates the
        // PROXY dial under the proxy's own policy (allow_cidrs AND allow_hosts
        // + FIN-017): destination hostname resolution is delegated to the
        // proxy by protocol construction (CONNECT / absolute-form), the
        // split-horizon corporate-DNS case this variant exists for.
        let resolver_policy = match profile.via_proxy {
            Some(proxy) => proxy.endpoint_policy(),
            None => dest.clone(),
        };
        builder.dns_resolver(ValidatingResolver::with_policy(
            resolver_policy.allow_loopback,
            &resolver_policy.network_policy,
        ))
    }
}

// The pair, in process: the resolver that never answers against the accept
// queue that swallows the SYN (#2784). Its own file, so this one does not grow.
#[cfg(test)]
mod resolution_pair;

// Resolver tests. Determinism note: `tokio::net::lookup_host` resolves
// IP literals locally (no network, no resolver daemon), so every case
// here uses literal hosts only. A real-hostname DNS-failure case is
// deliberately omitted: `getaddrinfo` behavior for unresolvable names
// depends on the machine's resolver configuration and can block on
// unreachable DNS servers — not deterministic enough for CI.
//
// Env-var hermeticity contract: `cargo test` (including the Coverage
// job's `cargo llvm-cov` run) executes all tests of a binary in ONE
// process on parallel threads sharing process-global environment
// state — only nextest isolates per-test processes. Every test that
// reads or mutates `DISABLE_VALIDATING_RESOLVER_ENV` must therefore
// hold [`ENV_LOCK`] for its entire body, including the wiring proofs
// (which build clients through `DefaultSsrfGuard::secure_client`).

#[cfg(test)]
#[path = "ssrf_tests.rs"]
mod tests;

// `await_holding_lock` is deliberate here: each #[tokio::test] runs on its
// own current-thread runtime, so a guard held across `.await` can never be
// contended by another task on the same runtime — and holding it for the
// whole test body is exactly what serializes env mutation across test
// THREADS (issue #926).
#[cfg(test)]
#[path = "ssrf_resolver_env_tests.rs"]
mod resolver_env_tests;

#[cfg(test)]
#[path = "ssrf_wiring_tests.rs"]
mod wiring_tests;
