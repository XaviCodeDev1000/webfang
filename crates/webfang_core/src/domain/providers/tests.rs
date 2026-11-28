use super::*;

fn provider(id: &str, capabilities: Vec<Capability>) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        display_name: id.to_string(),
        auth: AuthSource::Env {
            var: "WEBFANG_TEST_KEY".to_string(),
        },
        capabilities,
        model: Some("gpt-test".to_string()),
        ..super::test_provider_config()
    }
}

#[test]
fn extra_body_rejects_webfang_owned_keys_at_deserialize() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["embedding"],"extra_body":{"model":"attacker"}}"#;
    let err = serde_json::from_str::<ProviderConfig>(raw)
        .expect_err("a reserved body key must fail at config load, before any dial");
    assert!(
        err.to_string().contains("model"),
        "the error must name the reserved key, got: {err}"
    );
}

#[test]
fn test_provider_config_is_a_valid_minimal_fixture() {
    let cfg = super::test_provider_config();
    assert_eq!(cfg.kind, ProviderKind::OpenAiCompatible);
    assert_eq!(cfg.base_url.as_str(), "https://api.example.com/v1");
    assert_eq!(cfg.embedding_dim, None);
    assert!(!cfg.allow_loopback);
    assert!(cfg.proxy.is_none());
    assert!(cfg.extra_body.is_empty());
    assert!(cfg.query_extra_body.is_empty());
    assert!(!cfg.send_encoding_format);
    assert_eq!(cfg.response_format_mode, ResponseFormatMode::JsonObject);
    assert_eq!(cfg.max_tokens_parameter, MaxTokensParameter::MaxTokens);
}

#[test]
fn encoding_format_is_reserved_even_when_the_flag_is_off() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["embedding"],"send_encoding_format":false,"extra_body":{"encoding_format":"base64"}}"#;
    let err = serde_json::from_str::<ProviderConfig>(raw)
        .expect_err("a reserved key must fail even when the flag disables the field");
    assert!(
        err.to_string().contains("encoding_format"),
        "the error must name the reserved key, got: {err}"
    );
}

#[test]
fn send_encoding_format_defaults_to_false() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["embedding"]}"#;
    let provider: ProviderConfig = serde_json::from_str(raw).expect("absent flag parses");
    assert!(!provider.send_encoding_format);
}

#[test]
fn response_format_mode_defaults_to_json_object() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["completion"]}"#;
    let provider: ProviderConfig = serde_json::from_str(raw).expect("absent option parses");
    assert_eq!(
        provider.response_format_mode,
        ResponseFormatMode::JsonObject,
        "the default must keep today's payload (#3092)"
    );
}

#[test]
fn max_tokens_parameter_defaults_to_max_tokens() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["completion"]}"#;
    let provider: ProviderConfig = serde_json::from_str(raw).expect("absent option parses");
    assert_eq!(
        provider.max_tokens_parameter,
        MaxTokensParameter::MaxTokens,
        "the default must keep today's payload (#3092)"
    );
}

#[test]
fn response_format_mode_rejects_an_unknown_value() {
    // `json_schema` is deliberately out of scope: it needs a schema object on
    // the wire, not one more enum variant (#3092).
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["completion"],"response_format_mode":"json_schema"}"#;
    let err = serde_json::from_str::<ProviderConfig>(raw)
        .expect_err("an unknown mode must fail at config load, before any dial");
    let msg = err.to_string();
    assert!(
        msg.contains("json_schema"),
        "the error must name the rejected value, got: {msg}"
    );
    assert!(
        msg.contains("json_object") && msg.contains("none"),
        "the error must name the accepted values, got: {msg}"
    );
}

#[test]
fn max_tokens_parameter_rejects_an_unknown_value() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["completion"],"max_tokens_parameter":"max_new_tokens"}"#;
    let err = serde_json::from_str::<ProviderConfig>(raw)
        .expect_err("an unknown parameter must fail at config load, before any dial");
    let msg = err.to_string();
    assert!(
        msg.contains("max_new_tokens"),
        "the error must name the rejected value, got: {msg}"
    );
    assert!(
        msg.contains("max_tokens") && msg.contains("max_completion_tokens"),
        "the error must name the accepted values, got: {msg}"
    );
}

