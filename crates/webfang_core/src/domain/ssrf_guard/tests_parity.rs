use super::*;
use crate::domain::ssrf_guard::tests_support::*;
use crate::domain::ssrf_guard::{
    is_always_denied_ip, reject_forbidden_literal_url, seed_guard_refusal,
};

/// A4 (#1965), second half: the dial-layer always-denied set covers every
/// never-allowable range EXCEPT the translation prefixes whose verdict
/// derives from the embedded IPv4, which stay absent on purpose. The
/// asymmetry is pinned here so a future unification of the two tables
/// breaks loudly instead of silently changing a verdict.
///
/// A6 (#1965) split the NAT64 class in two: the registry entry covers the
/// whole prefix space 64:ff9b::/32, whose canonical Well-Known Prefix
/// keeps the embedded-IPv4 verdict (absent from the dial-layer set on
/// purpose) while the Network-Specific Prefix space cannot be decoded and
/// is therefore denied BY the dial layer. The expectation is per class,
/// not per probe address, so neither half can drift silently.
#[test]
fn the_dial_layer_always_denied_set_covers_every_never_allowable_range() {
    for (range_addr, prefix_len, class) in crate::domain::network_policy::NEVER_ALLOWABLE {
        let probe = *range_addr;
        if let IpAddr::V6(v6) = probe {
            if is_ipv6_nat64_prefix_space(&v6) {
                // The canonical Well-Known Prefix keeps the embedded-IPv4
                // verdict: absent from the dial-layer set on purpose.
                assert!(
                    !is_always_denied_ip(&probe),
                    "{class} ({probe}/{prefix_len}): the canonical NAT64 Well-Known Prefix must stay absent from the dial-layer set on purpose"
                );
                // The rest of the prefix space is denied by the dial layer.
                assert!(
                    is_always_denied_ip(&addr("64:ff9b:1::7f00:1")),
                    "{class} ({probe}/{prefix_len}): the non-canonical NAT64 space must be denied by the dial layer"
                );
                continue;
            }
            if ipv6_nat64_embedded_v4(&v6).is_some() || ipv6_6to4_embedded_v4(&v6).is_some() {
                assert!(
                    !is_always_denied_ip(&probe),
                    "{class} ({probe}/{prefix_len}): the translation prefixes stay absent from the dial-layer set on purpose"
                );
                continue;
            }
        }
        assert!(
            is_always_denied_ip(&probe),
            "{class} ({probe}/{prefix_len}): the dial-layer always-denied set must cover it"
        );
    }
}

/// The pure verdict `#1381` diagnoses with must never drift from the guard that
/// enforces. Same deny list, same host parser, same exact-`"1"` hatch — a
/// diagnostic that disagrees with the enforcement is worse than no diagnostic,
/// because it points an operator at a layer that was never armed.
#[test]
fn seed_guard_refusal_agrees_with_the_enforcing_guard_on_every_form() {
    for raw in [
        "http://127.0.0.1/",
        "http://0x7f000001:18888/article",
        "http://2130706433:18888/article",
        "http://127.1:18888/article",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/",
        "http://[::ffff:127.0.0.1]/",
        "https://example.com/",
        "http://8.8.8.8/",
        "http://93.184.216.34/",
        "http://[2606:4700::1111]/",
    ] {
        let url: url::Url = raw.parse().expect("test URLs always parse");

        // One EnvGuard at a time: ENV_LOCK is not reentrant (#1126).
        {
            let _armed = webfang_test_utils::EnvGuard::clean(&[DISABLE_ENTRY_GUARD_ENV]);
            assert_eq!(
                seed_guard_refusal(&url).is_some(),
                reject_forbidden_literal_url(&url).is_err(),
                "guards-armed verdict must match enforcement for {raw}"
            );
        }
        {
            let _hatched = webfang_test_utils::EnvGuard::with(&[(DISABLE_ENTRY_GUARD_ENV, "1")]);
            assert!(
                seed_guard_refusal(&url).is_none(),
                "hatched verdict must stay silent for {raw}"
            );
            assert!(
                reject_forbidden_literal_url(&url).is_ok(),
                "hatched enforcement must stay silent for {raw}"
            );
        }
    }
}
