#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! S10 property: a credential embedded in a URL must never reach any output
//! sink — `Debug`, `Serialize`, proxy errors, `CliExit` output or the
//! `--trace-file` JSONL. The endpoint itself (scheme, host, port, path, query)
//! stays identifiable so the diagnostic is still useful.

#[path = "common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;
use tracing_subscriber::layer::SubscriberExt;
use url::Url;

use common::cli_harness::cmd;
use webfang_core::application::rate_limiter::{record_pacing_wait, PacingContext};
use webfang_core::domain::auth_source::AuthSource;
use webfang_core::domain::network_policy::NetworkPolicy;
use webfang_core::domain::providers::{Capability, ProviderConfig, ProviderKind};
use webfang_core::domain::proxy_config::{ProxyConfigError, ProxyDefinition, ValidatedProxy};
use webfang_core::infrastructure::observability::FileTraceLayer;

const SECRET: &str = "s3cret";
const CREDENTIAL_URL: &str = "https://user:s3cret@host/";
const PROVIDER_TOML: &str = "[[providers]]\nid = \"userinfo\"\ndisplay_name = \"Userinfo\"\n\
     kind = \"open_ai_compatible\"\nbase_url = \"https://user:s3cret@host/\"\n\
     auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
     model = \"nomic-embed-text\"\n";

fn provider_config() -> ProviderConfig {
    ProviderConfig {
        extra_body: serde_json::Map::new(),
        query_extra_body: serde_json::Map::new(),
        id: "userinfo".to_owned(),
        display_name: "Userinfo".to_owned(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: Url::parse(CREDENTIAL_URL).expect("fixture url"),
        auth: AuthSource::None,
        capabilities: vec![Capability::Embedding],
        model: Some("nomic-embed-text".to_owned()),
        embedding_dim: None,
        allow_loopback: false,
        network_policy: NetworkPolicy::default(),
        proxy: None,
    }
}

fn assert_clean(surface: &str, sink: &str) {
    assert!(
        !surface.contains(SECRET),
        "{sink} leaked the credential: {surface}"
    );
}

fn seeded_config() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, PROVIDER_TOML).expect("write config");
    (dir, config)
}

/// Both CLI streams must stay free of the credential, including the sink that
/// carries `CliExit` output.
fn assert_streams_clean(output: &Output, sink: &str) {
    assert_clean(
        &String::from_utf8_lossy(&output.stdout),
        &format!("{sink} stdout"),
    );
    assert_clean(
        &String::from_utf8_lossy(&output.stderr),
        &format!("{sink} stderr (CliExit output)"),
    );
}

/// Run `f` under a scoped subscriber writing to `path`. The layer is dropped
/// before the caller reads the JSONL, so the file is fully flushed.
fn with_file_trace<R>(path: &std::path::Path, f: impl FnOnce() -> R) -> R {
    let layer = FileTraceLayer::new(path.to_path_buf()).expect("file trace layer");
    let subscriber = tracing_subscriber::Registry::default().with(layer);
    tracing::subscriber::with_default(subscriber, f)
}

#[test]
fn debug_and_serialize_strip_base_url_userinfo() {
    let config = provider_config();
    assert_clean(&format!("{config:?}"), "Debug(ProviderConfig)");
    assert_clean(
        &serde_json::to_string(&config).expect("serialize config"),
        "Serialize(ProviderConfig)",
    );
}

#[test]
fn proxy_errors_strip_origin_userinfo() {
    let definition = |url| ProxyDefinition {
        url,
        allow_loopback: false,
        network_policy: NetworkPolicy::default(),
        auth: None,
    };
    let unsupported = ValidatedProxy::try_inline(&definition(
        Url::parse("socks5://user:s3cret@host/").expect("fixture url"),
    ))
    .expect_err("socks5 must be rejected");
    let user_info = ValidatedProxy::try_inline(&definition(
        Url::parse(CREDENTIAL_URL).expect("fixture url"),
    ))
    .expect_err("userinfo must be rejected");
    let origin_only = ProxyConfigError::OriginOnly {
        origin: CREDENTIAL_URL.to_owned(),
    };
    for err in [unsupported, user_info, origin_only] {
        assert_clean(&err.to_string(), "ProxyConfigError Display");
    }
}

#[test]
fn url_valued_trace_fields_strip_userinfo_in_jsonl() {
    let dir = TempDir::new().expect("temp dir");
    let trace_path = dir.path().join("trace.jsonl");
    with_file_trace(&trace_path, || {
        record_pacing_wait(
            &PacingContext::bare("test").with_url(CREDENTIAL_URL),
            Duration::ZERO,
            "granted",
        );
        tracing::warn!(
            target_uri = %Url::parse(CREDENTIAL_URL).expect("fixture url"),
            "redirect blocked"
        );
    });

    let raw = std::fs::read_to_string(&trace_path).expect("trace file readable");
    assert_clean(&raw, "FileTraceLayer JSONL");

    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).expect("trace line is JSON"))
        .collect();
    let pacing = events
        .iter()
        .find(|event| event["fields"]["operation"].as_str() == Some("http.rate_limit.wait"))
        .expect("pacing event must be recorded");
    assert_eq!(
        pacing["fields"]["url"].as_str(),
        Some("https://host/"),
        "the redacted field keeps the endpoint but drops userinfo"
    );
    let redirect = events
        .iter()
        .find(|event| event["fields"]["target_uri"].is_string())
        .expect("redirect event must be recorded");
    assert_eq!(
        redirect["fields"]["target_uri"].as_str(),
        Some("https://host/"),
        "target_uri must be redacted too"
    );
}

#[test]
fn doctor_and_show_never_print_base_url_credentials() {
    let (_dir, config) = seeded_config();
    for subcommand in ["show", "doctor"] {
        let output = cmd()
            .args([subcommand])
            .env("WEBFANG_CONFIG", &config)
            .output()
            .expect("run binary");
        assert_streams_clean(&output, subcommand);
    }
}
