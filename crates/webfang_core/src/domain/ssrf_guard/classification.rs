use super::*;

/// Returns `true` if `ip` falls within a forbidden range.
///
/// Covers:
/// - IPv4: loopback (127.0.0.0/8), private (10/8, 172.16/12, 192.168/16),
///   link-local (169.254/16), the "this network on this host" family
///   (0.0.0.0/8 — reserved and never globally routable, RFC 1122 §3.2.1.3),
///   broadcast (255.255.255.255), reserved (240.0.0.0/4), CGNAT
///   (100.64.0.0/10).
/// - IPv6: loopback (::1), unspecified (::), unique-local (fc00::/7),
///   unicast link-local (fe80::/10), IPv4-mapped and IPv4-compatible
///   (re-validated as IPv4), NAT64 well-known prefix 64:ff9b::/96,
///   6to4 2002::/16 and ISATAP (RFC 5214 §6.1, ATOP identifier 0x5efe)
///   (embedded IPv4 re-validated as IPv4), the NAT64
///   Network-Specific Prefixes (the rest of 64:ff9b::/32, fail-closed),
///   Teredo 2001:0000::/32 (fail-closed).
#[must_use]
pub fn is_forbidden_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // 0.0.0.0/8 ("this network on this host", RFC 1122
                // §3.2.1.3): the whole family is reserved and never globally
                // routable, and 0.0.0.0 is the unspecified address — a
                // local-host alias as a destination. The family is denied,
                // not just the 0.0.0.0 address (#1965, A5).
                || is_ipv4_this_network(v4)
                || v4.is_broadcast()
                || is_reserved_v4(v4)
                || is_cgnat(v4)
        },
        IpAddr::V6(v6) => {
            // Native IPv6 special ranges first: `::1` and `::` also carry
            // IPv4-compatible representations (`::0.0.0.1` / `::0.0.0.0`) that
            // would otherwise re-validate as non-forbidden IPv4 addresses.
            if v6.is_loopback()
                || v6.is_unspecified()
                || is_ipv6_unique_local(v6)
                || is_ipv6_link_local(v6)
            {
                return true;
            }
            // The NAT64 Network-Specific Prefixes (RFC 6052 §2.2) fail closed
            // BEFORE the embedding check below, and they must stay there: the
            // embedding helper returns `None` for them (their embedded IPv4
            // sits at an undecodable position), so a later check could never
            // reach them. RFC 6052 §2.2 places the embedded IPv4 right after
            // the prefix, whose length varies (/32../64), and the address does
            // NOT encode that length — so the embedded address cannot be
            // located, let alone validated. Same fail-closed reason as
            // Teredo (#1965, A6).
            if is_ipv6_nat64_non_canonical(v6) {
                return true;
            }
            // RFC 4291 §2.5.5.2: IPv4-mapped (`::ffff:a.b.c.d`) and the deprecated
            // IPv4-compatible (`::a.b.c.d`) form both surface through `to_ipv4()`;
            // both must be re-validated against the IPv4 deny list, otherwise any
            // forbidden IPv4 address can bypass the guard as `::ffff:x.x.x.x`.
            if let Some(v4) = v6.to_ipv4() {
                return is_forbidden_ip(&IpAddr::V4(v4));
            }
            // Teredo (RFC 4380, 2001:0000::/32): the client IPv4/port are
            // XOR-obfuscated against the attacker-controllable relay's view of
            // them, so embedded-address re-validation cannot be trusted. Fail
            // closed over the whole range.
            //
            // BEFORE the encapsulation checks below, and not after: the ISATAP
            // marker (RFC 5214 §6.1) sits at a FIXED position that the Teredo
            // range overlaps, so an encapsulation check evaluated first could
            // shadow this fail-closed verdict with the embedded address's
            // (#2232). Neither NAT64 nor 6to4 can shadow it — their prefixes
            // are disjoint from 2001:0000::/32.
            if is_ipv6_teredo(v6) {
                return true;
            }
            // Translation/encapsulation prefixes carry an embedded IPv4 address
            // that the socket stack dials directly on IPv6-only networks with
            // live NAT64 (mobile carriers, enterprises): `64:ff9b::a.b.c.d`
            // connects to `a.b.c.d` through the NAT64 gateway, and neither
            // native-range checks nor `to_ipv4()` ever see it. ISATAP
            // (RFC 5214 §6.1) encapsulates the IPv4 the same way, and its
            // marker position is FIXED — so the embedded address can be
            // located and re-validated like the other two (#2232).
            // Re-validate every embedded IPv4 against the same deny list.
            if let Some(v4) = ipv6_nat64_embedded_v4(v6)
                .or_else(|| ipv6_6to4_embedded_v4(v6))
                .or_else(|| ipv6_isatap_embedded_v4(v6))
            {
                return is_forbidden_ip(&IpAddr::V4(v4));
            }
            false
        },
    }
}

