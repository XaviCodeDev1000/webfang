use super::tests_support::*;
use super::*;

// ---- B2 slice 2: policy-aware entry + resolution verdicts ----

/// Hostname verdict under an allowlist (the AND decision). The literal
/// path is the asymmetry pinned here on purpose: literals never travel
/// the hostname path, so `allow_cidrs` alone opens them.
#[test]
fn literal_host_policy_mirrors_address_policy() {
    let net = hand_built_policy_with_hosts(&["10.0.0.0/8"], &["vllm.lan"]);
    assert!(!is_forbidden_literal_host_with_policy(
        "10.0.0.5", false, &net
    ));
    assert!(is_forbidden_literal_host_with_policy(
        "10.0.0.5",
        false,
        &NetworkPolicy::default()
    ));
    // Hostnames are never literals: they are enforced at resolution.
    assert!(!is_forbidden_literal_host_with_policy(
        "vllm.lan", false, &net
    ));
    assert!(!is_forbidden_literal_host_with_policy(
        "vllm.lan", true, &net
    ));
    // The always-denied set (FIN-017) stays denied through this layer too.
    let metadata = hand_built_policy_with_hosts(&["169.254.0.0/16"], &["meta.lan"]);
    assert!(is_forbidden_literal_host_with_policy(
        "169.254.169.254",
        false,
        &metadata
    ));
    // Alternate spellings of an allowlisted address stay allowed.
    assert!(!is_forbidden_literal_host_with_policy(
        "0xa000005",
        false,
        &net
    ));
    // Loopback stays exclusively under `allow_loopback`.
    assert!(is_forbidden_literal_host_with_policy(
        "127.0.0.1",
        false,
        &net
    ));
    assert!(!is_forbidden_literal_host_with_policy(
        "127.0.0.1",
        true,
        &net
    ));
    // Empty policy is byte-for-byte today's behavior.
    assert!(is_forbidden_literal_host_with_policy(
        "10.0.0.5",
        false,
        &NetworkPolicy::default()
    ));
    assert!(!is_forbidden_literal_host_with_policy(
        "8.8.8.8",
        false,
        &NetworkPolicy::default()
    ));
}

#[test]
fn resolved_address_requires_cidr_and_host_together() {
    let net = hand_built_policy_with_hosts(&["10.0.0.0/8"], &["vllm.lan"]);
    let lan: IpAddr = addr("10.0.0.5");
    // AND: the allowlisted network is the precondition, the declared
    // hostname the permission.
    assert!(!is_forbidden_resolved_address(
        &lan, "vllm.lan", false, &net
    ));
    assert!(is_forbidden_resolved_address(&lan, "otro.lan", false, &net));
    let cidr_only = hand_built_policy_with_hosts(&["10.0.0.0/8"], &[]);
    assert!(is_forbidden_resolved_address(
        &lan, "vllm.lan", false, &cidr_only
    ));
    let hosts_only = hand_built_policy_with_hosts(&[], &["vllm.lan"]);
    assert!(is_forbidden_resolved_address(
        &lan,
        "vllm.lan",
        false,
        &hosts_only
    ));
    // Case and the trailing root dot are the same declared name.
    assert!(!is_forbidden_resolved_address(
        &lan, "VLLM.LAN", false, &net
    ));
    assert!(!is_forbidden_resolved_address(
        &lan,
        "vllm.lan.",
        false,
        &net
    ));
    // Empty policy keeps today's verdict.
    assert!(is_forbidden_resolved_address(
        &lan,
        "vllm.lan",
        false,
        &NetworkPolicy::default()
    ));
    // Public addresses are unaffected by the host gate.
    assert!(!is_forbidden_resolved_address(
        &addr("8.8.8.8"),
        "ejemplo.com",
        false,
        &NetworkPolicy::default()
    ));
}

#[test]
fn literal_spelling_needs_no_allow_hosts_on_the_resolved_path() {
    // Intentional asymmetry: `tokio::net::lookup_host` resolves a literal
    // too, so this is the one path where `allow_cidrs` alone suffices.
    let cidr_only = hand_built_policy_with_hosts(&["10.0.0.0/8"], &[]);
    assert!(!is_forbidden_resolved_address(
        &addr("10.0.0.5"),
        "10.0.0.5",
        false,
        &cidr_only
    ));
    assert!(!is_forbidden_resolved_address(
        &addr("10.0.0.5"),
        "10.1",
        false,
        &cidr_only
    ));
}

#[test]
fn resolved_address_keeps_fin017_and_loopback_closed() {
    let net = hand_built_policy_with_hosts(
        &["169.254.0.0/16", "100.64.0.0/10", "10.0.0.0/8"],
        &["meta.lan", "cgnat.lan", "vllm.lan"],
    );
    assert!(is_forbidden_resolved_address(
        &addr("169.254.169.254"),
        "meta.lan",
        false,
        &net
    ));
    assert!(is_forbidden_resolved_address(
        &addr("::ffff:169.254.169.254"),
        "meta.lan",
        false,
        &net
    ));
    assert!(is_forbidden_resolved_address(
        &addr("127.0.0.1"),
        "vllm.lan",
        false,
        &net
    ));
    assert!(!is_forbidden_resolved_address(
        &addr("127.0.0.1"),
        "vllm.lan",
        true,
        &net
    ));
    // CGNAT needs the explicit opt-in, which it has here.
    assert!(!is_forbidden_resolved_address(
        &addr("100.64.0.5"),
        "cgnat.lan",
        false,
        &net
    ));
    // NAT64/6to4: the verdict follows the embedded IPv4.
    assert!(!is_forbidden_resolved_address(
        &addr("64:ff9b::a00:5"),
        "vllm.lan",
        false,
        &net
    ));
    assert!(is_forbidden_resolved_address(
        &addr("64:ff9b::a9fe:a9fe"),
        "meta.lan",
        false,
        &net
    ));
}

