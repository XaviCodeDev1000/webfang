use super::tests_support::*;
use super::*;

#[test]
fn metadata_169_254_stays_denied_even_when_allowlisted() {
    let policy = hand_built_policy(&["169.254.0.0/16"]);
    assert!(is_forbidden_ip_with_policy(
        &addr("169.254.169.254"),
        false,
        &policy
    ));
    // The IPv4-mapped form must not bypass the always-denied set.
    assert!(is_forbidden_ip_with_policy(
        &addr("::ffff:169.254.169.254"),
        false,
        &policy
    ));
}

#[test]
fn teredo_stays_denied_even_when_hand_built_entry_covers_it() {
    let policy = hand_built_policy(&["2001:0::/32"]);
    assert!(is_forbidden_ip_with_policy(
        &addr("2001:0:5ef:3c2d::1"),
        false,
        &policy
    ));
}

#[test]
fn cgnat_default_denied_explicit_allowlist_permits() {
    let empty = NetworkPolicy::default();
    assert!(is_forbidden_ip_with_policy(
        &addr("100.64.0.5"),
        false,
        &empty
    ));
    let policy = hand_built_policy(&["100.64.0.0/10"]);
    assert!(!is_forbidden_ip_with_policy(
        &addr("100.64.0.5"),
        false,
        &policy
    ));
}

#[test]
fn rfc1918_default_denied_allowlist_permits() {
    let empty = NetworkPolicy::default();
    assert!(is_forbidden_ip_with_policy(
        &addr("10.0.0.5"),
        false,
        &empty
    ));
    let policy = hand_built_policy(&["10.0.0.0/8"]);
    assert!(!is_forbidden_ip_with_policy(
        &addr("10.0.0.5"),
        false,
        &policy
    ));
    // The IPv4-mapped form of the same address follows the same policy.
    assert!(!is_forbidden_ip_with_policy(
        &addr("::ffff:10.0.0.5"),
        false,
        &policy
    ));
}

#[test]
fn ula_default_denied_allowlist_permits() {
    let empty = NetworkPolicy::default();
    assert!(is_forbidden_ip_with_policy(&addr("fd00::1"), false, &empty));
    let policy = hand_built_policy(&["fc00::/7"]);
    assert!(!is_forbidden_ip_with_policy(
        &addr("fd00::1"),
        false,
        &policy
    ));
}

#[test]
fn nat64_embedded_allowlisted_v4_permits() {
    // 64:ff9b::a00:5 embeds 10.0.0.5 — the policy applies to the
    // address the socket actually dials.
    let policy = hand_built_policy(&["10.0.0.0/8"]);
    assert!(!is_forbidden_ip_with_policy(
        &addr("64:ff9b::a00:5"),
        false,
        &policy
    ));
}

#[test]
fn nat64_embedded_metadata_stays_denied() {
    // 64:ff9b::a9fe:a9fe embeds 169.254.169.254 — even a hand-built
    // blanket NAT64 entry cannot reach it.
    let policy = hand_built_policy(&["64:ff9b::/96", "10.0.0.0/8"]);
    assert!(is_forbidden_ip_with_policy(
        &addr("64:ff9b::a9fe:a9fe"),
        false,
        &policy
    ));
}

#[test]
fn loopback_stays_exclusively_under_the_flag() {
    // A policy cannot open loopback — only `allow_loopback` can.
    let policy = hand_built_policy(&["10.0.0.0/8"]);
    assert!(is_forbidden_ip_with_policy(
        &addr("127.0.0.1"),
        false,
        &policy
    ));
    assert!(is_forbidden_ip_with_policy(&addr("::1"), false, &policy));
    assert!(!is_forbidden_ip_with_policy(
        &addr("127.0.0.1"),
        true,
        &policy
    ));
    assert!(!is_forbidden_ip_with_policy(&addr("::1"), true, &policy));
}

