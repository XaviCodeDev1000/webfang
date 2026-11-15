//! The error types the SSRF resolver reports.
//! Their own module so `ssrf.rs` stays under its grandfathered size.

use std::net::IpAddr;

/// Error produced when a DNS answer violates SSRF policy.
///
/// The whole answer set is rejected fail-closed: a single forbidden address
/// among the returned records poisons the resolution, mirroring the
/// entry-level validator's semantics (never hand wreq a "safe subset" that an
/// attacker could pin connections to).
#[derive(Debug, thiserror::Error)]
#[error("DNS resolution of '{host}' returned forbidden address {ip} (SSRF)")]
pub struct ForbiddenResolutionError {
    /// Host whose answer set contained the forbidden address.
    pub host: String,
    /// The first forbidden address found in the answer set.
    pub ip: IpAddr,
}

/// Dial refused by the B2 allowlist's host half (slice 2, #1947).
///
/// Distinct from [`ForbiddenResolutionError`] on purpose: the address WAS
/// allowlisted, and what is missing is the declared hostname — a config
/// omission with an obvious fix, not an SSRF attempt. It therefore names the
/// field that closes it (`allow_hosts`), in Spanish, because it surfaces
/// verbatim in the operator's terminal through the transport error chain.
///
/// Reported only when the AND decision is what denied the dial: the resolved
/// IP is inside an allowlisted CIDR and the hostname is not declared, so
/// both facts are actionable.
#[derive(Debug, thiserror::Error)]
#[error("el host '{host}' resolvió a {ip}, una red privada: declaralo también en `allow_hosts` del provider (en `network_policy`) para permitir ese endpoint; `allow_cidrs` por sí solo es la precondición, no el permiso")]
pub struct ForbiddenHostNotAllowlistedError {
    /// Hostname dialed (never an IP literal: literals skip this check).
    pub host: String,
    /// Resolved address that IS inside an allowlisted CIDR.
    pub ip: IpAddr,
}
