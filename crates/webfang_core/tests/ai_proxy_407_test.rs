#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! ADR-0021: a 407 from the proxy hop is terminal (zero retries) and reports
//! whether a proxy credential was configured, without ever printing it.
//!
//! These live in an integration test so the grandfathered
//! `remote_embedding.rs` keeps its net-zero line budget (#file-size ratchet).

use std::sync::Arc;

use webfang_core::domain::auth_source::AuthSource;
use webfang_core::domain::embedding_port::EmbeddingPort;
use webfang_core::domain::network_policy::NetworkPolicy;
use webfang_core::domain::providers::{Capability, ProviderConfig, ProviderKind};
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
        embedding_dim: None,
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
        message.contains("credencial configurada"),
        "a configured proxy credential must be named without printing it: {message}"
    );
    assert_eq!(
        server.received_requests().await.expect("request log").len(),
        1,
        "407 is terminal — zero retries"
    );
}