/// A2 (#1965): the `allow_loopback` permit must not travel through the
/// translation forms. `64:ff9b::7f00:1` (NAT64) and `2002:7f00:1::`
/// (6to4) dial `127.0.0.1` on a NAT64/6to4 network, so the embedded
/// address is judged as the plain IPv4 it encodes — WITHOUT the permit.
/// The plain forms keep working: the flag is for a self-hosted endpoint
/// on this machine, not for a wider hole.
#[test]
fn translation_forms_of_loopback_stay_denied_under_the_flag() {
    let policy = hand_built_policy(&["10.0.0.0/8"]);
    // The embedded loopback is denied on both translation forms.
    assert!(is_forbidden_ip_with_policy(
        &addr("64:ff9b::7f00:1"),
        true,
        &policy
    ));
    assert!(is_forbidden_ip_with_policy(
        &addr("2002:7f00:1::"),
        true,
        &policy
    ));
    // ... and with the flag off, nothing changes for them.
    assert!(is_forbidden_ip_with_policy(
        &addr("64:ff9b::7f00:1"),
        false,
        &policy
    ));
    assert!(is_forbidden_ip_with_policy(
        &addr("2002:7f00:1::"),
        false,
        &policy
    ));
    // The plain loopback forms stay permitted under the flag.
    assert!(!is_forbidden_ip_with_policy(
        &addr("127.0.0.1"),
        true,
        &policy
    ));
    assert!(!is_forbidden_ip_with_policy(&addr("::1"), true, &policy));
    // The IPv4-mapped form is not a translation form and stays denied.
    assert!(is_forbidden_ip_with_policy(
        &addr("::ffff:127.0.0.1"),
        true,
        &policy
    ));
}

/// A5 (#1965): the "this network on this host" family 0.0.0.0/8 must be
/// denied as a FAMILY. The whole /8 is reserved and never globally
/// routable, and `0.0.0.0` itself is the unspecified address — a
/// local-host alias as a destination.
///
/// The deny list must hold on every path, and a hand-built policy that
/// allowlists the family must not reach it either: the always-denied set
/// denies FIRST, before any allowlist entry is consulted.
#[test]
fn this_network_family_is_denied_on_every_path() {
    let policy = hand_built_policy(&["0.0.0.0/8"]);
    for literal in ["0.0.0.0", "0.0.0.1", "0.0.0.2", "0.0.255.255", "0.1.0.1"] {
        let ip = addr(literal);
        assert!(
            is_forbidden_ip(&ip),
            "{literal} must be forbidden on the flat path"
        );
        assert!(
            is_forbidden_ip_with_policy(&ip, false, &policy),
            "{literal} must stay forbidden under a hand-built allowlist"
        );
        assert!(
            is_forbidden_ip_with_policy(&ip, true, &policy),
            "{literal} must stay forbidden under a hand-built allowlist, even with the loopback flag"
        );
        assert!(
            is_forbidden_resolved_address(&ip, "probe.lan", false, &policy),
            "{literal} must stay forbidden on the resolved path"
        );
        assert!(
            is_forbidden_resolved_address(&ip, "probe.lan", true, &policy),
            "{literal} must stay forbidden on the resolved path, even with the loopback flag"
        );
    }
}

/// A5 (#1965): the translated forms of the 0.0.0.0/8 family are denied
/// too. The translation prefixes delegate their verdict to the embedded
/// IPv4, so a hole in the IPv4 deny list is a hole in BOTH translation
/// paths.
///
/// The loopback flag is irrelevant here: this is NOT the loopback
/// exemption, it is the "this network" family — `64:ff9b::0:1` (NAT64)
/// and `2002:0:1::` (6to4) embed `0.0.0.1`, which is denied with the
/// flag on and off alike.
#[test]
fn translated_this_network_forms_are_denied_with_and_without_the_flag() {
    let policy = hand_built_policy(&["10.0.0.0/8"]);
    for literal in ["64:ff9b::0:1", "2002:0:1::"] {
        let ip = addr(literal);
        assert!(
            is_forbidden_ip(&ip),
            "{literal} must be forbidden on the flat path"
        );
        for allow_loopback in [false, true] {
            assert!(
                is_forbidden_ip_with_policy(&ip, allow_loopback, &policy),
                "{literal} must stay forbidden on the policy path (allow_loopback={allow_loopback})"
            );
            assert!(
                is_forbidden_resolved_address(&ip, "probe.lan", allow_loopback, &policy),
                "{literal} must stay forbidden on the resolved path (allow_loopback={allow_loopback})"
            );
        }
    }
}

