use super::tests_support::*;
use super::*;

#[test]
fn redirect_literal_blocked_honors_the_allowlist() {
    let net = hand_built_policy_with_hosts(&["10.0.0.0/8"], &["vllm.lan"]);
    assert!(!redirect_literal_blocked_with_policy(
        Some("10.0.0.5"),
        false,
        &net
    ));
    assert!(redirect_literal_blocked_with_policy(
        Some("10.0.0.5"),
        false,
        &NetworkPolicy::default()
    ));
    // Hostname hops are the resolver's business, never this sync guard.
    assert!(!redirect_literal_blocked_with_policy(
        Some("vllm.lan"),
        false,
        &NetworkPolicy::default()
    ));
    assert!(!redirect_literal_blocked_with_policy(None, false, &net));
    assert!(redirect_literal_blocked_with_policy(
        Some("169.254.169.254"),
        false,
        &hand_built_policy_with_hosts(&["169.254.0.0/16"], &[])
    ));
}

#[test]
fn disable_value_tests() {
    for (raw, expected) in [
        (None, false),
        (Some("1"), true),
        (Some("0"), false),
        (Some("false"), false),
        (Some("yes"), false),
        (Some("TRUE"), false),
        (Some(""), false),
        (Some(" 1"), false),
        (Some("1 "), false),
        (Some(" 1 "), false),
        (Some("\t1\n"), false),
    ] {
        assert_eq!(disables_ssrf(raw), expected, "raw value: {raw:?}");
    }
}

/// The entry hatch is a value-equality check, and its second half is the
/// CLI-side fact behind #1294 P6-3: `"1"` really does put a forbidden literal
/// on the wire, which is exactly what `tests/common/cli_harness.rs` relies on
/// to drive `127.0.0.1` mocks. MCP's own switch has that same scope for the
/// literals its pre-check stops covering — pinned by
/// `mcp_entry_env_has_the_same_literal_scope_as_the_cli_entry_env` in
/// `crates/webfang_mcp/tests/mcp_ssrf_knob_matrix_test.rs`. Change either
/// scope and both suites have to move together.
#[test]
fn entry_guard_hatch_requires_exact_value_one() {
    let url: url::Url = "http://127.0.0.1/".parse().expect("test URL parses");
    {
        let _guard = webfang_test_utils::EnvGuard::with(&[(DISABLE_ENTRY_GUARD_ENV, "0")]);
        assert!(
            reject_forbidden_literal_url(&url).is_err(),
            "WEBFANG_DISABLE_SSRF_ENTRY_GUARD=0 must keep validation active"
        );
    }
    {
        let _guard = webfang_test_utils::EnvGuard::with(&[(DISABLE_ENTRY_GUARD_ENV, "1")]);
        assert!(
            reject_forbidden_literal_url(&url).is_ok(),
            "exact \"1\" disarms the entry guard for wiremock harnesses"
        );
    }
}

// Registry semantics (new in sub-slice 3.C). The registry is process-global
// and unresettable, so no test may assume it starts unarmed: nextest gives
// each test its own process, but plain `cargo test` (the Coverage job)
// shares one process, where a sibling that builds a Container arms it
// first. Each registry test therefore captures the winner after its own arm
// and asserts only the order- and race-independent invariant — once armed,
// no later set replaces it (#996).
// NoopGuard proves the sealed trait object is dyn-safe. Never arm it in
// the registry: a no-op there disarms every later client (#2626).
struct NoopGuard;
impl sealed::Sealed for NoopGuard {}
impl SsrfGuard for NoopGuard {
    fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder {
        builder
    }

    fn secure_client_with_loopback(
        &self,
        builder: wreq::ClientBuilder,
        _allow_loopback: bool,
    ) -> wreq::ClientBuilder {
        builder
    }

    fn secure_client_with_policy(
        &self,
        builder: wreq::ClientBuilder,
        _allow_loopback: bool,
        _policy: &crate::domain::network_policy::NetworkPolicy,
    ) -> wreq::ClientBuilder {
        builder
    }
}

#[test]
fn ssrf_guard_port_is_object_safe_via_sealed() {
    fn assert_dyn(_: &dyn SsrfGuard) {}
    assert_dyn(&NoopGuard);
}

#[test]
fn registry_is_keep_first() {
    // Arm the real guard, not a no-op: this registry is process-global,
    // so a sentinel here would disarm every later client (#2626).
    set_ssrf_guard(Arc::new(DefaultSsrfGuard) as Arc<dyn SsrfGuard>);
    // Read after our own arm, so this is a registry value and never the
    // fallback: our guard when this test armed an unarmed registry, or a
    // sibling's guard in a shared `cargo test` process.
    let winner = ssrf_guard();
    set_ssrf_guard(Arc::new(DefaultSsrfGuard) as Arc<dyn SsrfGuard>);
    assert!(
        Arc::ptr_eq(&ssrf_guard(), &winner),
        "a later set must not replace the already-armed guard (#996)"
    );
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]
fn registry_fallback_is_self_sufficient() {
    // Under a shared `cargo test` process a sibling may have armed the
    // registry first; either way the accessor must yield a guard that
    // can build a client.
    let guard = ssrf_guard();
    let client = guard.secure_client(wreq::Client::builder()).build();
    assert!(
        client.is_ok(),
        "unarmed accessor must yield a guard able to build a client"
    );
}

// #1462 (U2): parameterized loopback permit — a per-client parameter,
// never registry state. Only the exact loopback addresses open; every
// other range stays denied even with the flag set.
#[test]
fn loopback_permit_opens_exact_loopback_only_with_flag() {
    assert!(!is_forbidden_ip_with(&addr("127.0.0.1"), true));
    assert!(!is_forbidden_ip_with(&addr("::1"), true));
    assert!(is_forbidden_ip_with(&addr("127.0.0.1"), false));
    assert!(is_forbidden_ip_with(&addr("::1"), false));
}

#[test]
fn loopback_permit_never_opens_other_ranges() {
    for host in [
        "127.0.0.2", // loopback range, but not the permitted address
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "0.0.0.0",
        "100.64.1.1",
        "::ffff:127.0.0.1", // mapped form is not the ::1 permit
        "fe80::1",
    ] {
        assert!(
            is_forbidden_ip_with(&addr(host), true),
            "{host} must stay forbidden even with allow_loopback"
        );
    }
    assert!(!is_forbidden_ip_with(&addr("8.8.8.8"), true));
    assert!(!is_forbidden_ip_with(&addr("8.8.8.8"), false));
}

#[test]
fn literal_host_permit_mirrors_address_permit() {
    assert!(!is_forbidden_literal_host_with("127.0.0.1", true));
    assert!(!is_forbidden_literal_host_with("[::1]", true));
    // Alternate spellings of the same permitted address stay permitted.
    assert!(!is_forbidden_literal_host_with("0x7f000001", true));
    assert!(is_forbidden_literal_host_with("127.0.0.1", false));
    assert!(is_forbidden_literal_host_with("127.0.0.2", true));
    assert!(is_forbidden_literal_host_with("10.0.0.1", true));
    assert!(!is_forbidden_literal_host_with("example.com", true));
}

#[test]
fn redirect_literal_blocked_honors_the_flag() {
    assert!(redirect_literal_blocked(Some("127.0.0.1"), false));
    assert!(!redirect_literal_blocked(Some("127.0.0.1"), true));
    assert!(!redirect_literal_blocked(Some("[::1]"), true));
    assert!(redirect_literal_blocked(Some("10.0.0.1"), true));
    assert!(!redirect_literal_blocked(Some("example.com"), false));
    assert!(!redirect_literal_blocked(None, false));
}
