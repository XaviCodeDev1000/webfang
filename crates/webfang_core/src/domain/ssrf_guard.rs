//! SSRF guard — pure deny-list policy plus the domain port (ADR-0012
//! sub-slice 3.C, issue #994).
//!
//! Defense is layered:
//!
//! 1. **Entry validation** — [`reject_forbidden_literal_url`] parses the
//!    request URL's host as an IP literal in ALL encodings (dotted decimal,
//!    hex `0x…`, bare decimal, abbreviated `127.1`, IPv6 incl. mapped forms)
//!    and rejects forbidden targets with a typed error BEFORE any socket
//!    opens. This is the ONE shared choke point: the CLI request path
//!    (`FetchRouter::fetch`, the robots fetcher, the scrape seed gate) and
//!    the MCP entry validator (`validate_url_no_ssrf`) all call it, so neither
//!    side can drift. Hostnames return `Ok` — they are covered by the async
//!    DNS entry check (MCP) and layer 2 below. This stays as fast-fail typed
//!    UX.
//! 2. **Connect-time enforcement** — the validating DNS resolver (defined in
//!    the infrastructure `ssrf` module, real GAI I/O stays there) is installed
//!    via `wreq::ClientBuilder::dns_resolver` through the [`SsrfGuard`] port on
//!    every scrape client and re-checks every DNS answer against
//!    [`is_forbidden_ip`]. Because redirects are followed inside the same
//!    client stack, each redirect hop's connection also resolves through this
//!    resolver, closing the gap where a hostname redirect target could reach an
//!    address that was never validated at entry (DNS rebinding / TOCTOU
//!    included).
//! 4. **Accepted residual (F-R3-7, ADR-0016 §6)** — hybrid escalation layers
//!    L2 (Obscura subprocess) and L3 (headless Chromium/CDP) perform their own
//!    networking, so the dial-level guard above does not run inside them. An
//!    entry-time re-check does NOT close the resulting TOCTOU window (it
//!    re-opens between the re-check and the subprocess's own dial). The residual
//!    is LOW and is ACCEPTED BY DESIGN — see ADR-0016 §6; re-evaluate whenever
//!    L2/L3 networking changes.
//! 3. **Belt-and-suspenders literal guard** — [`redirect_policy`] still stops
//!    redirects whose target is a *literal* forbidden IP synchronously,
//!    before any resolution happens.
//!
//! All layers share [`is_forbidden_ip`] as the single deny list; the
//! per-client loopback opt-in ([`is_forbidden_ip_with`], #1462) narrows it
//! for exactly `127.0.0.1` / `::1` and nothing else, and the per-provider
//! B2 allowlist ([`is_forbidden_ip_with_policy`] /
//! [`is_forbidden_resolved_address`], #1947) widens it for declared LAN
//! networks — with the never-allowable set (FIN-017) staying closed above
//! both.
//!
//! # Third-party types in `domain/` — accepted deliberately
//!
//! This module puts two `wreq` types into the domain layer:
//! [`wreq::redirect::Policy`] (as the [`redirect_policy`] return type) and
//! [`wreq::ClientBuilder`] (in the [`SsrfGuard`] port signature). They join the
//! accepted-leak surface already disclosed by the
//! [`downloader_factory`](crate::domain::downloader_factory) precedent
//! (`wreq::cookie::Jar` and `tokio_util::sync::CancellationToken` there).
//!
//! This is a known, accepted leak, not an oversight: the ADR-0010 intra-crate
//! direction gate (`scripts/check_intra_crate_direction.sh`) only inspects
//! `crate::<layer>::…` paths, so it cannot see third-party leakage at all. A
//! future `domain`-owned client-builder newtype would close the gap; until
//! then, do not read a green gate as "the domain layer is framework-free".

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::domain::network_policy::NetworkPolicy;
use crate::domain::providers::RedactedUrl;
use crate::domain::proxy_config::ClientSecurityProfile;

use wreq::redirect::Policy;