/// A6 (#1965): a NAT64 Network-Specific Prefix (RFC 6052 §2.2) does not
/// encode where the embedded IPv4 sits, so it cannot be validated — the
/// whole space fails closed, exactly like Teredo.
#[test]
fn non_canonical_nat64_is_denied_on_every_path() {
    let policy = hand_built_policy(&["10.0.0.0/8"]);
    for literal in [
        "64:ff9b:1::7f00:1",
        "64:ff9b:1::a00:1",
        "64:ff9b:0:1::1",
        "64:ff9b:1:0:0:0:7f00:1",
    ] {
        let ip = addr(literal);
        assert!(
            is_forbidden_ip(&ip),
            "{literal} must be forbidden on the flat path"
        );
        for allow_loopback in [false, true] {
            assert!(
                is_forbidden_ip_with_policy(&ip, allow_loopback, &policy),
                "{literal} must stay forbidden on the policy path (allow_loopback={allow_loopback})"
            );
            assert!(
                is_forbidden_resolved_address(&ip, "probe.lan", allow_loopback, &policy),
                "{literal} must stay forbidden on the resolved path (allow_loopback={allow_loopback})"
            );
        }
    }
}

/// A6 (#1965) positive control: the LEGITIMATE translation cases must
/// keep working, pinned here so a future change cannot silently break
/// them.
///
/// - `64:ff9b::8.8.8.8` (NAT64 of a public address) stays ALLOWED: the
///   canonical Well-Known Prefix is the standard path for IPv6-only
///   deployments that scrape public sites through a NAT64 gateway.
/// - `2002:808:808::` (6to4 of a public address) stays ALLOWED.
/// - `64:ff9b::7f00:1` (NAT64 of the loopback) stays FORBIDDEN — the
///   merged A2 behaviour, unchanged.
/// - `64:ff9b::a00:1` (NAT64 of a private address) stays FORBIDDEN.
#[test]
fn legitimate_translation_forms_keep_working() {
    assert!(!is_forbidden_ip(&addr("64:ff9b::8.8.8.8")));
    assert!(!is_forbidden_ip(&addr("2002:808:808::")));
    assert!(is_forbidden_ip(&addr("64:ff9b::7f00:1")));
    assert!(is_forbidden_ip(&addr("64:ff9b::a00:1")));
}

#[test]
fn public_addresses_unchanged_under_empty_policy() {
    let empty = NetworkPolicy::default();
    assert!(!is_forbidden_ip_with_policy(
        &addr("8.8.8.8"),
        false,
        &empty
    ));
    assert!(!is_forbidden_ip_with_policy(
        &addr("2001:4860:4860::8888"),
        false,
        &empty
    ));
}

#[test]
fn ipv4_mapped_forms_are_revalidated_as_ipv4() {
    // IPv4-mapped (RFC 4291 §2.5.5.2)
    assert!(is_forbidden_ip(&addr("::ffff:127.0.0.1")));
    assert!(is_forbidden_ip(&addr("::ffff:10.0.0.1")));
    assert!(is_forbidden_ip(&addr("::ffff:169.254.169.254")));
    assert!(is_forbidden_ip(&addr("::ffff:100.64.0.1")));
    assert!(!is_forbidden_ip(&addr("::ffff:8.8.8.8")));
    // Deprecated IPv4-compatible form
    assert!(is_forbidden_ip(&addr("::127.0.0.1")));
}

#[test]
fn native_v6_special_ranges_win_over_mapped_revalidation() {
    // `::1` and `::` also have IPv4-compatible projections; the native
    // loopback/unspecified checks must run first.
    assert!(is_forbidden_ip(&addr("::1")));
    assert!(is_forbidden_ip(&addr("::")));
    assert!(is_forbidden_ip(&addr("fc00::1")));
}

