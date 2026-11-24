//! Unit tests for the ADR-0021 domain types. They live in this submodule
//! (same crate, `use super::*`) so the assertions reach the private surface
//! exactly like an in-file `mod tests` would, keeping `proxy_config.rs` under
//! the 500-line file ceiling.

use super::*;
use crate::domain::network_policy::NetworkPolicyMode;

/// A restricted-policy definition builder: the base every validation test
/// mutates one field of.
fn definition(url: &str) -> ProxyDefinition {
    ProxyDefinition {
        url: Url::parse(url).expect("test url"),
        allow_loopback: false,
        network_policy: NetworkPolicy::default(),
        auth: None,
    }
}

#[test]
fn catalog_entry_parses_with_defaults() {
    let parsed: ProxyConfig =
        serde_json::from_str(r#"{"id": "corp-egress", "url": "http://proxy.corp.internal:3128"}"#)
            .expect("minimal entry must parse");
    assert_eq!(parsed.id, "corp-egress");
    assert!(!parsed.definition.allow_loopback);
    assert_eq!(parsed.definition.network_policy, NetworkPolicy::default());
    assert!(parsed.definition.auth.is_none());
}

#[test]
fn catalog_entry_with_allowlist_and_auth_parses() {
    let parsed: ProxyConfig = serde_json::from_str(
        r#"{
                "id": "corp-egress",
                "url": "http://proxy.corp.internal:3128",
                "network_policy": {"mode": "allowlist", "allow_cidrs": ["10.20.0.0/16"]},
                "auth": {"user": "svc-webfang", "password": {"source": "encrypted_file", "path": "/tmp/proxy.age"}}
            }"#,
    )
    .expect("full entry must parse");
    assert_eq!(
        parsed.definition.network_policy.mode,
        NetworkPolicyMode::Allowlist
    );
    let auth = parsed.definition.auth.expect("auth declared");
    assert_eq!(auth.user, "svc-webfang");
}

#[test]
fn spec_accepts_named_string_and_inline_table() {
    let named: ProxySpec = serde_json::from_str(r#""corp-egress""#).expect("string form");
    assert!(matches!(named, ProxySpec::Named(id) if id == "corp-egress"));
    let inline: ProxySpec = serde_json::from_str(
        r#"{"url": "http://proxy.corp.internal:3128", "allow_loopback": true}"#,
    )
    .expect("table form");
    match inline {
        ProxySpec::Inline(definition) => {
            assert!(definition.allow_loopback);
            assert_eq!(definition.url.host_str(), Some("proxy.corp.internal"));
        },
        other => panic!("expected inline form, got: {other:?}"),
    }
}

#[test]
fn provider_proxy_explicit_null_is_deserialization_error() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["embedding"],"proxy":null}"#;
    let err = serde_json::from_str::<ProviderConfig>(raw)
        .expect_err("explicit null on provider.proxy must fail loud");
    assert!(
        err.to_string().contains("proxy"),
        "message must name the field, got: {err}"
    );
}

#[test]
fn provider_without_proxy_field_defaults_to_direct_egress() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["embedding"]}"#;
    let parsed: ProviderConfig = serde_json::from_str(raw).expect("absent proxy is direct");
    assert!(parsed.proxy.is_none());
}

#[test]
fn socks_schemes_are_rejected_with_the_v1_diagnostic() {
    for url in ["socks5://proxy.corp:1080", "socks5h://proxy.corp:1080"] {
        let err = ValidatedProxy::try_inline(&definition(url))
            .expect_err("socks is outside the v1 surface");
        assert!(
            matches!(err, ProxyConfigError::UnsupportedScheme { ref scheme, .. } if scheme.starts_with("socks")),
            "got: {err}"
        );
        assert!(
            err.to_string().contains("http:// and https://"),
            "diagnostic must name the supported surface, got: {err}"
        );
    }
}

#[test]
fn userinfo_url_is_rejected_pointing_at_proxy_auth() {
    let err = ValidatedProxy::try_inline(&definition("http://user:secret@proxy.corp:3128"))
        .expect_err("inline credentials must be rejected");
    assert!(
        matches!(err, ProxyConfigError::UserInfoNotAllowed { .. }),
        "got: {err}"
    );
    assert!(
        err.to_string().contains("proxy.auth"),
        "diagnostic must point at proxy.auth, got: {err}"
    );
}

#[test]
fn non_origin_url_is_rejected() {
    for url in [
        "http://proxy.corp:3128/admin",
        "http://proxy.corp:3128?x=1",
        "http://proxy.corp:3128#frag",
    ] {
        let err =
            ValidatedProxy::try_inline(&definition(url)).expect_err("origin-only requirement");
        assert!(
            matches!(err, ProxyConfigError::OriginOnly { .. }),
            "got: {err}"
        );
    }
}

#[test]
fn rfc1918_proxy_is_rejected_without_the_proxy_opt_in() {
    let err = ValidatedProxy::try_inline(&definition("http://10.20.30.40:3128"))
        .expect_err("private literal proxy without opt-in must fail pre-socket");
    assert!(
        matches!(err, ProxyConfigError::ForbiddenProxyTarget { .. }),
        "got: {err}"
    );
}

