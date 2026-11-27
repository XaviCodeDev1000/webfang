#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! B1 — `AuthSource::None`: the endpoint without a credential.
//!
//! ## Why this is an integration test and not a unit test
//!
//! Half of these rows are about *configuration*: that `{"source": "none"}`
//! can be expressed in the providers file is precisely the assertion that
//! `domain/providers.rs` needs no split variants. That is checked from outside
//! the crate, over the public type, in the test crate — a unit test inside
//! `webfang_core` could "prove it" against the very code that defines it and
//! would prove nothing about the user-facing contract.
//!
//! ## What these rows pin (ADR-0004 §Review condition)
//!
//! 1. `None` builds the remote adapter without a credential error.
//! 2. The request carries NO `Authorization` header.
//! 3. Security invariant: with `None` **and** a valid key environment variable
//!    present, the request still carries **no** `Authorization` — the
//!    "falling back to another source" case nobody covered before this unit.
//! 4. A 401 is distinguished by whether a credential was sent, so a 401 from
//!    an anonymous provider is neither read nor reported as an "invalid
//!    credential".
//! 5. An unset `Env { var }` still fails, with the same message.
//! 6. Loopback is still allowed with the config opt-in.

#![allow(clippy::disallowed_methods)] // via EnvGuard, the sanctioned owner of env mutation

use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::llm_wire::build_llm_provider;
use webfang_core::domain::auth_source::{AuthError, AuthSource};
use webfang_core::domain::providers::{Capability, ProviderConfig, ProvidersConfig};
use webfang_core::domain::EmbeddingPort;
use webfang_core::error::SemanticError;
use webfang_core::infrastructure::llm::provider::{OpenAiCompatibleProvider, ProviderInitError};
use webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter;
use webfang_core::CliExit;
use webfang_test_utils::EnvGuard;

/// An 8d vector: the repo fixtures use this same shape.
const ONE_VECTOR_8D: &str = r#"{"data": [{"index": 0,
    "embedding": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]}]}"#;

/// Path of the embedding endpoint. Named so the mock and the assertions cannot
/// drift out of sync through a string repeated in two places.
const EMBEDDINGS_PATH: &str = "/embeddings";

/// An embedding `ProviderConfig` pointing at the wiremock, with the auth
/// source each row needs.
fn config_for(server: &wiremock::MockServer, auth: AuthSource) -> ProviderConfig {
    provider_with(
        url::Url::parse(&server.uri()).expect("mock uri parses"),
        true,
        auth,
    )
}

async fn mount_embeddings(
    server: &wiremock::MockServer,
    status: u16,
    body: &str,
) -> wiremock::MockGuard {
    let response = wiremock::ResponseTemplate::new(status).set_body_string(body);
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(EMBEDDINGS_PATH))
        .respond_with(response)
        .mount_as_scoped(server)
        .await
}

/// An embedding `ProviderConfig` for the rows that only observe construction
/// (they make no request): they need no real server, and a public base URL
/// suffices because the row never opens a socket.
fn config_without_server(auth: AuthSource) -> ProviderConfig {
    provider_with(
        url::Url::parse("https://api.example.com/v1").expect("url"),
        false,
        auth,
    )
}

/// The embedding `ProviderConfig` shared by both Arranges: the twins differed
/// only in base URL and in the wiremock's loopback opt-in.
fn provider_with(base_url: url::Url, allow_loopback: bool, auth: AuthSource) -> ProviderConfig {
    ProviderConfig {
        id: "anon-endpoint".to_string(),
        display_name: "Anonymous Endpoint".to_string(),
        base_url,
        auth,
        // T7: declare the contract width so the dimension guard does not cut
        // these auth/header cases before the request reaches the mock.
        embedding_dim: Some(8),
        allow_loopback,
        ..webfang_test_utils::test_provider_config()
    }
}

