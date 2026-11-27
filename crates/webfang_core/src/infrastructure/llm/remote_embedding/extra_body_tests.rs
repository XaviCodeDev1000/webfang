//! `extra_body` wire contracts for embeddings (#3074).
//!
//! Sibling of `e2e_tests` so the 500-line ratchet of that file is preserved.

use std::sync::{Arc, Mutex, MutexGuard};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

use super::fixtures::*;
use super::*;

/// Captures every request body sent to `/embeddings` (#3074).
#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<serde_json::Value>>>);

impl Capture {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn bodies(&self) -> MutexGuard<'_, Vec<serde_json::Value>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn last(&self) -> serde_json::Value {
        self.bodies()
            .last()
            .cloned()
            .expect("the mock must have captured the body")
    }
}

impl Respond for Capture {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        self.bodies()
            .push(serde_json::from_slice(&request.body).expect("body is JSON"));
        ResponseTemplate::new(200).set_body_string(load_fixture("embeddings_tolerant.json"))
    }
}

async fn mount_capture(server: &MockServer) -> Capture {
    let capture = Capture::new();
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(capture.clone())
        .mount(server)
        .await;
    capture
}

/// One indexing request with the shared contract width applied (#3074).
async fn embed_once(
    server: &MockServer,
    modifier: impl FnOnce(&mut crate::domain::providers::ProviderConfig),
) {
    let adapter = adapter_for(server, |cfg| {
        cfg.embedding_dim = Some(8);
        modifier(cfg);
    });
    adapter
        .embed_batch(&["hello world".to_string()])
        .await
        .expect("200 + valid JSON succeeds");
}

/// Vendor fields are flattened at the body root, never nested, and WebFang's
/// own fields are still present (#3074). This is the exact shape NVIDIA NIM
/// needs to accept `nvidia/nemotron-3.5-embed-8b`.
#[tokio::test]
async fn extra_body_is_flattened_into_the_embeddings_body() {
    let server = MockServer::start().await;
    let captured = mount_capture(&server).await;

    embed_once(&server, |cfg| {
        cfg.extra_body
            .insert("input_type".to_string(), serde_json::json!("passage"));
        cfg.extra_body
            .insert("truncate".to_string(), serde_json::json!("END"));
    })
    .await;

    let body = captured.last();
    assert_eq!(body["input_type"], serde_json::json!("passage"));
    assert_eq!(body["truncate"], serde_json::json!("END"));
    assert!(
        body.get("encoding_format").is_none(),
        "the field is opt-in, not sent by default"
    );
    assert_eq!(body["model"], serde_json::json!("nomic-embed-text"));
    assert_eq!(body["input"], serde_json::json!(["hello world"]));
    assert!(
        body.get("extra_body").is_none(),
        "a nested extra_body object is rejected by strict servers"
    );
}

/// Indexing and vault lookup use different overlays for asymmetric models:
/// the base map stays, and `query_extra_body` wins on the query request.
#[tokio::test]
async fn query_embedding_uses_the_query_extra_body_overlay() {
    let server = MockServer::start().await;
    let captured = mount_capture(&server).await;

    let adapter = adapter_for(&server, |cfg| {
        cfg.embedding_dim = Some(8);
        cfg.extra_body
            .insert("input_type".to_string(), serde_json::json!("passage"));
        cfg.extra_body
            .insert("truncate".to_string(), serde_json::json!("END"));
        cfg.query_extra_body
            .insert("input_type".to_string(), serde_json::json!("query"));
    });
    adapter
        .embed_batch(&["a document".to_string()])
        .await
        .expect("indexing succeeds");
    adapter
        .embed_query("a question")
        .await
        .expect("query succeeds");

    let bodies = captured.bodies();
    assert_eq!(bodies.len(), 2, "one request per embedding call");
    assert_eq!(bodies[0]["input_type"], serde_json::json!("passage"));
    assert_eq!(bodies[1]["input_type"], serde_json::json!("query"));
    assert_eq!(
        bodies[1]["truncate"],
        serde_json::json!("END"),
        "the overlay keeps base keys it does not override"
    );
}

/// WebFang's own body fields are serialized last: even a value injected
/// programmatically into the map cannot replace `model` or `input`.
#[tokio::test]
async fn webfang_body_fields_win_over_extra_body_values() {
    let server = MockServer::start().await;
    let captured = mount_capture(&server).await;

    embed_once(&server, |cfg| {
        cfg.extra_body
            .insert("model".to_string(), serde_json::json!("attacker-model"));
    })
    .await;

    let body = captured.last();
    assert_eq!(body["model"], serde_json::json!("nomic-embed-text"));
}

/// Opting in sends `encoding_format=float` for gateways that default to
/// base64. This is the only way the field reaches the wire (#3074).
#[tokio::test]
async fn opting_in_sends_encoding_format_float() {
    let server = MockServer::start().await;
    let captured = mount_capture(&server).await;

    embed_once(&server, |cfg| {
        cfg.send_encoding_format = true;
    })
    .await;

    let body = captured.last();
    assert_eq!(body["encoding_format"], serde_json::json!("float"));
}