/// Test-only escape hatch for the literal-IP redirect guard.
///
/// Mirrors the MCP crate's `WEBFANG_MCP_DISABLE_SSRF` convention: wiremock
/// listens on 127.0.0.1, so harnesses that exercise redirect chains need this
/// set before building clients. The read is compiled only for unit tests (S9):
/// a shipped binary never consults it, so the synchronous redirect stop is
/// always armed there.
///
/// TEST-ISOLATION: Writers use `EnvGuard::{set,remove,with,clean}` or
/// `env_lock()` for permanent setup (#1126). Readers must hold `EnvGuard`
/// for full read-or-assert window (even pure readers use `EnvGuard::clean`)
/// or `#[serial]`; see #1308. Never nest `EnvGuard` in `env_lock()`; spawned
/// children exempt (see `sanitize_env` in `cli_harness.rs`).
pub const DISABLE_REDIRECT_GUARD_ENV: &str = "WEBFANG_DISABLE_SSRF_REDIRECT_GUARD";

/// Test-only escape hatch for the connect-time validating DNS resolver.
///
/// Same rationale as [`DISABLE_REDIRECT_GUARD_ENV`]: wiremock binds 127.0.0.1,
/// which [`is_forbidden_ip`] rejects, so any harness driving real connections
/// through the production clients must set `WEBFANG_DISABLE_SSRF_RESOLVER=1`
/// before clients are built. The read is compiled only for unit tests (S9); a
/// shipped binary always validates every DNS answer.
///
/// `pub` (not `pub(crate)`) so the discovery integration tests — external
/// test targets — can reference the single source of truth instead of
/// duplicating the variable name as a string (same as
/// [`DISABLE_ENTRY_GUARD_ENV`]).
///
/// TEST-ISOLATION: Writers use `EnvGuard::{set,remove,with,clean}` or
/// `env_lock()` for permanent setup (#1126). Readers must hold `EnvGuard`
/// for full read-or-assert window (even pure readers use `EnvGuard::clean`)
/// or `#[serial]`; see #1308. Never nest `EnvGuard` in `env_lock()`; spawned
/// children exempt (see `sanitize_env` in `cli_harness.rs`).
pub const DISABLE_VALIDATING_RESOLVER_ENV: &str = "WEBFANG_DISABLE_SSRF_RESOLVER";

/// Test-only escape hatch for the entry literal-IP guard
/// ([`reject_forbidden_literal_url`], F-06 + F-32, #1217).
///
/// Same rationale as the sibling hatches: wiremock binds 127.0.0.1, which the
/// entry guard rejects, so any harness driving requests at forbidden literals
/// through the production request path must set
/// `WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1`. Only the exact value `"1"` disarms
/// the guard — any other value (including `"0"`, `"true"`, `"yes"`) keeps
/// validation active, mirroring `DISABLE_VALIDATING_RESOLVER_ENV`.
/// Production never sets it.
///
/// `pub` (not `pub(crate)`) so the CLI behavioral harness — an external test
/// target — can reference the single source of truth instead of duplicating
/// the variable name as a string.
///
/// TEST-ISOLATION: Writers use `EnvGuard::{set,remove,with,clean}` or
/// `env_lock()` for permanent setup (#1126). Readers must hold `EnvGuard`
/// for full read-or-assert window (even pure readers use `EnvGuard::clean`)
/// or `#[serial]`; see #1308. Never nest `EnvGuard` in `env_lock()`; spawned
/// children exempt (see `sanitize_env` in `cli_harness.rs`).
pub const DISABLE_ENTRY_GUARD_ENV: &str = "WEBFANG_DISABLE_SSRF_ENTRY_GUARD";

/// Test-only escape hatch for the LLM base-URL SSRF gate (#703).
///
/// #1615 (DF-E9): this is the one SSRF hatch that is NOT reachable from a
/// production build. Its read sits behind `#[cfg(test)]` in
/// [`ssrf_gate`](crate::application::llm_extraction::ssrf_gate), so setting the
/// variable in a deployed process has no effect — the code that would consult
/// it is not compiled in. It is defined here, next to the other hatches, so a
/// caller that wants to name it has a canonical name and never a literal.
///
/// The value is still presence-based, and still only inside a test build:
/// hardening the value check would change behaviour under `cfg(test)` alone,
/// which is not a production surface and not what the finding was about. The
/// value is therefore irrelevant to the gate: the constant exists so the
/// variable is never spelled out at a call site, and so a rename cannot
/// desynchronize the writer and the reader (the same rule the sibling SSRF
/// hatches follow).
///
/// `pub` (not `pub(crate)`) so external test targets can reference the single
/// source of truth instead of duplicating the variable name as a string.
pub const LLM_SSRF_TEST_HATCH_ENV: &str = "WEBFANG_DISABLE_SSRF";

