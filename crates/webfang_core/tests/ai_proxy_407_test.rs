#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! ADR-0021: a 407 from the proxy hop is terminal (zero retries) and reports
//! whether a proxy credential was configured, without ever printing it.
//!
//! These live in an integration test so the grandfathered
//! `remote_embedding.rs` keeps its net-zero line budget (#file-size ratchet).

use std::sync::Arc;

use webfang_core::domain::auth_source::AuthSource;
use webfang_core::domain::embedding_port::EmbeddingPort;
use webfang_core::domain::llm_port::{ChatMessage, LlmPort, LlmRequest};
use webfang_core::domain::network_policy::NetworkPolicy;
use webfang_core::domain::providers::{Capability, ProviderConfig, ProviderKind};
use webfang_core::infrastructure::llm::provider::OpenAiCompatibleProvider;
use webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(base_url: &str) -> ProviderConfig {
    ProviderConfig {
        id: "proxy-407".to_string(),
        display_name: "Proxy 407".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: url::Url::parse(base_url).expect("valid base url"),
        auth: AuthSource::None,
        capabilities: vec![Capability::Embedding],
        model: Some("nomic-embed-text".to_string()),
        // T7: 407 cases must reach the mock; a cold adapter would cut first.
        embedding_dim: Some(8),
        allow_loopback: true,
        network_policy: NetworkPolicy::default(),
        proxy: None,
    }
}

async fn server_returning_407() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(407).set_body_string("proxy authentication required"))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn status_407_is_terminal_and_names_the_missing_proxy_auth() {
    let server = server_returning_407().await;
    let adapter = RemoteEmbeddingAdapter::with_http_and_proxy_auth(
        config(&server.uri()),
        wreq::Client::builder().build().expect("test client builds"),
        false,
    )
    .expect("adapter builds");
    let port: Arc<dyn EmbeddingPort> = Arc::new(adapter);

    let err = port
        .embed_batch(&["a".to_string()])
        .await
        .expect_err("407 must fail");
    let message = err.to_string();
    assert!(
        message.contains("407") && message.contains("proxy.auth"),
        "407 must name the missing proxy.auth: {message}"
    );
    assert_eq!(
        server.received_requests().await.expect("request log").len(),
        1,
        "407 is terminal — zero retries"
    );
}

fn completion_config(base_url: &str) -> ProviderConfig {
    ProviderConfig {
        auth: AuthSource::Env {
            var: "WEBFANG_TEST_PROXY_407_KEY".to_string(),
        },
        capabilities: vec![Capability::Completion],
        ..config(base_url)
    }
}

fn completion_request() -> LlmRequest {
    LlmRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "ping".to_string(),
        }],
        model: "gpt-test".to_string(),
        max_tokens: 16,
    }
}

async fn server_returning_407_completion() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(407).set_body_string("proxy authentication required"))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn completion_407_is_terminal_and_names_the_missing_proxy_auth() {
    let _env = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_PROXY_407_KEY", "sk-test")]);
    let server = server_returning_407_completion().await;
    let provider = OpenAiCompatibleProvider::with_http_and_proxy_auth(
        completion_config(&server.uri()),
        wreq::Client::builder().build().expect("test client builds"),
        false,
    )
    .expect("provider builds");

    let err = provider
        .send_completion(completion_request())
        .await
        .expect_err("407 must fail");
    let message = err.to_string();
    assert!(
        message.contains("407") && message.contains("proxy.auth"),
        "completion 407 must name the missing proxy.auth: {message}"
    );
    assert_eq!(
        server.received_requests().await.expect("request log").len(),
        1,
        "407 is terminal — zero retries"
    );
}

#[tokio::test]
async fn completion_407_distinguishes_a_configured_proxy_credential() {
    let _env = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_PROXY_407_KEY", "sk-test")]);
    let server = server_returning_407_completion().await;
    let provider = OpenAiCompatibleProvider::with_http_and_proxy_auth(
        completion_config(&server.uri()),
        wreq::Client::builder().build().expect("test client builds"),
        true,
    )
    .expect("provider builds");

    let err = provider
        .send_completion(completion_request())
        .await
        .expect_err("407 must fail");
    assert!(
        err.to_string().contains("configured credential"),
        "a configured proxy credential must be named without printing it: {err}"
    );
    assert_eq!(
        server.received_requests().await.expect("request log").len(),
        1,
        "407 is terminal — zero retries"
    );
}

#[tokio::test]
async fn status_407_distinguishes_a_configured_proxy_credential() {
    let server = server_returning_407().await;
    let adapter = RemoteEmbeddingAdapter::with_http_and_proxy_auth(
        config(&server.uri()),
        wreq::Client::builder().build().expect("test client builds"),
        true,
    )
    .expect("adapter builds");

    let err = adapter
        .embed_batch(&["a".to_string()])
        .await
        .expect_err("407 must fail");
    let message = err.to_string();
    assert!(
        message.contains("configured credential"),
        "a configured proxy credential must be named without printing it: {message}"
    );
    assert_eq!(
        server.received_requests().await.expect("request log").len(),
        1,
        "407 is terminal — zero retries"
    );
}