#[test]
fn extra_body_accepts_vendor_keys_and_defaults_to_empty() {
    let raw = r#"{"id":"p","display_name":"p","kind":"open_ai_compatible","base_url":"https://api.example.com/v1","auth":{"source":"env","var":"K"},"capabilities":["embedding"],"extra_body":{"input_type":"passage"}}"#;
    let provider: ProviderConfig = serde_json::from_str(raw).expect("vendor key is accepted");
    assert_eq!(
        provider.extra_body["input_type"],
        serde_json::json!("passage")
    );
    assert!(provider.query_extra_body.is_empty());
}

#[test]
fn resolve_returns_provider_when_capability_present() {
    let registry = ProviderRegistry::new(ProvidersConfig {
        providers: vec![provider("openai", vec![Capability::Completion])],
    });
    let resolved = registry
        .resolve("openai", Capability::Completion)
        .expect("completion capability declared");
    assert_eq!(resolved.id, "openai");
}

#[test]
fn resolve_unknown_id_is_provider_not_found() {
    let registry = ProviderRegistry::new(ProvidersConfig::default());
    let err = registry
        .resolve("missing", Capability::Completion)
        .expect_err("unknown id must fail");
    assert!(matches!(err, RegistryError::ProviderNotFound { .. }));
}

#[test]
fn resolve_missing_capability_is_mismatch() {
    let registry = ProviderRegistry::new(ProvidersConfig {
        providers: vec![provider("local", vec![Capability::Embedding])],
    });
    let err = registry
        .resolve("local", Capability::Completion)
        .expect_err("completion not declared");
    assert!(matches!(err, RegistryError::CapabilityMismatch { .. }));
}

#[test]
fn default_completion_picks_first_completion_provider() {
    let registry = ProviderRegistry::new(ProvidersConfig {
        providers: vec![
            provider("local-embeddings", vec![Capability::Embedding]),
            provider("openai", vec![Capability::Completion]),
            provider("groq", vec![Capability::Completion]),
        ],
    });
    let resolved = registry
        .resolve_default_completion()
        .expect("one completion provider exists");
    assert_eq!(resolved.id, "openai");
}

#[test]
fn default_completion_without_any_completion_provider_errors() {
    let registry = ProviderRegistry::new(ProvidersConfig {
        providers: vec![provider("local", vec![Capability::Embedding])],
    });
    let err = registry
        .resolve_default_completion()
        .expect_err("no completion provider configured");
    assert!(matches!(err, RegistryError::ProviderNotFound { .. }));
}

#[test]
fn empty_config_reports_zero_len() {
    let config = ProvidersConfig::default();
    assert!(config.is_empty());
    assert_eq!(config.len(), 0);
}

/// Shared default-slot fixture (#1462): the first provider declaring
/// `Embedding` is `local-embeddings` and the first declaring
/// `Completion` is `openai` — one registry for both default-slot tests
/// instead of two mirrored setups. The trailing dual-capability
/// provider pins first-wins ordering on both slots at once.
fn default_slot_registry() -> ProviderRegistry {
    ProviderRegistry::new(ProvidersConfig {
        providers: vec![
            provider("local-embeddings", vec![Capability::Embedding]),
            provider("openai", vec![Capability::Completion]),
            provider(
                "ollama",
                vec![Capability::Embedding, Capability::Completion],
            ),
        ],
    })
}

#[test]
fn default_embedding_picks_first_embedding_provider() {
    let registry = default_slot_registry();
    let resolved = registry
        .resolve_default_embedding()
        .expect("one embedding provider exists");
    assert_eq!(resolved.id, "local-embeddings");
}

#[test]
fn default_embedding_without_any_embedding_provider_is_not_found() {
    let registry = ProviderRegistry::new(ProvidersConfig {
        providers: vec![provider("openai", vec![Capability::Completion])],
    });
    let err = registry
        .resolve_default_embedding()
        .expect_err("no embedding provider configured");
    assert!(
        matches!(
            err,
            RegistryError::ProviderNotFound { ref provider_id }
            if provider_id == "<default>"
        ),
        "default slot must report ProviderNotFound(<default>), got: {err}"
    );
}

const PROVIDER_JSON_TEMPLATE: &str = r#"{
    "id": "ollama",
    "display_name": "Ollama",
    "kind": "open_ai_compatible",
    "base_url": "https://api.example.com/v1",
    "auth": {"source": "env", "var": "WEBFANG_TEST_KEY"},
    "capabilities": ["embedding"],
    "model": "nomic-embed"__DIM____LOOPBACK____POLICY__
}"#;

