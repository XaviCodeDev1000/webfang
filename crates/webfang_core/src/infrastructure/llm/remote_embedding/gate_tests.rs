use super::super::fixtures::{config_with, LAN_HOST, LAN_PORT};
use super::super::render_error_chain;
use super::*;
use crate::domain::auth_source::AuthSource;
use crate::domain::network_policy::NetworkPolicy;
use url::Url;

fn lan_policy(cidrs: &[&str], hosts: &[&str]) -> NetworkPolicy {
    NetworkPolicy {
        mode: crate::domain::network_policy::NetworkPolicyMode::Allowlist,
        allow_cidrs: cidrs
            .iter()
            .map(|c| crate::domain::network_policy::CidrBlock::parse(c).expect("test CIDR parses"))
            .collect(),
        allow_hosts: hosts.iter().map(|h| (*h).to_string()).collect(),
    }
}

#[test]
fn entry_gate_accepts_allowlisted_lan_literal() {
    let mut cfg = config_with(AuthSource::None);
    cfg.base_url = Url::parse(&format!("http://{LAN_HOST}:{LAN_PORT}/v1")).expect("url");
    cfg.network_policy = lan_policy(&["10.0.0.0/8"], &[]);
    entry_gate(&cfg).expect("an allowlisted LAN literal must pass the entry gate");
}

#[test]
fn entry_gate_rejects_undeclared_lan_literal_naming_the_fields() {
    let mut cfg = config_with(AuthSource::None);
    cfg.base_url = Url::parse(&format!("http://{LAN_HOST}:{LAN_PORT}/v1")).expect("url");
    cfg.network_policy = NetworkPolicy::default();
    let err = entry_gate(&cfg).expect_err("an undeclared LAN literal stays forbidden");
    let rendered = err.to_string();
    assert!(
        rendered.contains("allow_loopback") && rendered.contains("allow_cidrs"),
        "the rejection must name both config fields that can open it: {rendered}"
    );
}

#[test]
fn entry_gate_keeps_cloud_metadata_denied_under_any_policy() {
    // FIN-017 invariant 1: a hand-built policy that bypassed config
    // validation still cannot open the metadata endpoint, and the
    // message says why instead of pointing at a field that would not help.
    let mut cfg = config_with(AuthSource::None);
    cfg.base_url = Url::parse("http://169.254.169.254/v1").expect("url");
    cfg.network_policy = lan_policy(&["169.254.0.0/16"], &[]);
    let err = entry_gate(&cfg).expect_err("metadata stays denied");
    let rendered = err.to_string();
    assert!(
        !rendered.contains("allow_cidrs"),
        "the message must not point at a field that cannot permit this range: {rendered}"
    );
}

#[test]
fn entry_gate_leaves_hostnames_to_the_resolver() {
    let mut cfg = config_with(AuthSource::None);
    cfg.base_url = Url::parse("http://vllm.lan:8000/v1").expect("url");
    cfg.network_policy = lan_policy(&["10.0.0.0/8"], &["vllm.lan"]);
    entry_gate(&cfg).expect("hostnames are enforced per connection, not here");
}

#[test]
fn transport_error_surfaces_the_resolver_diagnosis() {
    // The resolver's Spanish, field-naming diagnosis reaches the operator
    // through the transport error: it lives in the source chain, and the
    // top-level Display does not carry it (measured — the chain is
    // "error sending request … → client error (Connect) → dns error →
    // <our message>").
    #[derive(Debug, thiserror::Error)]
    #[error("error sending request for uri (http://vllm.lan:8000/v1/embeddings): client error (Connect)")]
    struct Transport {
        #[source]
        source: Denied,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("el host 'vllm.lan' resolvió a una red privada no declarada en allow_hosts")]
    struct Denied;

    let rendered = render_error_chain(&Transport { source: Denied });
    assert!(
        rendered.starts_with("error sending request") && rendered.contains("allow_hosts"),
        "the rendered chain must carry the diagnosis: {rendered}"
    );
    // A flat error renders as itself: no dangling separators.
    assert_eq!(render_error_chain(&Denied), Denied.to_string());
}
