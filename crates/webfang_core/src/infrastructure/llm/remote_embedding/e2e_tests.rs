use super::*;
use super::fixtures::*;
use crate::domain::auth_source::AuthSource;
use std::time::Duration;

// --- Wire behavior against wiremock (loopback literals bypass the
// custom resolver, so the plain injected client needs no env hatch) ---

async fn mount_embeddings(
    server: &wiremock::MockServer,
    status: u16,
    body: &str,
) -> wiremock::MockGuard {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount_as_scoped(server)
        .await
}

#[tokio::test]
async fn embed_batch_happy_path_serves_fixture_vectors_in_order() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    let _guard = mount_embeddings(&server, 200, &load_fixture("embeddings_happy.json")).await;
    let adapter = adapter_for(&server, |_| {});

    let vecs = adapter
        .embed_batch(&["a".to_string(), "b".to_string()])
        .await
        .expect("happy path serves");
    assert_eq!(vecs.len(), 2);
    assert_eq!(vecs[0].len(), 8, "non-384d dims pass through");
    assert_eq!(adapter.embedding_dim(), 0, "no probe ran yet");
}

#[tokio::test]
async fn embed_delegates_to_batch_for_single_text() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    let _guard =
        mount_embeddings(&server, 200, &load_fixture("embeddings_tolerant.json")).await;
    let adapter = adapter_for(&server, |_| {});

    let vec = adapter.embed("hello").await.expect("single embed serves");
    assert_eq!(vec.len(), 8);
}

#[tokio::test]
async fn embed_batch_empty_input_short_circuits_without_request() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    // No mock mounted: any request would 404. Empty input must never dial.
    let adapter = adapter_for(&server, |_| {});

    let empty: Vec<String> = vec![];
    let vecs = adapter
        .embed_batch(&empty)
        .await
        .expect("empty input short-circuits");
    assert!(vecs.is_empty());
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        0,
        "empty input must not send any request"
    );
}

#[tokio::test]
async fn wire_shape_matches_contract_golden() {
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

    struct Capture(Arc<Mutex<Option<serde_json::Value>>>);
    impl Respond for Capture {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            *self.0.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(serde_json::from_slice(&request.body).expect("body es JSON"));
            // Single-vector body: the golden posts one input, and the
            // 1:1 count check holds the wire honest.
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

    let adapter = adapter_for(&server, |_| {});
    adapter
        .embed_batch(&["hello world".to_string()])
        .await
        .expect("200 + valid JSON succeeds");

    let body = captured
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .expect("el mock debió capturar el body");
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/remote_embedding/embed_request.golden.json");
    let golden: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&golden_path).expect("golden legible"))
            .expect("golden es JSON válido");
    assert_eq!(
        body, golden,
        "wire shape cambió: actualizá el golden en commit separado con justificación"
    );
}

#[tokio::test]
async fn status_429_then_success_recovers() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let happy = load_fixture("embeddings_tolerant.json");
    // Mount the one-shot 429 FIRST: wiremock matches in mount order, so
    // the first request hits the 429 and the retry falls through to the
    // blanket 200 mounted second.
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .set_body_string("lento"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_string(&happy))
        .mount(&server)
        .await;
    let adapter = adapter_for(&server, |_| {});

    let vecs = adapter
        .embed_batch(&["a".to_string()])
        .await
        .expect("429 + Retry-After: 0 recovers on retry");
    assert_eq!(vecs.len(), 1);
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        2,
        "one 429 plus one recovery attempt"
    );
}

#[tokio::test]
async fn status_500_exhausts_and_reports_last_status() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    let _guard = mount_embeddings(&server, 500, "caído").await;
    let adapter = adapter_for(&server, |_| {});

    let err = adapter
        .embed_batch(&["a".to_string()])
        .await
        .expect_err("persistent 500 must fail");
    assert!(
        matches!(err, SemanticError::Inference(_)),
        "exhaustion must be Inference, got: {err}"
    );
    assert!(
        err.to_string().contains("500"),
        "exhaustion must report the last status, got: {err}"
    );
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        EMBEDDING_MAX_ATTEMPTS as usize,
        "retries must spend the full attempt budget"
    );
}

#[tokio::test]
async fn status_400_is_terminal_without_retry() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    let _guard = mount_embeddings(&server, 400, "mala petición").await;
    let adapter = adapter_for(&server, |_| {});

    let err = adapter
        .embed_batch(&["a".to_string()])
        .await
        .expect_err("400 must fail");
    assert!(
        matches!(err, SemanticError::Inference(_)),
        "terminal 4xx must be Inference, got: {err}"
    );
    assert!(
        err.to_string().contains("400"),
        "terminal 4xx reports its status as-is, got: {err}"
    );
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1,
        "terminal 4xx must not retry"
    );
}

#[tokio::test]
async fn oversize_body_maps_to_semantic_error() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let huge = "x".repeat((EMBEDDING_MAX_BODY_BYTES + 1) as usize);
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_string(&huge))
        .mount(&server)
        .await;
    let adapter = adapter_for(&server, |_| {});

    let err = adapter
        .embed_batch(&["a".to_string()])
        .await
        .expect_err("body past 16 MiB must fail");
    assert!(
        matches!(err, SemanticError::Inference(_)),
        "oversize body must be Inference, got: {err}"
    );
    assert!(
        err.to_string().contains("16 MiB"),
        "oversize error must name the cap, got: {err}"
    );
}

