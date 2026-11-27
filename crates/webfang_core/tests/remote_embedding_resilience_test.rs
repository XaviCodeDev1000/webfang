#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Resilience of `RemoteEmbeddingAdapter` against a hostile remote endpoint.
//!
//! ## Why these rows are integration tests and not unit tests
//!
//! `remote_embedding.rs` already has a unit-test module covering the pure layer
//! (body parsing, backoff policy) and several wire rows. What it **did not**
//! cover — and what ADR-0004 requires — is the server-side half of the guard
//! chain: retry classification when the endpoint answers 429 with
//! `Retry-After`, sustained 5xx, a degenerate body, batch order and an
//! unreadable name. These rows enter through the public port (`EmbeddingPort`
//! over `webfang_core`), which is what the CLI and the MCP actually use, so a
//! regression in the signature or on the wire breaks them even while the
//! internal module stays green.
//!
//! ## What these rows pin (ADR-0004 §Review condition, PR-8 wiremock suite)
//!
//! 1. 429 with `Retry-After` in seconds → recovers after honoring it.
//! 2. 429 with `Retry-After` as an HTTP date → honored on the wire: an
//!    already-past date clamps to zero (and the retry leaves in ~1s, the
//!    exponential floor) instead of sleeping for the ~32 years the header
//!    asks for; an unusable header degrades to exponential backoff without
//!    turning the retry into an error. The pure conversion —both RFC 9110
//!    §10.2.3 forms, with `now` injected— is covered in the in-crate test
//!    module of `remote_embedding.rs`, where a pure private function belongs;
//!    what is asserted here is the behavior observable through the public port.
//! 3. Sustained 5xx → spends exactly `EMBEDDING_MAX_ATTEMPTS` attempts and
//!    reports the **last** observed status, never a fixed one.
//! 4. Malformed response (invalid JSON, empty `data`, count mismatch) →
//!    `SemanticError::Inference` with the user-facing English message.
//! 5. Batch order → `input` keeps the input order and vectors come back
//!    positionally 1:1; `index` is parsed but **does not** re-sort.
//! 6. DNS → reserved TLD name: transport error, never a panic.

use std::time::Duration;

use webfang_core::domain::providers::ProviderConfig;
use webfang_core::domain::EmbeddingPort;
use webfang_core::error::SemanticError;
use webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Path of the embedding endpoint, shared by the mocks and by the `path()`
/// matcher: a string repeated in two places is a string that can drift out of
/// sync without anything failing.
const EMBEDDINGS_PATH: &str = "/embeddings";

/// TLD reserved by RFC 2606: `nonexistent.invalid` **never** resolves, so the
/// DNS row depends neither on the real network nor on an environment wildcard.
/// (Unlike an invented `.com` name, which could be captured by a wildcard
/// resolver.)
const UNRESOLVABLE_HOST: &str = "http://nonexistent.invalid/v1";

/// Anonymous embedding `ProviderConfig` pointing at the wiremock.
///
/// `AuthSource::None` rather than an `EnvGuard`: this suite asserts nothing
/// about credentials (that is `auth_source_none_test.rs` territory), so it
/// avoids mutating the environment and keeps a one-line Arrange.
fn anonymous_config(base_url: String) -> ProviderConfig {
    ProviderConfig {
        id: "resilience-endpoint".to_string(),
        display_name: "Resilience Endpoint".to_string(),
        base_url: url::Url::parse(&base_url).expect("base_url parses"),
        // T7: a cold adapter now fails closed before dialing. These cases
        // use the one-dimensional `one_d` fixture, so the contract width is 1.
        embedding_dim: Some(1),
        // Loopback is the wiremock; the config opt-in is what allows it.
        allow_loopback: true,
        ..webfang_test_utils::test_provider_config()
    }
}

