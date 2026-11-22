use super::*;
use std::net::SocketAddr;
use wreq::dns::{Name, Resolve};

use super::resolver_env_tests::resolved_addrs;
use crate::domain::network_policy::{CidrBlock, NetworkPolicy, NetworkPolicyMode};

/// Test [`NameSource`] returning a FIXED answer set, whatever the
/// query name. This is the whole point of the seam: a real LAN
/// resolution is not reproducible in CI (machine resolver + routing),
/// so the answer is injected and only the wiring under test varies.
///
/// `Err` mode lets a case prove that a lookup failure propagates
/// unchanged rather than being swallowed by the scan.
#[derive(Debug)]
enum FixedNameSource {
    Answers(Vec<std::net::SocketAddr>),
    Fails(&'static str),
}

impl NameSource for FixedNameSource {
    fn lookup(&self, _host: String) -> self::name_source::LookupFuture {
        let outcome = match self {
            Self::Answers(addrs) => Ok(addrs.clone()),
            Self::Fails(msg) => Err(Box::new(std::io::Error::other(*msg)) as BoxedLookupError),
        };
        Box::pin(async move { outcome })
    }
}

/// Build a resolver whose lookup is fixed, driving the REAL
/// `gai_lookup` → `fail_closed_scan` join with a deterministic answer.
fn resolver_with_answers(
    policy: &NetworkPolicy,
    addrs: Vec<std::net::SocketAddr>,
) -> ValidatingResolver {
    ValidatingResolver {
        validation_enabled: true,
        allow_loopback: false,
        policy: std::sync::Arc::new(policy.clone()),
        name_source: std::sync::Arc::new(FixedNameSource::Answers(addrs)),
    }
}

#[test]
fn fail_closed_scan_rejects_empty_answer_set() {
    // An empty DNS answer can never yield a connectable address:
    // fail closed regardless of which (if any) record was expected.
    let outcome =
        ValidatingResolver::fail_closed_scan("empty.test", &[], false, &NetworkPolicy::default());
    assert!(outcome.is_err(), "empty answer set must fail closed");
}

// B2 slice 2: the AND decision enforced where the hostname AND the
// resolved addresses are both known — `fail_closed_scan`.
/// Allowlist-mode fixture. Its near-twin in
/// `tests/ssrf_policy_e2e_test.rs` deliberately does NOT look like this
/// one: that suite builds its policies through the serde config path
/// instead, which is both more faithful and free of the jscpd clone a
/// shared struct literal would create.
pub(super) fn lan_policy(cidrs: &[&str], hosts: &[&str]) -> NetworkPolicy {
    NetworkPolicy {
        mode: NetworkPolicyMode::Allowlist,
        allow_cidrs: cidrs
            .iter()
            .map(|c| CidrBlock::parse(c).expect("test CIDR parses"))
            .collect(),
        allow_hosts: hosts.iter().map(|h| (*h).to_string()).collect(),
    }
}

fn lan_answer(ip: &str) -> Vec<SocketAddr> {
    vec![SocketAddr::new(ip.parse().expect("test literal"), 8000)]
}

#[test]
fn fail_closed_scan_permits_declared_host_over_allowlisted_network() {
    let net = lan_policy(&["10.0.0.0/8"], &["vllm.lan"]);
    let outcome =
        ValidatingResolver::fail_closed_scan("vllm.lan", &lan_answer("10.0.0.5"), false, &net);
    assert!(
        outcome.is_ok(),
        "declared host over allowlisted LAN resolves"
    );
    // Case and the trailing root dot are the same declared name.
    for host in ["VLLM.LAN", "vllm.lan."] {
        let outcome =
            ValidatingResolver::fail_closed_scan(host, &lan_answer("10.0.0.5"), false, &net);
        assert!(outcome.is_ok(), "{host} is the same declared name");
    }
}

#[test]
fn fail_closed_scan_rejects_undeclared_host_and_names_allow_hosts() {
    let net = lan_policy(&["10.0.0.0/8"], &[]);
    let err =
        ValidatingResolver::fail_closed_scan("vllm.lan", &lan_answer("10.0.0.5"), false, &net)
            .expect_err("an allowlisted network alone is not a permission");
    let rendered = err.to_string();
    assert!(
        rendered.contains("allow_hosts"),
        "the rejection must name the missing config field: {rendered}"
    );
    assert!(
        rendered.contains("vllm.lan") && rendered.contains("10.0.0.5"),
        "the rejection must name both the host and the address: {rendered}"
    );
    // A declared but different host is equally refused.
    let other = lan_policy(&["10.0.0.0/8"], &["otro.lan"]);
    assert!(ValidatingResolver::fail_closed_scan(
        "vllm.lan",
        &lan_answer("10.0.0.5"),
        false,
        &other
    )
    .is_err());
    // Error CLASS is the operator-facing contract: this denial is a
    // missing declaration, not an SSRF attempt.
    assert!(
        err.downcast_ref::<ForbiddenHostNotAllowlistedError>().is_some(),
        "an allowlisted network with an undeclared host must yield the allow_hosts error, got: {err}"
    );
}

#[test]
fn missing_declaration_class_is_distinct_from_the_ssrf_one() {
    let net = lan_policy(&["10.0.0.0/8", "169.254.0.0/16"], &["vllm.lan"]);
    let err = ValidatingResolver::fail_closed_scan(
        "meta.lan",
        &lan_answer("169.254.169.254"),
        false,
        &net,
    )
    .expect_err("metadata is never negotiable");
    assert!(
        err.downcast_ref::<ForbiddenHostNotAllowlistedError>()
            .is_none(),
        "a never-allowable range must NOT be reported as a missing declaration: {err}"
    );
    assert!(
        err.downcast_ref::<ForbiddenResolutionError>().is_some(),
        "it must keep the SSRF class: {err}"
    );
}

#[test]
fn fail_closed_scan_literal_needs_no_allow_hosts() {
    let net = lan_policy(&["10.0.0.0/8"], &[]);
    assert!(
        ValidatingResolver::fail_closed_scan("10.0.0.5", &lan_answer("10.0.0.5"), false, &net)
            .is_ok(),
        "a literal spelling never travels the hostname path"
    );
    assert!(
        ValidatingResolver::fail_closed_scan(
            "10.0.0.5",
            &lan_answer("10.0.0.5"),
            false,
            &NetworkPolicy::default()
        )
        .is_err(),
        "without the CIDR entry the same literal stays denied"
    );
}

#[test]
fn fail_closed_scan_keeps_always_denied_and_loopback_closed() {
    let net = lan_policy(&["169.254.0.0/16", "10.0.0.0/8"], &["meta.lan", "vllm.lan"]);
    assert!(ValidatingResolver::fail_closed_scan(
        "meta.lan",
        &lan_answer("169.254.169.254"),
        false,
        &net
    )
    .is_err());
    assert!(ValidatingResolver::fail_closed_scan(
        "localhost",
        &lan_answer("127.0.0.1"),
        false,
        &net
    )
    .is_err());
    assert!(
        ValidatingResolver::fail_closed_scan("localhost", &lan_answer("127.0.0.1"), true, &net)
            .is_ok(),
        "allow_loopback stays the only way in"
    );
    // An empty policy is today's behavior, verbatim.
    assert!(ValidatingResolver::fail_closed_scan(
        "vllm.lan",
        &lan_answer("10.0.0.5"),
        false,
        &NetworkPolicy::default()
    )
    .is_err());
}

#[test]
fn fail_closed_scan_never_filters_to_a_safe_subset() {
    let net = lan_policy(&["10.0.0.0/8"], &["vllm.lan"]);
    let addrs = vec![
        SocketAddr::new("10.0.0.5".parse().expect("test literal"), 8000),
        SocketAddr::new("169.254.169.254".parse().expect("test literal"), 8000),
    ];
    assert!(
        ValidatingResolver::fail_closed_scan("vllm.lan", &addrs, false, &net).is_err(),
        "one forbidden record poisons the whole answer set"
    );
}

// The `fail_closed_scan` cases above call the scan DIRECTLY. These
// drive the REAL `Resolve::resolve` entry point with a fixed answer
// set, so the join from the lookup seam to the scan is exercised too —
// the half that the isolated cases, by construction, cannot reach.
//
// The seam's absence left a real hole: `gai_lookup` performed the
// lookup inline and its two halves were tested separately, so a bug
// in how it connected them (dropping the answer before scanning it,
// scanning an empty slice, passing the wrong policy) would pass every
// existing test. These rows pin that join.
//
// Nothing here depends on the machine: the answer set is injected, so
// no DNS server, no routing, no `/etc/hosts`.

#[tokio::test]
async fn resolve_admits_a_declared_host_over_an_allowlisted_network() {
    let net = lan_policy(&["10.0.0.0/8"], &["vllm.lan"]);
    let resolver = resolver_with_answers(&net, lan_answer("10.0.0.5"));

    let addrs = resolved_addrs(&resolver, "vllm.lan").await;
    assert_eq!(
        addrs[0].ip().to_string(),
        "10.0.0.5",
        "the allowed answer set must survive the scan and reach the caller"
    );
}

#[tokio::test]
async fn resolve_rejects_an_undeclared_host_with_the_allow_hosts_class() {
    // CIDR declared, hostname NOT: the AND decision made through the
    // real resolve path, and the error CLASS proven to be the
    // actionable one (a missing declaration, not an SSRF attempt).
    let net = lan_policy(&["10.0.0.0/8"], &[]);
    let resolver = resolver_with_answers(&net, lan_answer("10.0.0.5"));

    let err = match resolver.resolve(Name::from("vllm.lan")).await {
        Ok(_) => panic!("an allowlisted network alone is not a permission"),
        Err(err) => err,
    };
    assert!(
        err.downcast_ref::<ForbiddenHostNotAllowlistedError>()
            .is_some(),
        "the resolve path must preserve the allow_hosts class: {err}"
    );
}

#[tokio::test]
async fn resolve_poisons_the_whole_answer_set_through_the_real_path() {
    // Mixed answer set (one allowed, one metadata): the scan must run
    // over the WHOLE set the lookup returned, not the first address.
    let net = lan_policy(&["10.0.0.0/8"], &["vllm.lan"]);
    let resolver = resolver_with_answers(
        &net,
        vec![
            SocketAddr::new("10.0.0.5".parse().expect("test literal"), 8000),
            SocketAddr::new("169.254.169.254".parse().expect("test literal"), 8000),
        ],
    );

    let err = match resolver.resolve(Name::from("vllm.lan")).await {
        Ok(_) => panic!("one forbidden record poisons the resolution"),
        Err(err) => err,
    };
    assert!(
        err.downcast_ref::<ForbiddenResolutionError>().is_some(),
        "a never-allowable record keeps the SSRF class: {err}"
    );
}

#[tokio::test]
async fn resolve_skips_the_scan_when_validation_is_disarmed() {
    // `validation_enabled = false` must short-circuit BEFORE the scan:
    // a forbidden answer resolves anyway. Pins that the seam did not
    // accidentally bypass the gate. (The env-var → flag wiring is
    // pinned separately by `disable_flag_is_read_once_at_construction`;
    // this row sets the flag directly, no env var involved.)
    let net = NetworkPolicy::default();
    let mut resolver = resolver_with_answers(&net, lan_answer("169.254.169.254"));
    resolver.validation_enabled = false;

    let addrs = resolved_addrs(&resolver, "anywhere.example").await;
    assert_eq!(addrs.len(), 1, "disarmed validation returns the answer");
}

#[tokio::test]
async fn resolve_propagates_a_lookup_failure_unchanged() {
    // A lookup error must surface as itself, never be absorbed by a
    // scan over an empty slice (which would mis-report "empty DNS
    // answer" for what is really a resolver failure).
    let resolver = ValidatingResolver {
        validation_enabled: true,
        allow_loopback: false,
        policy: std::sync::Arc::new(NetworkPolicy::default()),
        name_source: std::sync::Arc::new(FixedNameSource::Fails("resolver unavailable")),
    };

    let err = match resolver.resolve(Name::from("vllm.lan")).await {
        Ok(_) => panic!("a lookup failure must not resolve"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("resolver unavailable"),
        "the lookup error must pass through verbatim: {err}"
    );
}