/// Whether `ip` is one of the two loopback addresses the per-client
/// `allow_loopback` opt-in permits (#1462): exactly `127.0.0.1` and `::1`.
///
/// Deliberately narrow: the rest of `127.0.0.0/8`, IPv4-mapped/compatible
/// forms (`::ffff:127.0.0.1`), NAT64/6to4 embeddings and every other range
/// stay denied even with the flag set — the opt-in is for a self-hosted
/// endpoint on the same machine, not a wider hole.
#[must_use]
pub fn is_permitted_loopback(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => *v4 == Ipv4Addr::LOCALHOST,
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Parameterized deny-list verdict (#1462): [`is_forbidden_ip`] with a
/// per-client loopback permit.
///
/// `allow_loopback` opens exactly [`is_permitted_loopback`] and nothing
/// else. The flag travels as a call parameter from the provider config —
/// never through the keep-first process registry, which cannot express
/// "this client may dial loopback while every other client may not".
#[must_use]
pub fn is_forbidden_ip_with(ip: &IpAddr, allow_loopback: bool) -> bool {
    if allow_loopback && is_permitted_loopback(ip) {
        return false;
    }
    is_forbidden_ip(ip)
}

/// Dial-time always-denied set (FIN-017): link-local, the "this network on
/// this host" family (0.0.0.0/8), unspecified, multicast, broadcast,
/// reserved, Teredo and the NAT64 Network-Specific Prefixes — the ranges no
/// allowlist entry can ever reach.
///
/// BOTH IPv4-in-IPv6 spellings normalize to IPv4 first: the IPv4-mapped
/// (`::ffff:a.b.c.d`) and the deprecated IPv4-compatible (`::a.b.c.d`) one.
/// The compatible form USED to be left unnormalized here, on the belief that
/// it "never matches an allowlist, so it stays denied either way" — that
/// justification was FALSE: a hand-built allowlist of `::/120` does match
/// `::2`, and the policy path then ALLOWED it while the flat path denied it.
/// The whole contract of this set is "ranges no allowlist entry can ever
/// reach", so normalizing the compatible spelling is what makes the three
/// verdict paths agree (#2231).
///
/// The NATIVE V6 special ranges keep their own spelling: `::1` and `::` also
/// carry compatible projections, and `::1` belongs to the exclusive
/// `allow_loopback` flag (B1, #1462) — not to this never-allowable set.
///
/// The CANONICAL translation prefixes are absent on purpose: their verdict
/// derives from the embedded IPv4 via [`is_forbidden_ip_with_policy`], and
/// the canonical NAT64 Well-Known Prefix (64:ff9b::/96) is the standard
/// path that must keep working. The NON-CANONICAL NAT64 space (the rest of
/// 64:ff9b::/32) is the opposite case and IS in this set: a
/// Network-Specific Prefix does not encode where the embedded IPv4 sits, so
/// it cannot be validated and fails closed (#1965, A6). ISATAP is absent for
/// the same reason as the canonical NAT64 prefix: its verdict derives from
/// the embedded IPv4, so the allowlist opt-in still reaches a legitimate
/// private address through it (#2232).
#[must_use]
pub fn is_always_denied_ip(ip: &IpAddr) -> bool {
    let normalized = match ip {
        // RFC 4291 §2.5.5.2: the mapped and the deprecated compatible
        // spellings both surface through `to_ipv4()`. The native V6 special
        // ranges are checked FIRST, exactly like in [`is_forbidden_ip`]:
        // `::1` (loopback) and `::` (unspecified) also carry compatible
        // projections, and the loopback belongs to the `allow_loopback` flag.
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                *ip
            } else {
                v6.to_ipv4().map_or(*ip, IpAddr::V4)
            }
        },
        v4 @ IpAddr::V4(_) => *v4,
    };
    match normalized {
        IpAddr::V4(v4) => {
            v4.is_link_local()
                // 0.0.0.0/8: reserved and never globally routable, and
                // 0.0.0.0 is the unspecified address — a local-host alias
                // as a destination (#1965, A5).
                || is_ipv4_this_network(&v4)
                || v4.is_broadcast()
                || v4.is_multicast()
                || is_reserved_v4(&v4)
        },
        IpAddr::V6(v6) => {
            v6.is_unspecified()
                || is_ipv6_link_local(&v6)
                || v6.is_multicast()
                || is_ipv6_teredo(&v6)
                // The NAT64 Network-Specific Prefixes cannot be decoded, so
                // they fail closed here — before any allowlist entry is
                // consulted (#1965, A6).
                || is_ipv6_nat64_non_canonical(&v6)
        },
    }
}