/// Test-only escape hatch for the MCP entry pre-check (layer 1), read by
/// `webfang_mcp::mcp_server::ssrf` (`is_ssrf_enabled`).
///
/// Canonical name for the MCP layer-1 hatch (#1348): MCP code and MCP test
/// harnesses reference this constant instead of restating the string, so a
/// rename cannot silently desynchronize writers and readers. Only the exact
/// value `"1"` disarms the pre-check — any other value (including `"0"`,
/// `"true"`, `"yes"`) keeps it armed, mirroring the sibling hatches
/// (docs/ssrf-layers.md). Production never sets it.
///
/// TEST-ISOLATION: Writers use `EnvGuard::{set,remove,with,clean}` or
/// `env_lock()` for permanent setup (#1126). Readers must hold `EnvGuard`
/// for full read-or-assert window (even pure readers use `EnvGuard::clean`)
/// or `#[serial]`; see #1308. Never nest `EnvGuard` in `env_lock()`; spawned
/// children exempt (see `sanitize_env` in `cli_harness.rs`).
pub const WEBFANG_MCP_DISABLE_SSRF_ENV: &str = "WEBFANG_MCP_DISABLE_SSRF";

/// Returns `true` only when the raw MCP SSRF disable value is exactly `"1"`.
///
/// This parser is deliberately pure and exact: an absent value, an empty value,
/// surrounding whitespace, and alternate truthy spellings such as `"0"`,
/// `"false"`, `"yes"`, and `"TRUE"` all leave the guard enabled. It governs
/// only the MCP entry pre-check and does not apply to the separate
/// `WEBFANG_DISABLE_SSRF` contract used by LLM extraction.
#[must_use]
pub fn disables_ssrf(raw: Option<&str>) -> bool {
    raw == Some("1")
}

mod classification;
mod env_hatches;
mod forbidden_literal;
mod ip_helpers;

pub use classification::{
    is_always_denied_ip, is_cgnat, is_forbidden_ip, is_forbidden_ip_with,
    is_forbidden_ip_with_policy, is_ipv4_this_network, is_ipv6_link_local, is_ipv6_unique_local,
    is_permitted_loopback, is_reserved_v4,
};
pub(crate) use env_hatches::{redirect_guard_disabled_by_env, validating_resolver_disabled_by_env};
pub use forbidden_literal::ForbiddenLiteral;
pub use ip_helpers::{
    ipv6_6to4_embedded_v4, ipv6_isatap_embedded_v4, ipv6_nat64_embedded_v4,
    is_forbidden_literal_host, is_forbidden_literal_host_with,
    is_forbidden_literal_host_with_policy, is_forbidden_resolved_address,
    is_ipv6_nat64_non_canonical, is_ipv6_teredo, parse_ip_literal,
};

/// Is the [`DISABLE_ENTRY_GUARD_ENV`] hatch armed right now?
///
/// THE single reader of that variable: only the exact value `"1"` arms it —
/// an absent value, an empty value and every other truthy spelling leave the
/// literal-IP entry guard active (the same rule [`disables_ssrf`] applies to
/// the MCP variable). Read on every call rather than cached, because a test
/// harness arms and clears it between cases (#1126, #1308).
///
/// Unlike its two siblings (`redirect_guard_disabled_by_env` /
/// `validating_resolver_disabled_by_env`, both `cfg(test)`-confined), this one
/// IS reachable from a shipped binary — by design: wiremock-bound smoke runs
/// and self-hosted operators both need it. What it disarms is narrow: the
/// literal-IP rejection at the ENTRY boundary, nothing else. The CLI announces
/// that scope once per run through
/// [`cli::ssrf_hatch_notice`](crate::cli::ssrf_hatch_notice).
#[must_use]
pub fn entry_guard_disabled_by_env() -> bool {
    std::env::var(DISABLE_ENTRY_GUARD_ENV).as_deref() == Ok("1")
}