#[test]
fn allowlisted_rfc1918_proxy_is_accepted() {
    let mut def = definition("http://10.20.30.40:3128");
    def.network_policy = NetworkPolicy {
        mode: NetworkPolicyMode::Allowlist,
        allow_cidrs: vec![
            crate::domain::network_policy::CidrBlock::parse("10.20.0.0/16").expect("cidr"),
        ],
        allow_hosts: vec!["proxy.corp.internal".to_string()],
    };
    let proxy = ValidatedProxy::try_inline(&def).expect("the corporate case must validate");
    assert_eq!(proxy.origin().host_str(), Some("10.20.30.40"));
    assert_eq!(proxy.origin().port(), Some(3128));
    assert!(proxy.proxy_id().is_none());
}

#[test]
fn never_allowable_proxy_is_denied_even_when_allowlisted() {
    let mut def = definition("http://169.254.169.254:3128");
    def.network_policy = NetworkPolicy {
        mode: NetworkPolicyMode::Allowlist,
        allow_cidrs: vec![
            crate::domain::network_policy::CidrBlock::parse("169.254.0.0/16").expect("cidr"),
        ],
        allow_hosts: Vec::new(),
    };
    let err = ValidatedProxy::try_inline(&def)
        .expect_err("FIN-017 stays denied even for an allowlisted proxy");
    assert!(
        matches!(err, ProxyConfigError::ForbiddenProxyTarget { .. }),
        "got: {err}"
    );
}

#[test]
fn loopback_proxy_requires_the_proxy_opt_in() {
    let err = ValidatedProxy::try_inline(&definition("http://127.0.0.1:8080"))
        .expect_err("loopback proxy without opt-in must fail");
    assert!(
        matches!(err, ProxyConfigError::ForbiddenProxyTarget { .. }),
        "got: {err}"
    );
    let mut def = definition("http://127.0.0.1:8080");
    def.allow_loopback = true;
    let proxy = ValidatedProxy::try_inline(&def).expect("opt-in permits the loopback proxy");
    assert_eq!(proxy.origin().port(), Some(8080));
}

#[test]
fn registry_rejects_duplicate_ids() {
    let entry = |id: &str| {
        serde_json::from_str::<ProxyConfig>(
            format!(r#"{{"id": "{id}", "url": "http://proxy.corp:3128"}}"#).as_str(),
        )
        .expect("entry")
    };
    let err = ProxyRegistry::try_new(vec![entry("a"), entry("b"), entry("a")])
        .expect_err("duplicate ids must fail loud");
    assert!(
        matches!(err, ProxyConfigError::DuplicateProxyId { ref id } if id == "a"),
        "got: {err}"
    );
}

#[test]
fn registry_resolves_named_inline_and_unknown() {
    let entry: ProxyConfig =
        serde_json::from_str(r#"{"id": "corp", "url": "http://proxy.corp.internal:3128"}"#)
            .expect("entry");
    let registry = ProxyRegistry::try_new(vec![entry]).expect("unique ids");
    let named = registry
        .resolve(&ProxySpec::Named("corp".to_string()))
        .expect("declared id resolves");
    assert_eq!(named.proxy_id(), Some("corp"));
    let inline = registry
        .resolve(&ProxySpec::Inline(definition("http://other.corp:3128")))
        .expect("inline resolves without the catalog");
    assert!(inline.proxy_id().is_none());
    let err = registry
        .resolve(&ProxySpec::Named("missing".to_string()))
        .expect_err("undeclared id must fail loud");
    assert!(
        matches!(err, ProxyConfigError::UnknownProxyReference { .. }),
        "got: {err}"
    );
}

#[test]
fn endpoint_policy_mirrors_the_definition_and_the_provider() {
    let mut def = definition("http://proxy.corp:3128");
    def.allow_loopback = true;
    let proxy = ValidatedProxy::try_inline(&def).expect("validated");
    let policy = proxy.endpoint_policy();
    assert!(policy.allow_loopback);
    assert_eq!(policy.network_policy, NetworkPolicy::default());

    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["embedding"],"allow_loopback":true}"#;
    let provider: ProviderConfig = serde_json::from_str(raw).expect("provider");
    let destination = EndpointPolicy::from(&provider);
    assert!(destination.allow_loopback);
    assert_eq!(destination.network_policy, provider.network_policy);
}

#[test]
fn profile_pairing_sets_the_proxy_hop() {
    let destination = EndpointPolicy::from(&crate::domain::providers::ProviderConfig {
        id: "p".to_string(),
        display_name: "p".to_string(),
        kind: crate::domain::providers::ProviderKind::OpenAiCompatible,
        base_url: Url::parse("https://api.example.com/v1").expect("url"),
        auth: crate::domain::auth_source::AuthSource::None,
        capabilities: vec![],
        model: None,
        embedding_dim: None,
        allow_loopback: false,
        network_policy: NetworkPolicy::default(),
        proxy: None,
    });
    let direct = ClientSecurityProfile::direct(destination.clone());
    assert!(direct.via_proxy.is_none());

    let proxy =
        ValidatedProxy::try_inline(&definition("http://proxy.corp.internal:3128")).expect("ok");
    let proxied = ClientSecurityProfile::through_proxy(destination, &proxy);
    assert!(proxied.via_proxy.is_some());
    assert!(!proxied.destination.allow_loopback);
}