/// Policy-parameterized verdict (B2, ADR-0004, issue #1947):
/// [`is_forbidden_ip`] plus a per-provider [`NetworkPolicy`] allowlist.
///
/// Evaluation order is security-first:
///
/// 1. The always-denied set (FIN-017) denies FIRST — even a hand-built
///    policy cannot reach `169.254.169.254`; config validation rejecting
///    those entries is the first layer, this is the second.
/// 2. Loopback stays exclusively under the B1 `allow_loopback` flag
///    (#1462) — an allowlist entry cannot open it (validation rejects
///    those too, this re-checks).
/// 3. Public addresses allow, exactly like [`is_forbidden_ip`].
/// 4. NAT64/6to4/ISATAP-embedded IPv4 re-validates the embedded address
///    against the same policy, so an allowlist is applied to the address
///    the socket actually dials, never to the translation prefix as a
///    blanket.
///    The loopback exemption does NOT travel through a translation prefix:
///    `64:ff9b::7f00:1` reaches `127.0.0.1` through a NAT64 gateway, and
///    the B1 flag is for the literal loopback only (#1965). The same holds
///    for the encapsulation form `::5efe:7f00:1` (#2232).
///    The denial is unconditional — a translated loopback is refused even
///    with the flag off. The only verdict it changes is one a hand-built
///    policy could reach by allowlisting `127.0.0.0/8`, and the config
///    layer rejects loopback prefixes outright, so in production this can
///    only ever turn "permitted" into "forbidden".
/// 5. An address inside an allowlisted CIDR allows — RFC1918, ULA and
///    CGNAT are reachable only through this explicit opt-in.
#[must_use]
pub fn is_forbidden_ip_with_policy(
    ip: &IpAddr,
    allow_loopback: bool,
    policy: &NetworkPolicy,
) -> bool {
    if is_always_denied_ip(ip) {
        return true;
    }
    if allow_loopback && is_permitted_loopback(ip) {
        return false;
    }
    if !is_forbidden_ip(ip) {
        return false;
    }
    if let IpAddr::V6(v6) = ip {
        if let Some(v4) = ipv6_nat64_embedded_v4(v6)
            .or_else(|| ipv6_6to4_embedded_v4(v6))
            .or_else(|| ipv6_isatap_embedded_v4(v6))
        {
            // The loopback exemption does not travel through a translation
            // prefix: `64:ff9b::7f00:1` reaches `127.0.0.1` through a NAT64
            // gateway, and the flag exempts the literal loopback only
            // (#1965, A2). The same holds for the encapsulation forms:
            // `::5efe:7f00:1` reaches `127.0.0.1` through an ISATAP router
            // (#2232).
            if is_permitted_loopback(&IpAddr::V4(v4)) {
                return true;
            }
            return is_forbidden_ip_with_policy(&IpAddr::V4(v4), allow_loopback, policy);
        }
    }
    !policy.allows_ip(ip)
}

/// Returns `true` if `v4` is within the CGNAT range 100.64.0.0/10
/// (100.64.0.0 – 100.127.255.255).
#[must_use]
pub fn is_cgnat(v4: &Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000
}

/// Returns `true` if `v4` is within the reserved range 240.0.0.0/4,
/// excluding broadcast (255.255.255.255). Mirrors std's still-unstable
/// `Ipv4Addr::is_reserved` (#27709) with the same semantics.
#[must_use]
pub fn is_reserved_v4(v4: &Ipv4Addr) -> bool {
    v4.octets()[0] & 0b1111_0000 == 0b1111_0000 && !v4.is_broadcast()
}

/// Returns `true` if `v4` falls within the "this network on this host"
/// family 0.0.0.0/8 (RFC 1122 §3.2.1.3).
///
/// The whole /8 is RESERVED and never globally routable — IANA keeps
/// 0.0.0.0/8 out of the global unicast space — so no public target lives
/// there and the family is denied as a family, not only the one address
/// (#1965, A5). Which member of the family routes where is a property of
/// the local routing table, not of the address space: the denial never
/// depends on it.
///
/// `0.0.0.0` itself is the unspecified address, and as a DESTINATION it is
/// a local-host alias on several stacks — `http://0.0.0.0/` and `http://0/`
/// reach a listener bound on the machine, a documented SSRF bypass.
///
/// Denying the family also closes the translated forms: `64:ff9b::0:1`
/// (NAT64) and `2002:0:1::` (6to4) embed an address of this family, and
/// the translation prefixes delegate their verdict to that embedded IPv4.
#[must_use]
pub fn is_ipv4_this_network(v4: &Ipv4Addr) -> bool {
    v4.octets()[0] == 0
}

/// Returns `true` if `v6` is within the unique-local range fc00::/7
/// (fc00:: – fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff).
#[must_use]
pub fn is_ipv6_unique_local(v6: &Ipv6Addr) -> bool {
    v6.segments()[0] & 0xfe00 == 0xfc00
}

/// Returns `true` if `v6` is within the unicast link-local range fe80::/10
/// (fe80:: – febf:ffff:...), symmetric with the IPv4 link-local denial
/// (169.254/16, which covers the cloud-metadata endpoint 169.254.169.254).
#[must_use]
pub fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    v6.segments()[0] & 0xffc0 == 0xfe80
}