/// Pure, non-logging verdict of the literal entry guard (layer 2 of
/// `docs/ssrf-layers.md`).
///
/// Returns the [`ForbiddenLiteral`] the guard would raise for `url` — same host
/// parsing ([`parse_ip_literal`], every `inet_aton`/WHATWG encoding), same deny
/// list ([`is_forbidden_ip`]), same [`DISABLE_ENTRY_GUARD_ENV`] exact-`"1"` hatch
/// — without the `WARN`. [`reject_forbidden_literal_url`] is this verdict plus the
/// log line, so the two can never disagree.
///
/// Diagnostic callers need exactly that: a run whose seed was already cut must be
/// able to name the refusal after the fact without emitting a second identical
/// warning (#1381). Hostnames return `None` — the layer that refuses them is the
/// connect-time validating resolver, not this one.
#[must_use]
pub fn seed_guard_refusal(url: &url::Url) -> Option<ForbiddenLiteral> {
    if entry_guard_disabled_by_env() {
        return None;
    }
    let host = url.host_str().unwrap_or("");
    let ip = parse_ip_literal(host).filter(is_forbidden_ip)?;
    Some(ForbiddenLiteral {
        host: host.to_owned(),
        ip,
    })
}

/// Shared literal-IP entry guard (F-06 + F-32, #1217): the ONE choke point for
/// CLI and MCP.
///
/// Returns `Err` when `url`'s host parses (via [`parse_ip_literal`]) to an
/// address [`is_forbidden_ip`] rejects — before any socket opens. Hostnames
/// return `Ok`: they need async DNS, which stays with the MCP entry validator
/// and the connect-time validating resolver.
///
/// The [`DISABLE_ENTRY_GUARD_ENV`] escape hatch (exact `"1"` only) is read on
/// every call so test harnesses driving wiremock (127.0.0.1) through the
/// production request path can disarm this layer without touching the others.
/// Production never sets it.
#[tracing::instrument(skip(url), fields(host = %url.host_str().unwrap_or("")))]
pub fn reject_forbidden_literal_url(url: &url::Url) -> Result<(), ForbiddenLiteral> {
    match seed_guard_refusal(url) {
        Some(rejection) => {
            tracing::warn!(
                host = %rejection.host,
                ip = %rejection.ip,
                "SSRF literal-IP target rejected at entry (no socket opened)"
            );
            Err(rejection)
        },
        None => Ok(()),
    }
}

/// Redirect policy for all scrape clients: the default 10-hop limit with an
/// added SSRF guard that stops redirects targeting a literal forbidden IP.
///
/// This is now **belt-and-suspenders**, not the primary hostname defense:
/// redirect targets given as hostnames are enforced by the connect-time
/// validating DNS resolver (see [`SsrfGuard`]) at connect time (every hop of a
/// followed redirect resolves through the same client stack), so this sync
/// guard only short-circuits *literal* IP targets before any resolution
/// happens. Non-forbidden attempts are delegated to [`Policy::default`] (which
/// is `Policy::limited(10)`), so the hop cap and loop protection of the
/// previous `Policy::limited(10)` are preserved.
///
/// Unit-test harnesses that exercise redirect chains against wiremock (which
/// binds 127.0.0.1) can set `WEBFANG_DISABLE_SSRF_REDIRECT_GUARD=1` before
/// building clients; non-test builds never read it, so the stop is always
/// armed in a shipped binary (S9).
#[must_use]
pub fn redirect_policy() -> Policy {
    let base = Policy::default();
    let guard_enabled = !redirect_guard_disabled_by_env();
    Policy::custom(move |attempt| {
        if guard_enabled && attempt.uri.host().is_some_and(is_forbidden_literal_host) {
            tracing::warn!(
                target_uri = %RedactedUrl::from_str_lossy(&attempt.uri.to_string()),
                "Redirect to forbidden literal IP blocked (SSRF guard)"
            );
            attempt.stop()
        } else {
            base.redirect(attempt)
        }
    })
}