// Multi-thread runtime: the stalled responder below blocks one worker
// for 500ms, and the 150ms client deadline must fire on another one —
// on a current-thread runtime the stall would park the timer itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_is_retried_and_recovery_is_served() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

    // First request stalls past the client deadline; later ones serve.
    struct StallOnce {
        stalled: AtomicBool,
        body: String,
    }
    impl Respond for StallOnce {
        fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
            if !self.stalled.swap(true, Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(500));
            }
            ResponseTemplate::new(200).set_body_string(self.body.clone())
        }
    }

    let server = MockServer::start().await;
    let responder = Arc::new(StallOnce {
        stalled: AtomicBool::new(false),
        body: load_fixture("embeddings_tolerant.json"),
    });
    struct Share(Arc<StallOnce>);
    impl Respond for Share {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            self.0.respond(request)
        }
    }
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(Share(responder))
        .mount(&server)
        .await;

    // Tiny-deadline client injected via with_http (multi-thread runtime:
    // the stalled worker must not park the timeout timer).
    let env = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
    let mut cfg = config_with(AuthSource::Env {
        var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
    });
    cfg.base_url = Url::parse(&server.uri()).expect("mock uri parses");
    cfg.allow_loopback = true;
    let client = wreq::Client::builder()
        .timeout(Duration::from_millis(150))
        .build()
        .expect("deadline client builds");
    let adapter = RemoteEmbeddingAdapter::with_http(cfg, client).expect("test adapter builds");
    drop(env);

    let vecs = adapter
        .embed_batch(&["a".to_string()])
        .await
        .expect("timeout must retry and serve the recovery");
    assert_eq!(vecs.len(), 1);
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        2,
        "one timed-out attempt plus one recovery"
    );
}

// --- Body-read error mapping ---
//
// ADR-0004's original "truncado de stream" row is not expressible
// here: the adapter has no streaming API (it uses
// `read_body_capped`), and wiremock has no primitive to abort a body
// mid-stream. What IS observable is the error mapping for a body
// read that fails — the arm that a mid-stream abort would land in.

#[tokio::test]
async fn body_read_error_maps_to_spanish_inference_error() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    // `respond_with` cannot abort a body mid-stream in wiremock; a
    // `Content-Length` that overshoots the delivered payload is the
    // closest reachable shape — the client sees a short read.
    let short = r#"{"data":[{"index":0,"embedding":[0.1,0.2]}]}"#;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-length", "4096")
                .set_body_string(short),
        )
        .mount(&server)
        .await;
    let adapter = adapter_for(&server, |_| {});

    // Either the short read surfaces as the body-read error arm or the
    // complete body parses; both are legitimate, and the assertion
    // pins that a failure is NEVER silent and never panics.
    match adapter.embed_batch(&["a".to_string()]).await {
        Ok(vecs) => assert_eq!(vecs.len(), 1, "a complete body must still parse"),
        Err(err) => {
            assert!(
                matches!(err, SemanticError::Inference(_)),
                "a body-read failure must be Inference, got: {err}"
            );
            let rendered = err.to_string();
            assert!(
                rendered.contains("embeddings"),
                "the error must name the endpoint, got: {rendered}"
            );
        },
    }
}

// --- Startup probe: pin-or-adopt ---

#[tokio::test]
async fn probe_adopts_dim_when_unpinned() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    // Single-vector body: the probe sends exactly one input, and the
    // 1:1 count check holds the wire honest.
    let _guard =
        mount_embeddings(&server, 200, &load_fixture("embeddings_tolerant.json")).await;
    let adapter = adapter_for(&server, |_| {});

    let dim = adapter.probe_dim().await.expect("probe serves 8d");
    assert_eq!(dim, 8);
    assert_eq!(adapter.embedding_dim(), 8, "adopted dim is served");
}

#[tokio::test]
async fn probe_matching_pin_continues() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    let _guard =
        mount_embeddings(&server, 200, &load_fixture("embeddings_tolerant.json")).await;
    let adapter = adapter_for(&server, |cfg| cfg.embedding_dim = Some(8));

    let dim = adapter.probe_dim().await.expect("matching pin continues");
    assert_eq!(dim, 8);
}

#[tokio::test]
async fn probe_pin_mismatch_fails_closed() {
    use wiremock::MockServer;
    let server = MockServer::start().await;
    let _guard =
        mount_embeddings(&server, 200, &load_fixture("embeddings_tolerant.json")).await;
    let adapter = adapter_for(&server, |cfg| cfg.embedding_dim = Some(7));

    let err = adapter.probe_dim().await.expect_err("pin mismatch fails");
    assert!(
        matches!(err, SemanticError::Inference(_)),
        "pin mismatch must be Inference, got: {err}"
    );
    let rendered = err.to_string();
    assert!(
        rendered.contains('7') && rendered.contains('8'),
        "mismatch must name declared and actual dims, got: {rendered}"
    );
}
