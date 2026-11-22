use super::*;
use std::net::SocketAddr;
use wreq::dns::{Name, Resolve};

fn validation_on() -> (webfang_test_utils::EnvGuard, ValidatingResolver) {
    let guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
    (guard, ValidatingResolver::new())
}

pub(super) async fn resolved_addrs(resolver: &ValidatingResolver, host: &str) -> Vec<SocketAddr> {
    resolver
        .resolve(Name::from(host))
        .await
        .expect("resolution succeeds")
        .collect()
}

/// Shared loopback-resolution proof (#1462): resolving the
/// `127.0.0.1` literal must yield exactly that address — used by
/// both the env-hatch bypass test and the per-client permit test so
/// the two mirrors cannot drift.
async fn assert_resolves_loopback_literal(resolver: &ValidatingResolver) {
    let addrs = resolved_addrs(resolver, "127.0.0.1").await;
    assert_eq!(addrs[0].ip().to_string(), "127.0.0.1");
}

#[tokio::test]
async fn forbidden_ipv4_literal_hostname_is_rejected() {
    let (_guard, resolver) = validation_on();

    for host in ["127.0.0.1", "169.254.169.254", "0.0.0.0"] {
        let outcome = resolver.resolve(Name::from(host)).await;
        assert!(
            outcome.is_err(),
            "forbidden literal must be rejected at connect time: {host}"
        );
    }
}

// #1462 (U2): per-client loopback permit. The flag travels with the
// resolver instance — never through the keep-first process registry.
#[tokio::test]
async fn allow_loopback_permits_loopback_literal_resolution() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
    let resolver = ValidatingResolver::with_allow_loopback(true);

    assert_resolves_loopback_literal(&resolver).await;
}

#[tokio::test]
async fn allow_loopback_still_rejects_private_literals() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
    let resolver = ValidatingResolver::with_allow_loopback(true);

    for host in ["10.0.0.1", "127.0.0.2", "0.0.0.0"] {
        let outcome = resolver.resolve(Name::from(host)).await;
        assert!(
            outcome.is_err(),
            "{host} must stay rejected with allow_loopback"
        );
    }
}

#[tokio::test]
async fn forbidden_ipv4_mapped_ipv6_is_rejected() {
    let (_guard, resolver) = validation_on();

    let outcome = resolver.resolve(Name::from("::ffff:127.0.0.1")).await;
    assert!(
        outcome.is_err(),
        "IPv4-mapped loopback must not bypass the validating resolver"
    );
}

#[tokio::test]
async fn public_ip_literals_pass_through() {
    let (_guard, resolver) = validation_on();

    let v4 = resolved_addrs(&resolver, "8.8.8.8").await;
    assert_eq!(v4.len(), 1, "literal resolution yields one address");
    assert_eq!(v4[0].ip().to_string(), "8.8.8.8");

    let v6 = resolved_addrs(&resolver, "2606:4700::1111").await;
    assert_eq!(v6.len(), 1);
    assert_eq!(v6[0].ip().to_string(), "2606:4700::1111");
}

#[tokio::test]
async fn rejection_error_names_the_offending_host_and_ip() {
    let (_guard, resolver) = validation_on();

    let err = match resolver.resolve(Name::from("127.0.0.1")).await {
        Err(err) => err,
        Ok(_) => panic!("loopback must be rejected"),
    };
    let rendered = err.to_string();
    assert!(
        rendered.contains("127.0.0.1"),
        "error must identify both host and forbidden ip: {rendered}"
    );
}

#[tokio::test]
async fn env_bypass_allows_loopback_resolution() {
    let _guard = webfang_test_utils::EnvGuard::with(&[(DISABLE_VALIDATING_RESOLVER_ENV, "1")]);
    let resolver = ValidatingResolver::new();

    assert_resolves_loopback_literal(&resolver).await;
}

#[tokio::test]
async fn bypass_requires_exact_value_one() {
    // Any value other than the literal "1" (including "0", "true",
    // "yes") must NOT disarm the guard.
    let _guard = webfang_test_utils::EnvGuard::with(&[(DISABLE_VALIDATING_RESOLVER_ENV, "0")]);
    let resolver = ValidatingResolver::new();

    let outcome = resolver.resolve(Name::from("127.0.0.1")).await;
    assert!(
        outcome.is_err(),
        r#"WEBFANG_DISABLE_SSRF_RESOLVER=0 must keep validation active"#
    );
}

#[tokio::test]
async fn disable_flag_is_read_once_at_construction() {
    // Construct with validation ON, then flip the escape hatch: the
    // already-built resolver must keep enforcing (the flag is read
    // once, at construction time, so long-lived clients cannot be
    // disarmed mid-flight by an env mutation).
    let (mut guard, resolver) = validation_on();
    // Flip the escape hatch while the guard already holds ENV_LOCK —
    // `set` mutates under the held lock and restores on drop, so no
    // raw `set_var` is needed here (issue #1126).
    guard.set(DISABLE_VALIDATING_RESOLVER_ENV, "1");

    let outcome = resolver.resolve(Name::from("127.0.0.1")).await;
    assert!(
        outcome.is_err(),
        "flag captured at construction must keep validation active"
    );
}