/// The `Authorization` header as the server saw it, if there was one.
async fn recorded_authorization(server: &wiremock::MockServer) -> Vec<Option<String>> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|req| {
            req.headers
                .get("authorization")
                .map(|v| v.to_str().unwrap_or_default().to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Expression from the configuration file
// ---------------------------------------------------------------------------

/// `AuthSource` is `#[serde(tag = "source")]`, so `{"source": "none"}` must be
/// enough. This row is the proof that `domain/providers.rs` (outside this
/// unit's surfaces, and deliberately untouched) declares
/// `pub auth: AuthSource` and does not split variants: if someone changed it
/// to "support" `None`, this row would document the exact point.
#[test]
fn providers_config_parses_source_none_from_the_config_file() {
    let raw = r#"{
      "providers": [
        {
          "id": "local-embed",
          "display_name": "Local embeddings",
          "kind": "open_ai_compatible",
          "base_url": "http://127.0.0.1:11434/v1",
          "auth": { "source": "none" },
          "capabilities": ["embedding"],
          "model": "nomic-embed-text",
          "allow_loopback": true
        }
      ]
    }"#;
    let parsed: ProvidersConfig =
        serde_json::from_str(raw).expect("config with auth none must parse");
    assert_eq!(parsed.len(), 1);
    let provider = &parsed.providers[0];
    assert!(
        matches!(provider.auth, AuthSource::None),
        "auth must deserialize to the None variant, not another: {:?}",
        provider.auth
    );
}

/// Round-trip: the wizard writes the config and reads it back without losing
/// its meaning.
#[test]
fn source_none_round_trips_through_serialization() {
    let source = AuthSource::None;
    let json = serde_json::to_string(&source).expect("serializes");
    assert_eq!(json, r#"{"source":"none"}"#);
    let back: AuthSource = serde_json::from_str(&json).expect("reparses");
    assert!(matches!(back, AuthSource::None));
}

/// `{"source": "none"}` is not an alias for "no `auth`": the field remains
/// mandatory and explicit. A provider without `auth` does not parse, which is
/// the difference between "declared anonymous endpoint" and "incomplete
/// configuration".
#[test]
fn omitting_auth_still_fails_rather_than_implying_none() {
    let raw = r#"{
      "providers": [
        {
          "id": "no-auth",
          "display_name": "No auth",
          "kind": "open_ai_compatible",
          "base_url": "http://127.0.0.1:11434/v1",
          "capabilities": ["embedding"],
          "model": "m"
        }
      ]
    }"#;
    let parsed: Result<ProvidersConfig, _> = serde_json::from_str(raw);
    assert!(
        parsed.is_err(),
        "omitting `auth` must fail: `None` is explicit, not an implicit default"
    );
}

// ---------------------------------------------------------------------------
// 2-3. Construction without a credential + the request carries NO Authorization
// ---------------------------------------------------------------------------

/// AC1: `auth: None` builds the adapter without a credential error.
#[tokio::test]
async fn none_auth_builds_the_adapter_without_a_credential_error() {
    let server = wiremock::MockServer::start().await;
    let cfg = config_for(&server, AuthSource::None);
    let adapter = match RemoteEmbeddingAdapter::new(cfg) {
        Ok(a) => a,
        Err(e) => panic!("auth none must build the adapter, failed: {e}"),
    };
    assert_eq!(adapter.id(), "anon-endpoint");
}

/// AC2 + AC3: what this row proves is that the request carries **no
/// `Authorization` header** — it reads what arrived at the server, not an
/// inspection of the code.
///
/// The `WEBFANG_TEST_NONE_TRAP_KEY` variable is **decorative** on purpose:
/// `AuthSource::None` names no environment variable, so setting it does not by
/// itself prove there is no fallback. It stays as a safety net for the
/// concrete regression of falling back to *that* variable; it is not —nor does
/// it claim to be— proof of the general "no fallback" guarantee. That
/// guarantee rests on the observable assertion below, together with the
/// positive control `env_auth_still_sends_the_bearer_header` (which shows the
/// helper does detect a bearer when one exists).
#[tokio::test]
async fn none_auth_request_carries_no_authorization_header() {
    let server = wiremock::MockServer::start().await;
    let _guard = EnvGuard::with(&[("WEBFANG_TEST_NONE_TRAP_KEY", "sk-must-not-be-used")]);
    let _mock = mount_embeddings(&server, 200, ONE_VECTOR_8D).await;
    let cfg = config_for(&server, AuthSource::None);
    let adapter = RemoteEmbeddingAdapter::new(cfg).expect("adapter with auth none");

    let vectors = adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect("the anonymous endpoint responds 200");
    assert_eq!(vectors.len(), 1);

    let seen = recorded_authorization(&server).await;
    assert_eq!(seen.len(), 1, "exactly one request must have been recorded");
    assert!(
        seen[0].is_none(),
        "auth None must NEVER synthesize an Authorization header; saw: {:?}",
        seen[0]
    );
}

/// Positive control for the previous row: with a resolved `Env { var }`, the
/// header DOES travel. Without this row, "no Authorization" could pass with a
/// client that never sends auth at all.
#[tokio::test]
async fn env_auth_still_sends_the_bearer_header() {
    let server = wiremock::MockServer::start().await;
    let _guard = EnvGuard::with(&[("WEBFANG_TEST_ENV_BEARER_KEY", "sk-expected")]);
    let _mock = mount_embeddings(&server, 200, ONE_VECTOR_8D).await;
    let cfg = config_for(
        &server,
        AuthSource::Env {
            var: "WEBFANG_TEST_ENV_BEARER_KEY".to_string(),
        },
    );
    let adapter = RemoteEmbeddingAdapter::new(cfg).expect("adapter with auth env");

    adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect("the endpoint responds 200");

    let seen = recorded_authorization(&server).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].as_deref(),
        Some("Bearer sk-expected"),
        "auth env must still sign the request"
    );
}