#[test]
fn unspecified_broadcast_reserved_v4_is_forbidden() {
    // 0.0.0.0 routes to loopback on Linux/macOS — must never be allowed.
    assert!(is_forbidden_ip(&addr("0.0.0.0")));
    // The IPv4-mapped form falls into the recursive IPv4 arm: it MUST be
    // forbidden too, otherwise it bypasses the guard as a "public" v6.
    assert!(is_forbidden_ip(&addr("::ffff:0.0.0.0")));
    assert!(is_forbidden_literal_host("[::ffff:0.0.0.0]"));
    assert!(is_forbidden_literal_host("0.0.0.0"));
    // Reserved 240.0.0.0/4 and broadcast.
    assert!(is_forbidden_ip(&addr("240.0.0.1")));
    assert!(is_forbidden_ip(&addr("255.255.255.255")));
}

#[test]
fn translated_and_encapsulated_ranges_are_revalidated_as_ipv4() {
    // Finding R3-1 exact trigger addresses.
    assert!(is_forbidden_ip(&addr("64:ff9b::7f00:1"))); // NAT64 → 127.0.0.1
    assert!(is_forbidden_ip(&addr("2002:7f00:1::"))); // 6to4 → 127.0.0.1
                                                      // Loopback/private/metadata/CGNAT embedded through each range.
    assert!(is_forbidden_ip(&addr("64:ff9b::a9fe:a9fe"))); // 169.254.169.254
    assert!(is_forbidden_ip(&addr("64:ff9b::a00:1"))); // 10.0.0.1
    assert!(is_forbidden_ip(&addr("64:ff9b::6440:1"))); // 100.64.0.1
    assert!(is_forbidden_ip(&addr("64:ff9b::ffff:ffff"))); // broadcast
    assert!(is_forbidden_ip(&addr("2002:a9fe:a9fe::"))); // 169.254.169.254
    assert!(is_forbidden_ip(&addr("2002:a00:1::"))); // 10.0.0.1
}

#[test]
fn teredo_range_fails_closed() {
    // 2001:0000::/32: the client IPv4 is XOR-obfuscated and the relay
    // is attacker-controllable, so the whole range fails closed.
    assert!(is_forbidden_ip(&addr("2001:0::1")));
    assert!(is_forbidden_ip(&addr("2001:0:abcd:ef01:5678:5678:7f00:1")));
}

#[test]
fn public_addresses_near_translation_prefixes_stay_allowed() {
    // Real public addresses remain allowed.
    assert!(!is_forbidden_ip(&addr("2606:4700::1111")));
    assert!(!is_forbidden_ip(&addr("2001:4860:4860::8888")));
    assert!(!is_forbidden_ip(&addr("2620:0:ccc::2")));
    // Public IPv4 embedded through translation ranges remains allowed.
    assert!(!is_forbidden_ip(&addr("64:ff9b::808:808"))); // 8.8.8.8
    assert!(!is_forbidden_ip(&addr("2002:808:808::"))); // 8.8.8.8
                                                        // Prefix look-alikes outside the ranges.
    assert!(!is_forbidden_ip(&addr("2001:db8::1"))); // not 2001:0::/32
    assert!(!is_forbidden_ip(&addr("2002:100:1::"))); // 6to4 → 1.0.0.1 (public)
                                                      // 64:ff9b:1::1 is NOT a look-alike outside the ranges: it is a NAT64
                                                      // Network-Specific Prefix INSIDE 64:ff9b::/32, whose embedded IPv4
                                                      // cannot be located (RFC 6052 §2.2), so it fails closed (#1965, A6).
    assert!(is_forbidden_ip(&addr("64:ff9b:1::1")));
}

#[test]
fn ipv6_unicast_link_local_is_forbidden() {
    // fe80::/10 spans fe80:: .. febf:ffff:... — symmetric with the
    // IPv4 link-local denial (169.254/16).
    assert!(is_forbidden_ip(&addr("fe80::1")));
    assert!(is_forbidden_ip(&addr("febf::ffff")));
    // fec0::/10 (deprecated site-local) is NOT link-local; outside scope.
    assert!(!is_forbidden_ip(&addr("fec0::1")));
}