/// Pure verdict behind both redirect policies: does a redirect to `host`
/// stop synchronously under `allow_loopback`? (#1462)
///
/// Unit-testable without building a client; both [`redirect_policy`] (with
/// `false`) and [`redirect_policy_with_loopback`] delegate here.
#[must_use]
pub fn redirect_literal_blocked(host: Option<&str>, allow_loopback: bool) -> bool {
    host.is_some_and(|h| is_forbidden_literal_host_with(h, allow_loopback))
}

/// Pure verdict behind [`redirect_policy_with_policy`]: does a redirect to
/// `host` stop synchronously under this provider's policy? Same shape as
/// [`redirect_literal_blocked`], with the B2 allowlist applied — so a hop to
/// an allowlisted LAN literal is still followed.
#[must_use]
pub fn redirect_literal_blocked_with_policy(
    host: Option<&str>,
    allow_loopback: bool,
    policy: &NetworkPolicy,
) -> bool {
    host.is_some_and(|h| is_forbidden_literal_host_with_policy(h, allow_loopback, policy))
}

/// Redirect policy with a per-client loopback permit (#1462).
///
/// Same 10-hop base as [`redirect_policy`], but the synchronous literal-IP
/// stop honors `allow_loopback` instead of the process-env hatch: a
/// self-hosted endpoint that 302-redirects within loopback (or is addressed
/// by a loopback literal) stays reachable for the permitted client only.
/// Unlike [`redirect_policy`], this variant reads NO environment — the flag
/// is explicit at the call site.
#[must_use]
pub fn redirect_policy_with_loopback(allow_loopback: bool) -> Policy {
    let base = Policy::default();
    Policy::custom(move |attempt| {
        if redirect_literal_blocked(attempt.uri.host(), allow_loopback) {
            tracing::warn!(
                target_uri = %RedactedUrl::from_str_lossy(&attempt.uri.to_string()),
                allow_loopback,
                "Redirect to forbidden literal IP blocked (SSRF guard)"
            );
            attempt.stop()
        } else {
            base.redirect(attempt)
        }
    })
}

/// Redirect policy with a per-provider loopback permit AND network allowlist
/// (B2 slice 2, #1947).
///
/// Same 10-hop base as [`redirect_policy`], but the synchronous literal-IP
/// stop honors this provider's `allow_loopback` + [`NetworkPolicy`]: a
/// redirect hop to an allowlisted LAN literal is still followed, while an
/// undeclared one (and every never-allowable range, FIN-017) is stopped
/// before any socket opens.
///
/// Hostname hops are the resolver's business — each hop's connection
/// resolves through the validating resolver, which applies the AND decision
/// per hop, so a redirect cannot smuggle an undeclared hostname into the LAN.
///
/// Reads NO environment, exactly like [`redirect_policy_with_loopback`]; the
/// policy is explicit at the call site. The policy is cloned once into the
/// callback (it is per-client state owned by the client, not process state).
#[must_use]
pub fn redirect_policy_with_policy(allow_loopback: bool, policy: NetworkPolicy) -> Policy {
    let base = Policy::default();
    Policy::custom(move |attempt| {
        if redirect_literal_blocked_with_policy(attempt.uri.host(), allow_loopback, &policy) {
            tracing::warn!(
                target_uri = %RedactedUrl::from_str_lossy(&attempt.uri.to_string()),
                allow_loopback,
                "Redirect to forbidden literal IP blocked (SSRF guard, provider network policy)"
            );
            attempt.stop()
        } else {
            base.redirect(attempt)
        }
    })
}

/// Default concrete guard — the type lives in `domain`, its [`SsrfGuard`]
/// impl lives in the infrastructure `ssrf` module.
///
/// Type-here / impl-there placement (deliberate, not an oversight): the
/// registry fallback must be constructible inside `domain` — bypass paths
/// (CLI preflight, crawler discovery, sitemap discovery) never build a
/// `Container`, and the spec requires them to receive the FULL guard, not a
/// no-op stub. Applying the full guard requires the validating resolver, whose
/// construction is infrastructure I/O that a `domain` body may not reference
/// under the ADR-0010 gate. Same-crate impl placement solves it: Rust permits
/// the trait impl in any module of the defining crate, so the impl block sits
/// next to the resolver in the infrastructure `ssrf` module (infrastructure →
/// domain is the allowed direction) while this unit type stays
/// domain-constructible.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultSsrfGuard;