/// A2 (#1965) on the LIVE resolution path: the validating resolver asks
/// this verdict for every DNS answer, so the loopback permit must not
/// travel through the translation forms here either.
#[test]
fn resolved_address_denies_translation_forms_of_loopback_under_the_flag() {
    let net = hand_built_policy_with_hosts(&["10.0.0.0/8"], &["vllm.lan"]);
    assert!(is_forbidden_resolved_address(
        &addr("64:ff9b::7f00:1"),
        "vllm.lan",
        true,
        &net
    ));
    assert!(is_forbidden_resolved_address(
        &addr("2002:7f00:1::"),
        "vllm.lan",
        true,
        &net
    ));
    // The plain loopback stays reachable for the permitted client.
    assert!(!is_forbidden_resolved_address(
        &addr("127.0.0.1"),
        "vllm.lan",
        true,
        &net
    ));
    // The IPv4-mapped form stays denied.
    assert!(is_forbidden_resolved_address(
        &addr("::ffff:127.0.0.1"),
        "vllm.lan",
        true,
        &net
    ));
}

/// A4 (#1965): the config-validation never-allowable table
/// (`network_policy::NEVER_ALLOWABLE`) and the dial-layer always-denied
/// set ([`is_always_denied_ip`]) cover the same ground — the ranges no
/// allowlist may open — and nothing tied them together.
///
/// They are NOT the same set and must never be asserted equal: the two
/// translation prefixes live only in the config table, because their
/// dial-time verdict comes from the embedded IPv4. What is pinned here is
/// the RELATIONSHIP — no range the config layer refuses to allowlist is
/// reachable through the policy verdict path, not even under a blanket
/// allowlist.
#[test]
fn never_allowable_ranges_stay_unreachable_through_the_policy_verdict() {
    // The strongest allowlist a config could carry: everything allowed.
    let blanket = hand_built_policy(&["0.0.0.0/0", "::/0"]);
    for (range_addr, prefix_len, class) in crate::domain::network_policy::NEVER_ALLOWABLE {
        let probe = *range_addr;
        let range = format!("{probe}/{prefix_len} ({class})");
        assert!(
            is_forbidden_ip_with_policy(&probe, false, &blanket),
            "{range} must stay unreachable under a blanket allowlist"
        );
        assert!(
            is_forbidden_ip_with_policy(&probe, true, &blanket),
            "{range} must stay unreachable under a blanket allowlist, even with the loopback flag"
        );
        assert!(
            is_forbidden_resolved_address(&probe, "probe.lan", false, &blanket),
            "{range} must stay unreachable under a blanket allowlist"
        );
        assert!(
            is_forbidden_resolved_address(&probe, "probe.lan", true, &blanket),
            "{range} must stay unreachable under a blanket allowlist, even with the loopback flag"
        );
    }
}

/// A6 (#1965): the NAT64 prefix space is denied as a whole EXCEPT the
/// canonical Well-Known Prefix, whose verdict comes from the embedded
/// IPv4 and therefore stays absent from the dial-layer set on purpose.
///
/// The split is the truth the registry and the dial layer must agree on:
/// a Network-Specific Prefix cannot be decoded (RFC 6052 §2.2 does not
/// encode the embedded IPv4's position), so it fails closed; the
/// canonical /96 can be decoded, so its verdict stays the embedded
/// address's.
#[test]
fn the_dial_layer_denies_the_non_canonical_nat64_space_only() {
    assert!(
        !is_always_denied_ip(&addr("64:ff9b::8.8.8.8")),
        "the canonical Well-Known Prefix must stay absent from the dial-layer set on purpose"
    );
    assert!(
        is_always_denied_ip(&addr("64:ff9b:1::7f00:1")),
        "the non-canonical NAT64 space must be denied by the dial layer"
    );
}

/// A4 (#1965): a translation prefix is never allowable as a blanket, and
/// its verdict IS the embedded IPv4's verdict — computed WITHOUT the
/// loopback permit, so a translation form never reaches something the
/// plain embedded address could not reach under the same policy.
#[test]
fn translation_prefixes_inherit_the_embedded_ipv4_verdict() {
    let lan = hand_built_policy(&["10.0.0.0/8"]);
    for (translation, embedded) in [
        (addr("64:ff9b::7f00:1"), addr("127.0.0.1")),
        (addr("2002:7f00:1::"), addr("127.0.0.1")),
        (addr("64:ff9b::a00:5"), addr("10.0.0.5")),
        (addr("2002:a00:5::"), addr("10.0.0.5")),
        (addr("64:ff9b::a9fe:a9fe"), addr("169.254.169.254")),
        (addr("2002:a9fe:a9fe::"), addr("169.254.169.254")),
    ] {
        for allow_loopback in [false, true] {
            assert_eq!(
                is_forbidden_ip_with_policy(&translation, allow_loopback, &lan),
                is_forbidden_ip_with_policy(&embedded, false, &lan),
                "{translation} must inherit the {embedded} verdict (allow_loopback={allow_loopback})"
            );
        }
    }
}