// ---------------------------------------------------------------------------
// 4. A 401 is distinguished by whether a credential was sent
// ---------------------------------------------------------------------------

/// AC4 (row A): 401 from a provider **without** a credential. The message must
/// state that the request was anonymous — if it said "invalid credential" it
/// would send the operator off to rotate a key that never existed.
#[tokio::test]
async fn unauthorized_without_credential_is_not_reported_as_an_invalid_credential() {
    let server = wiremock::MockServer::start().await;
    let _mock = mount_embeddings(&server, 401, r#"{"error":"unauthorized"}"#).await;
    let cfg = config_for(&server, AuthSource::None);
    let adapter = RemoteEmbeddingAdapter::new(cfg).expect("adapter with auth none");

    let err = adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect_err("401 must fail");
    let msg = err.to_string();

    assert!(
        matches!(err, SemanticError::Inference(_)),
        "still Inference (same type as before), only the message changed: {msg}"
    );
    assert!(
        msg.contains("401"),
        "the message must name the observed status: {msg}"
    );
    assert!(
        !msg.contains("invalid"),
        "an anonymous 401 is NOT an invalid credential, the message must not say so: {msg}"
    );
    assert!(
        msg.contains("none") || msg.contains("anonymous"),
        "the message must state that the request was sent without a credential: {msg}"
    );
}

/// AC4 (row B): 401 from a provider **with** a credential. Different from the
/// previous row, and explicitly a rejection of the credential that was sent.
#[tokio::test]
async fn unauthorized_with_credential_is_reported_as_a_rejected_credential() {
    let server = wiremock::MockServer::start().await;
    let _guard = EnvGuard::with(&[("WEBFANG_TEST_401_KEY", "sk-rejected")]);
    let _mock = mount_embeddings(&server, 401, r#"{"error":"unauthorized"}"#).await;
    let cfg = config_for(
        &server,
        AuthSource::Env {
            var: "WEBFANG_TEST_401_KEY".to_string(),
        },
    );
    let adapter = RemoteEmbeddingAdapter::new(cfg).expect("adapter with auth env");

    let err = adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect_err("401 must fail");
    let msg = err.to_string();

    assert!(msg.contains("401"), "{msg}");
    assert!(
        msg.contains("credential") || msg.contains("env"),
        "the message must point to the credential and its source: {msg}"
    );

    // The two AC4 rows must be distinguishable from each other: that is the
    // point of the criterion, not two messages that merely look alike.
    let anonymous_server = wiremock::MockServer::start().await;
    let anon_mock = mount_embeddings(&anonymous_server, 401, r#"{"error":"unauthorized"}"#).await;
    let anon_adapter = RemoteEmbeddingAdapter::new(config_for(&anonymous_server, AuthSource::None))
        .expect("adapter with auth none");
    let anon_err = anon_adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect_err("anonymous 401 must fail");
    assert_ne!(
        anon_err.to_string(),
        msg,
        "the anonymous 401 and the credentialed 401 must not render the same text"
    );
    drop(anon_mock);
}

// ---------------------------------------------------------------------------
// 5. A configured but unresolvable credential is still a hard error
// ---------------------------------------------------------------------------

/// Invariant 2: no fallback. `Env { var }` without the variable set does NOT
/// degrade to "no auth" — it fails hard, with the same message as always.
#[test]
fn unresolvable_env_credential_still_fails_hard_without_degrading_to_none() {
    let _guard = EnvGuard::clean(&["WEBFANG_TEST_UNSET_CREDENTIAL"]);
    let cfg = config_without_server(AuthSource::Env {
        var: "WEBFANG_TEST_UNSET_CREDENTIAL".to_string(),
    });

    let err = match RemoteEmbeddingAdapter::new(cfg) {
        Err(e) => e,
        Ok(_) => panic!("an unresolvable credential must still fail"),
    };
    assert!(
        matches!(err, ProviderInitError::AuthFailed { .. }),
        "must still be AuthFailed, not a silent degradation: {err}"
    );
    match &err {
        ProviderInitError::AuthFailed { source, .. } => assert!(
            matches!(source, AuthError::EnvNotSet(_)),
            "and with the usual error variant: {source}"
        ),
        other => panic!("unexpected variant: {other}"),
    }
    assert!(
        err.to_string().contains("WEBFANG_TEST_UNSET_CREDENTIAL"),
        "the message names the variable: {err}"
    );
}

// ---------------------------------------------------------------------------
// 7. GAP 1 — the completion path, reached by real user configuration
// ---------------------------------------------------------------------------

/// A real providers file (JSON, exactly as the user would write it) with
/// `completion` capability and `auth: {source: none}`. None of this is built
/// by hand in the test: it all goes through the deserializer.
const COMPLETION_ANON_CONFIG: &str = r#"{
  "providers": [
    {
      "id": "anon-chat",
      "display_name": "Anonymous chat",
      "kind": "open_ai_compatible",
      "base_url": "https://api.example.com/v1",
      "auth": { "source": "none" },
      "capabilities": ["completion"],
      "model": "gpt-test"
    }
  ]
}"#;

fn llm_opts(provider_id: &str) -> CrawlOptions {
    CrawlOptions {
        extract_with_llm: true,
        llm_provider: Some(provider_id.to_string()),
        ..Default::default()
    }
}

/// Route 1 — the path a user actually takes: `--extract-with-llm` against an
/// anonymous provider declared in the configuration file. It fails at startup
/// (`ConfigError`, exit 78), and the message declares the anonymous endpoint
/// instead of hinting at a broken credential.
#[test]
fn completion_provider_with_auth_none_fails_at_startup_with_the_anonymous_message() {
    let providers: ProvidersConfig =
        serde_json::from_str(COMPLETION_ANON_CONFIG).expect("anonymous completion config parses");

    let err = match build_llm_provider(&llm_opts("anon-chat"), &providers) {
        Err(e) => e,
        Ok(_) => panic!("an anonymous completion provider must fail at startup"),
    };
    let msg = match &err {
        CliExit::ConfigError(msg) => msg,
        other => panic!("must be ConfigError (exit 78), not: {other:?}"),
    };
    assert!(
        msg.contains("anon-chat"),
        "the message names the provider that failed: {msg}"
    );
    assert!(
        msg.contains("anonymous"),
        "the message states that the endpoint is anonymous: {msg}"
    );
    assert!(
        !msg.contains("could not resolve credential"),
        "that text belongs to AuthFailed, the OTHER variant: {msg}"
    );
}

/// The typed variant, over the `ProviderConfig` **that comes out of the
/// configuration file** (not an artificial enum construction): it pins the
/// contract of `provider.rs`, not only the text the CLI projects.
#[test]
fn completion_provider_with_auth_none_yields_the_anonymous_variant_not_auth_failed() {
    let providers: ProvidersConfig =
        serde_json::from_str(COMPLETION_ANON_CONFIG).expect("anonymous completion config parses");
    let config = providers.providers[0].clone();
    assert!(
        matches!(config.auth, AuthSource::None),
        "the config from the file must carry auth none"
    );
    assert!(config.has_capability(Capability::Completion));

    let err = match OpenAiCompatibleProvider::new(config) {
        Err(e) => e,
        Ok(_) => panic!("auth none in completion must fail"),
    };
    match &err {
        ProviderInitError::AnonymousCompletionUnsupported { provider_id } => {
            assert_eq!(provider_id, "anon-chat");
        },
        other => panic!("expected AnonymousCompletionUnsupported, got: {other}"),
    }
    // Invariant 4: `None` is NOT a credential failure, not here nor anywhere.
    assert!(
        !matches!(err, ProviderInitError::AuthFailed { .. }),
        "None must never map to AuthFailed: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("auth: none"),
        "the message names the cause: {msg}"
    );
    // The full text is user-facing, and AGENTS.md requires English.
    // Asserting only `auth: none` would let through a message that lost the
    // half explaining WHY the anonymous path is unsupported — the actionable
    // part for whoever configures the provider.
    assert!(
        msg.contains("not yet supported by the chat/completions path"),
        "the message must explain that anonymous chat/completions is not yet supported: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 6. Loopback and offline: not broken
// ---------------------------------------------------------------------------

/// Loopback (`127.0.0.1`) is still allowed with the config opt-in — the SSRF
/// row is untouched by adding an auth source that carries no credential.
#[test]
fn loopback_with_opt_in_still_builds_under_auth_none() {
    let mut cfg = config_without_server(AuthSource::None);
    cfg.base_url = url::Url::parse("http://127.0.0.1:9/v1").expect("url");
    cfg.allow_loopback = true;

    let adapter = match RemoteEmbeddingAdapter::new(cfg) {
        Ok(a) => a,
        Err(e) => panic!("loopback with opt-in must still build: {e}"),
    };
    assert_eq!(adapter.id(), "anon-endpoint");
}

/// And without the opt-in it is still blocked: `auth: None` is not a way to
/// relax the SSRF guard.
#[test]
fn loopback_without_opt_in_is_still_blocked_under_auth_none() {
    let mut cfg = config_without_server(AuthSource::None);
    cfg.base_url = url::Url::parse("http://127.0.0.1:9/v1").expect("url");
    cfg.allow_loopback = false;

    let err = match RemoteEmbeddingAdapter::new(cfg) {
        Err(e) => e,
        Ok(_) => panic!("loopback without opt-in must still be blocked"),
    };
    assert!(
        matches!(err, ProviderInitError::InvalidBaseUrl { .. }),
        "auth none must not change the SSRF guard's decision: {err}"
    );
}