fn provider_json(dim: &str, loopback: &str) -> String {
    provider_json_with_policy(dim, loopback, "")
}

fn provider_json_with_policy(dim: &str, loopback: &str, policy: &str) -> String {
    PROVIDER_JSON_TEMPLATE
        .replace("__DIM__", dim)
        .replace("__LOOPBACK__", loopback)
        .replace("__POLICY__", policy)
}

#[test]
fn network_policy_absent_defaults_to_restricted_empty() {
    let parsed: ProviderConfig =
        serde_json::from_str(&provider_json("", "")).expect("absent policy must parse");
    assert_eq!(parsed.network_policy, NetworkPolicy::default());
}

#[test]
fn network_policy_null_is_error() {
    // serde_json reports struct-from-null as "invalid type"; the
    // invariant is that null errors instead of silently defaulting.
    let err = serde_json::from_str::<ProviderConfig>(&provider_json_with_policy(
        "",
        "",
        ",\n        \"network_policy\": null",
    ))
    .expect_err("explicit null must never silently default");
    let text = err.to_string();
    assert!(
        text.contains("invalid type") || text.contains("expected value"),
        "unexpected null error text: {text}"
    );
}

#[test]
fn network_policy_allow_cidrs_invalid_is_error() {
    let err = serde_json::from_str::<ProviderConfig>(&provider_json_with_policy(
        "",
        "",
        ",\n        \"network_policy\": {\"mode\": \"allowlist\", \"allow_cidrs\": [\"garbage\"]}",
    ))
    .expect_err("invalid CIDR must fail loud");
    assert!(
        err.to_string().contains("no es un CIDR válido"),
        "got: {err}"
    );
}

#[test]
fn network_policy_allowlist_with_entry_parses() {
    let parsed: ProviderConfig = serde_json::from_str(&provider_json_with_policy(
        "",
        "",
        ",\n        \"network_policy\": {\"mode\": \"allowlist\", \"allow_cidrs\": [\"10.0.0.0/8\"], \"allow_hosts\": [\"MyServer.LAN\"]}",
    ))
    .expect("valid allowlist policy must parse");
    assert_eq!(
        parsed.network_policy.mode,
        crate::domain::network_policy::NetworkPolicyMode::Allowlist
    );
    assert_eq!(parsed.network_policy.allow_cidrs.len(), 1);
    assert_eq!(parsed.network_policy.allow_hosts, ["myserver.lan"]);
}

#[test]
fn allow_loopback_absent_deserializes_to_false() {
    let parsed: ProviderConfig =
        serde_json::from_str(&provider_json("", "")).expect("absent fields must parse");
    assert!(!parsed.allow_loopback);
    assert_eq!(parsed.embedding_dim, None);
}

#[test]
fn allow_loopback_explicit_true_parses() {
    let parsed: ProviderConfig =
        serde_json::from_str(&provider_json("", ",\n        \"allow_loopback\": true"))
            .expect("explicit true must parse");
    assert!(parsed.allow_loopback);
}

#[test]
fn allow_loopback_explicit_null_is_deserialization_error() {
    let err = serde_json::from_str::<ProviderConfig>(&provider_json(
        "",
        ",\n        \"allow_loopback\": null",
    ))
    .expect_err("explicit null must never silently default");
    assert!(
        err.to_string().contains("invalid type"),
        "null must fail as invalid type, got: {err}"
    );
}

#[test]
fn embedding_dim_absent_deserializes_to_none() {
    let parsed: ProviderConfig =
        serde_json::from_str(&provider_json("", "")).expect("absent dim must parse");
    assert_eq!(parsed.embedding_dim, None);
}

#[test]
fn embedding_dim_explicit_value_parses() {
    let parsed: ProviderConfig =
        serde_json::from_str(&provider_json(",\n        \"embedding_dim\": 1536", ""))
            .expect("explicit dim must parse");
    assert_eq!(parsed.embedding_dim, Some(1536));
}

#[test]
fn embedding_dim_explicit_null_is_deserialization_error() {
    let err = serde_json::from_str::<ProviderConfig>(&provider_json(
        ",\n        \"embedding_dim\": null",
        "",
    ))
    .expect_err("explicit null must never silently adopt");
    assert!(
        err.to_string().contains("null"),
        "null must fail loudly, got: {err}"
    );
}