/// A one-dimensional vector, with a value that identifies it: positional
/// ordering is asserted over distinct values, not over equal lengths.
fn one_d(scalar: f32) -> String {
    format!(r#"{{"data":[{{"embedding":[{scalar}]}}]}}"#)
}

/// Counts the requests the server actually saw.
async fn request_count(server: &MockServer) -> usize {
    server.received_requests().await.unwrap_or_default().len()
}

/// The anonymous adapter under test, already built against the server.
fn adapter_for(server: &MockServer) -> RemoteEmbeddingAdapter {
    RemoteEmbeddingAdapter::new(anonymous_config(server.uri())).expect("anonymous adapter builds")
}

/// Mounts a constant `status` on the embedding endpoint.
async fn mount_status(server: &MockServer, status: u16, body: &str) {
    Mock::given(method("POST"))
        .and(path(EMBEDDINGS_PATH))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

/// Mounts a **one-shot** 429 followed by a courtesy 200.
///
/// The 429 is mounted first because wiremock evaluates mocks in mount order:
/// the first request receives the 429 and the retry falls through to the 200
/// mounted afterwards. `retry_after` is the raw header value — `None` omits
/// it, which is the case of a server that does not send one.
async fn mount_429_then_ok(server: &MockServer, retry_after: Option<&str>) {
    let mut throttled = ResponseTemplate::new(429);
    if let Some(value) = retry_after {
        throttled = throttled.insert_header("retry-after", value);
    }
    Mock::given(method("POST"))
        .and(path(EMBEDDINGS_PATH))
        .respond_with(throttled.set_body_string("rate limit reached"))
        .up_to_n_times(1)
        .mount(server)
        .await;
    mount_status(server, 200, &one_d(0.1)).await;
}

// ---------------------------------------------------------------------------
// 1. 429 with Retry-After in seconds
// ---------------------------------------------------------------------------

/// A 429 with `Retry-After` in delay-seconds is honored **above** the
/// exponential floor, and the retry recovers when the endpoint returns 200.
///
/// The value is `2`, not `1`, for a reason this row must demonstrate: the
/// exponential backoff of the first attempt is exactly 1000 ms, so a
/// `Retry-After: 1` would be indistinguishable from ignoring it — the row
/// would pass whether the fix was in place or not. With `2` the two hypotheses
/// diverge (2000 ms honored versus 1000 ms discarded) and the lower-floor
/// assertion decides between them. That is why the test measures time: it is
/// the only signal through which honoring the wire is observable from the
/// outside, since the pure conversion lives in an in-crate module.
#[tokio::test]
async fn status_429_with_retry_after_seconds_recovers_on_retry() {
    let server = MockServer::start().await;
    mount_429_then_ok(&server, Some("2")).await;
    let started = std::time::Instant::now();

    let vecs = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect("429 + Retry-After: 2 recovers on the next attempt");

    assert_eq!(vecs.len(), 1);
    assert_eq!(
        request_count(&server).await,
        2,
        "exactly one 429 plus one recovery attempt"
    );
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "the server asked for 2s, which must beat the 1000ms exponential \
         floor — a faster retry means Retry-After was discarded; took {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// 2. 429 with Retry-After as an HTTP date
// ---------------------------------------------------------------------------

/// The HTTP-date wire row: a 429 sending an **already-past** date must clamp
/// to zero and recover, instead of trying to sleep for the ~32-year delta (or,
/// in the old code, falling back to backoff without saying anything). The
/// assertion is two-sided: on the lower side, one second is the exponential
/// floor and proves there **was** a real retry and not a hot loop; on the
/// upper side, thirty seconds is the ceiling —an order of magnitude above the
/// real ~1s, with headroom for a loaded CI runner— and proves the date was not
/// honored literally: if it were, the test would never finish.
///
/// What this row **cannot** see is whether the clamp was to zero or a fall
/// back to exponential backoff: on the wire both produce ~1s. That distinction
/// is pinned by the pure test in the in-crate module, with `now` injected;
/// what is asserted here is the observable behavior.
#[tokio::test]
async fn status_429_with_past_http_date_clamps_and_recovers() {
    let server = MockServer::start().await;
    mount_429_then_ok(&server, Some("Sun, 06 Nov 1994 08:49:37 GMT")).await;
    let started = std::time::Instant::now();

    adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect("a past HTTP-date clamps to zero and the retry recovers");

    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(30),
        "a past HTTP-date must clamp to the exponential floor (~1s), not to \
         the ~32-year delta the header literally asks for; the 30s bound is an \
         order of magnitude above the real ~1s and leaves headroom on a loaded \
         CI runner — the discriminating power is the magnitude, not the exact \
         ceiling; took {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(900),
        "the retry must go through the backoff floor, not a hot loop; took {elapsed:?}"
    );
    assert_eq!(request_count(&server).await, 2);
}

/// An unusable `Retry-After` header degrades to exponential backoff and
/// **says so** — the structured `warn!` carries `provider_id`, `raw` and
/// `reason`. What is observable from the outside is that the row neither hangs
/// nor fails: the degradation is a retry, never an error.
#[tokio::test]
async fn unparsable_retry_after_degrades_without_failing_the_request() {
    let server = MockServer::start().await;
    mount_429_then_ok(&server, Some("pronto")).await;

    let vecs = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect("an unusable Retry-After degrades to backoff, it is not an error");

    assert_eq!(vecs.len(), 1);
    assert_eq!(request_count(&server).await, 2);
}

/// The row that **does** distinguish the fix from the regression.
///
/// The other two rows in this section use values that do not separate the two
/// hypotheses: a `retry-after: "1"` coincides with the exponential floor, and
/// an already-past date produces ~1s whether it is clamped to zero or
/// discarded. What separates "honored" from "ignored" is a **future** date:
/// honored it sleeps for what it asks, ignored it falls back to exponential
/// backoff.
///
/// The header is built at `now + 4s` with `chrono`, and the assertion asks for
/// at least 3s. That margin absorbs the IMF-fixdate format's truncation to
/// seconds and the milliseconds between building the header and reading it,
/// without leaving room for the bug (which would produce ~1s).
#[tokio::test]
async fn status_429_with_future_http_date_waits_what_the_server_asked() {
    // RFC 9110 §10.2.3: IMF-fixdate is the form servers send.
    let future = chrono::Utc::now() + Duration::from_secs(4);
    let header = future.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

    let server = MockServer::start().await;
    mount_429_then_ok(&server, Some(&header)).await;
    let started = std::time::Instant::now();

    let vecs = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect("a future HTTP-date is honored and the retry recovers");

    assert_eq!(vecs.len(), 1);
    assert_eq!(request_count(&server).await, 2);
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "the server asked for ~{header}, which must beat the 1000ms \
         exponential floor — a ~1s retry means the HTTP-date was discarded \
         (the pre-fix behavior); took {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// 3. Sustained 5xx
// ---------------------------------------------------------------------------

/// A 5xx is classified as retryable and spends the full attempt budget
/// (4: the initial try plus 3 retries). The error names the **last observed**
/// status: that is the guard-chain rule ("retries exhausted report the LAST
/// observed status, never a hardcoded one"), which is why the mock returns
/// 503 — a code different from the 500 used by other rows —: if the adapter
/// burned a fixed status, this row would expose it.
#[tokio::test]
async fn persistent_5xx_spends_full_budget_and_names_the_last_status() {
    let server = MockServer::start().await;
    mount_status(&server, 503, "unavailable").await;

    let err = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect_err("a persistent 503 must fail after the attempt budget");

    assert!(
        matches!(err, SemanticError::Inference(_)),
        "exhaustion is Inference, got: {err}"
    );
    assert!(
        err.to_string().contains("503"),
        "the error must name the last observed status, got: {err}"
    );
    assert_eq!(
        request_count(&server).await,
        4,
        "the budget is the initial attempt plus three retries"
    );
}

// ---------------------------------------------------------------------------
// 4. Malformed response
// ---------------------------------------------------------------------------

/// A degenerate body is a [`SemanticError::Inference`] with the English message
/// the end user sees — not an `unwrap`, not a `panic`, and not a `200` reported
/// as a success with zero vectors.
///
/// The exact text of each case is asserted because that is the surface the
/// user reads; a generic matcher ("contains Inference") would not distinguish
/// an empty body from a count mismatch.
#[tokio::test]
async fn malformed_body_names_its_own_failure() {
    // 1. Invalid JSON: there is not even a document to parse.
    assert_malformed("this is not json", "invalid body").await;

    // 2. Empty `data`: a 200 that brings nothing to serve.
    assert_malformed(r#"{"data":[]}"#, "empty `data`").await;

    // 3. Count mismatch: 2 vectors for 1 text. The message names the served
    //    count *and* the expected one, which is what lets the operator see
    //    whether the vendor truncated.
    assert_malformed(
        r#"{"data":[{"embedding":[0.1]},{"embedding":[0.2]}]}"#,
        "2 vectors for 1 texts",
    )
    .await;
}

/// Mounts a 200 with `body`, invokes the port and checks that the failure is
/// `Inference` and that the message contains `expected_message`.
///
/// This is a helper rather than three tests because the Arrange (mounting the
/// same mock and repeating the same invocation) is identical in all three
/// cases: what varies —the degenerate body— is the data, not the scenario.
async fn assert_malformed(body: &str, expected_message: &str) {
    let server = MockServer::start().await;
    mount_status(&server, 200, body).await;

    let err = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect_err("a degenerate body must not be served as a success");

    assert!(
        matches!(err, SemanticError::Inference(_)),
        "malformed body must be Inference, got: {err}"
    );
    assert!(
        err.to_string().contains(expected_message),
        "message must name the failure cause ({expected_message}), got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 5. Batch order
// ---------------------------------------------------------------------------

/// Order is **positional**: `input` travels in the order it was passed and the
/// vectors come back in the same order as the response.
///
/// The body returned by the mock deliberately carries **reversed** `index`
/// values (`2, 1, 0`). `EmbeddingDatum::index` is parsed to tolerate each
/// vendor's wire but is intentionally unused: if anyone ever used it to
/// re-sort, this row —which returns 0.3, 0.2, 0.1 in that wire order— would
/// expose it, because the expected result is positional, not indexed.
#[tokio::test]
async fn batch_preserves_input_order_and_returns_vectors_positionally() {
    let server = MockServer::start().await;
    mount_status(
        &server,
        200,
        r#"{"data":[
            {"index":2,"embedding":[0.3]},
            {"index":1,"embedding":[0.2]},
            {"index":0,"embedding":[0.1]}
        ]}"#,
    )
    .await;

    let inputs = ["uno".to_string(), "dos".to_string(), "tres".to_string()];
    let vecs = adapter_for(&server)
        .embed_batch(&inputs)
        .await
        .expect("a well-formed batch is served");

    // The request leaves in the input order.
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        1,
        "one request per batch, never one per text"
    );
    let sent: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("the request body is JSON");
    assert_eq!(
        sent["input"],
        serde_json::json!(["uno", "dos", "tres"]),
        "input must travel in the caller's order"
    );

    // And the vectors come back positionally: in wire order, not `index`
    // order (which is deliberately reversed here).
    assert_eq!(
        vecs,
        vec![vec![0.3], vec![0.2], vec![0.1]],
        "vectors are positional 1:1; `index` is parsed but never re-sorts"
    );
}

// ---------------------------------------------------------------------------
// 6. DNS
// ---------------------------------------------------------------------------

/// A reserved TLD name does not resolve: the request fails in transport and
/// is reported with the English message that names the endpoint.
///
/// `entry_gate` lets the host through because it only checks the scheme and a
/// literal IP; resolution is the job of the guarded client's validated
/// resolver, so the failure appears in the transport layer — the same arm as
/// a connection refused.
///
/// Environment note: `.invalid` is reserved by RFC 2606 and cannot resolve,
/// so the row depends neither on the real network nor on an environment
/// wildcard. The only environment-dependent thing is the *speed* of the
/// NXDOMAIN: a resolver that delayed its answer would take the attempt to the
/// 10s `connect_timeout` — and a timeout is retryable, so the row would take
/// longer without changing its verdict.
#[tokio::test]
async fn unresolvable_host_reports_a_transport_error() {
    let adapter = RemoteEmbeddingAdapter::new(anonymous_config(UNRESOLVABLE_HOST.to_string()))
        .expect("a reserved hostname passes the entry gate: no literal IP");

    let err = adapter
        .embed_batch(&["uno".to_string()])
        .await
        .expect_err("a reserved TLD never resolves");

    assert!(
        matches!(err, SemanticError::Inference(_)),
        "a DNS failure must be Inference, got: {err}"
    );
    let rendered = err.to_string();
    assert!(
        rendered.contains("transport error"),
        "a resolution failure belongs to the transport arm, got: {rendered}"
    );
    assert!(
        rendered.contains("resilience-endpoint"),
        "the error must name the provider, got: {rendered}"
    );
}
