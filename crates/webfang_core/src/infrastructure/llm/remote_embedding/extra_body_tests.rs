//! `extra_body` wire contracts for embeddings (#3074).
//!
//! Sibling of `e2e_tests` so the 500-line ratchet of that file is preserved.

use super::fixtures::*;
use super::*;

/// Vendor fields are flattened at the body root, never nested, and WebFang's
/// own fields are still present (#3074). This is the exact shape NVIDIA NIM
/// needs to accept `nvidia/nemotron-3.5-embed-8b`.
#[tokio::test]
async fn extra_body_is_flattened_into_the_embeddings_body() {
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

    struct Capture(Arc<Mutex<Option<serde_json::Value>>>);
    impl Respond for Capture {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            *self.0.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(serde_json::from_slice(&request.body).expect("body is JSON"));
            ResponseTemplate::new(200).set_body_string(load_fixture("embeddings_tolerant.json"))
        }
    }

    let server = MockServer::start().await;
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(Capture(captured.clone()))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server, |cfg| {
        cfg.embedding_dim = Some(8);
        cfg.extra_body
            .insert("input_type".to_string(), serde_json::json!("passage"));
        cfg.extra_body
            .insert("truncate".to_string(), serde_json::json!("END"));
    });
    adapter
        .embed_batch(&["hello world".to_string()])
        .await
        .expect("200 + valid JSON succeeds");

    let body = captured
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .expect("the mock must have captured the body");
    assert_eq!(body["input_type"], serde_json::json!("passage"));
    assert_eq!(body["truncate"], serde_json::json!("END"));
    assert_eq!(body["encoding_format"], serde_json::json!("float"));
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
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

    struct Capture(Arc<Mutex<Vec<serde_json::Value>>>);
    impl Respond for Capture {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            self.0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(serde_json::from_slice(&request.body).expect("body is JSON"));
            ResponseTemplate::new(200).set_body_string(load_fixture("embeddings_tolerant.json"))
        }
    }

    let server = MockServer::start().await;
    let captured: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(Capture(captured.clone()))
        .mount(&server)
        .await;

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

    let bodies = captured.lock().unwrap_or_else(|p| p.into_inner());
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
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

    struct Capture(Arc<Mutex<Option<serde_json::Value>>>);
    impl Respond for Capture {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            *self.0.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(serde_json::from_slice(&request.body).expect("body is JSON"));
            ResponseTemplate::new(200).set_body_string(load_fixture("embeddings_tolerant.json"))
        }
    }

    let server = MockServer::start().await;
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(Capture(captured.clone()))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server, |cfg| {
        cfg.embedding_dim = Some(8);
        cfg.extra_body
            .insert("model".to_string(), serde_json::json!("attacker-model"));
    });
    adapter
        .embed_batch(&["hello world".to_string()])
        .await
        .expect("200 + valid JSON succeeds");

    let body = captured
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .expect("the mock must have captured the body");
    assert_eq!(body["model"], serde_json::json!("nomic-embed-text"));
}