#[allow(missing_docs)]
pub(crate) mod sealed {
    #[allow(missing_docs)]
    pub trait Sealed {}
}

/// Domain port for the layered SSRF guard — sealed.
/// Only the domain `DefaultSsrfGuard` (impl in the infrastructure `ssrf`
/// module) may implement it.
pub trait SsrfGuard: Send + Sync + sealed::Sealed {
    /// Apply the full SSRF guard (redirect policy + validating resolver) to a
    /// client builder. Also pins direct egress (#2500): the proxy environment
    /// is ignored, so the resolver always resolves the real destination.
    /// Consuming: `wreq::ClientBuilder` is not `Clone`.
    fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder;

    /// Parameterized variant of [`secure_client`](Self::secure_client) (#1462):
    /// when `allow_loopback` is set, dials to exactly `127.0.0.1` / `::1`
    /// are permitted (self-hosted endpoints); every other forbidden range
    /// stays denied, in the redirect policy and in the validating resolver
    /// alike.
    ///
    /// The flag travels with this single builder call — never through the
    /// keep-first process registry, which cannot express "this client may
    /// dial loopback while every other client may not".
    fn secure_client_with_loopback(
        &self,
        builder: wreq::ClientBuilder,
        allow_loopback: bool,
    ) -> wreq::ClientBuilder;

    /// Additive policy-parameterized variant (B2 slice 2, #1947):
    /// [`secure_client_with_loopback`](Self::secure_client_with_loopback)
    /// plus this provider's [`NetworkPolicy`] allowlist, applied in both the
    /// synchronous redirect stop and the connect-time validating resolver.
    ///
    /// Additive on purpose: `secure_client_with_loopback` keeps its exact
    /// signature, so the scrape path (`wreq_downloader.rs`) and every
    /// caller that never declares a policy are untouched.
    ///
    /// Same contract as the loopback flag — a per-client parameter, never
    /// process registry state: the allowlist is copied into the client's own
    /// resolver, so two providers with different policies in the same process
    /// cannot influence each other.
    fn secure_client_with_policy(
        &self,
        builder: wreq::ClientBuilder,
        allow_loopback: bool,
        policy: &NetworkPolicy,
    ) -> wreq::ClientBuilder;

    /// Additive profile variant (ADR-0021): the full guard with the
    /// destination and proxy trust domains separated BY TYPE —
    /// `domain::proxy_config::ClientSecurityProfile` pairs the destination
    /// `domain::proxy_config::EndpointPolicy` (entry gate, redirect stops,
    /// direct-dial resolver) with an optional validated proxy (its own dial
    /// policy + prebuilt `wreq` matcher).
    ///
    /// Wiring order is the fetch-guard invariant (AGENTS.md): redirect from
    /// the DESTINATION policy → `.no_proxy()` (ambient env stays ignored) →
    /// the proxy matcher, when configured → the validating resolver carrying
    /// the PROXY policy when proxying, the destination policy otherwise.
    /// Allowing a private CIDR for the proxy hop never opens redirects into
    /// that CIDR — the redirect guard only ever sees the destination policy.
    ///
    /// Infallible on purpose: every proxy validation happens once at
    /// `ValidatedProxy::try_new` (including building the `wreq` matcher), so
    /// no wiring-time failure path exists to surface.
    ///
    /// Additive like its siblings: the three pre-ADR-0021 methods keep their
    /// exact signatures, and every caller without a proxy declaration is
    /// untouched.
    fn secure_client_with_profile(
        &self,
        builder: wreq::ClientBuilder,
        profile: &ClientSecurityProfile<'_>,
    ) -> wreq::ClientBuilder;
}

mod registry;

#[cfg(test)]
pub(crate) use registry::ssrf_guard_armed;
pub use registry::{set_ssrf_guard, ssrf_guard};

#[cfg(test)]
mod tests_ip;

#[cfg(test)]
mod tests_literals;

#[cfg(test)]
mod tests_resolved_dial;

#[cfg(test)]
mod tests_runtime;

#[cfg(test)]
mod tests_support;
