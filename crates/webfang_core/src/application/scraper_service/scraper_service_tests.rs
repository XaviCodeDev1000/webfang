use super::*;
use crate::domain::http_port::HttpResponse;
use crate::infrastructure::observability::test_log_capture::LogBuffer;
use std::collections::HashMap;
use std::pin::Pin;
use tracing_subscriber::layer::SubscriberExt;

/// Canned `HttpClientPort` — the scrape use case is exercised against
/// fixed bodies, never the network.
struct StubClient {
    status: u16,
    body: String,
}

impl HttpClientPort for StubClient {
    fn get(
        &self,
        _url: &str,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = crate::domain::http_error::HttpResult<HttpResponse>>
                + Send
                + '_,
        >,
    > {
        let status = self.status;
        let body = self.body.clone();
        Box::pin(async move {
            Ok(HttpResponse {
                status,
                body,
                headers: HashMap::new(),
            })
        })
    }
}

/// Run `body` on its own current-thread runtime under a real
/// `FileTraceLayer` and parse the emitted JSONL (same shape as the
/// `rate_limiter` and `file_trace_layer` span-outcome tests, #1610).
fn capture_trace<F, Fut>(body: F) -> Vec<serde_json::Value>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("trace.jsonl");
    let layer = crate::infrastructure::observability::FileTraceLayer::new(path.clone())
        .expect("trace layer");
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    tracing::dispatcher::with_default(&dispatch, || runtime.block_on(body()));
    drop(runtime);
    // Flush before reading: the layer's BufWriter is only drained on drop.
    drop(dispatch);
    std::fs::read_to_string(&path)
        .expect("trace file")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
        .collect()
}

/// Every `span_close` record for `name`.
fn span_closes(records: &[serde_json::Value], name: &str) -> Vec<serde_json::Value> {
    records
        .iter()
        .filter(|r| r["record"] == "span_close" && r["span"] == name)
        .cloned()
        .collect()
}

/// Prose long enough to clear the minimum-content guard (#706), so the
/// test exercises the SUCCESS arm of the outcome contract rather than the
/// extraction failure it would otherwise report.
const ARTICLE: &str = "<html><head><title>Observability</title></head><body><article>\
    <h1>Span outcomes</h1>\
    <p>A hot path that cannot say whether it worked forces every operator to reassemble \
    the answer from neighbouring events, which is slow, lossy, and impossible to automate \
    without a parser per call site.</p>\
    <p>Declaring the outcome on the span itself makes the verdict a queryable field of the \
    record that already describes the operation, so an offline run can be classified \
    without correlating timestamps across unrelated lines of the same file.</p>\
    </article></body></html>";

/// #1610 (OBS-P2-4): a successful scrape must close its span carrying
/// `outcome = "ok"` and the produced-document count. Before the outcome
/// contract the span closed with a duration and a URL, and "did this page
/// work" was only answerable from neighbouring events.
///
/// Miri gate (#2187): a SUCCESSFUL scrape is the only arm of this module
/// that reaches `domain::html_cleaner::clean_html` → `lol_html::rewrite_str`,
/// whose drop glue ends in the `servo_arc` 0.4.3 `ArcUnion` drop — a Tree
/// Borrows violation (`fetch_sub` on a Frozen tag) that aborts the whole
/// Miri process, not one case. Same defect family as the already-gated
/// siblings: `application/extraction.rs` (via lol_html), `application/
/// spa_detection.rs` (via lol_html) and `application/crawler/discovery.rs`
/// (via lol_html). The error arm below returns before `clean_html` and
/// needs no gate.
#[cfg_attr(
    miri,
    ignore = "servo_arc Tree Borrows UB in lol_html::rewrite_str via clean_html (#2187)"
)]
#[test]
fn scrape_span_records_success_outcome_and_result_count() {
    // The page identity is a CHILD of this root, so the trace must match
    // the caller's — a late outcome record must not disturb it.
    let root = CorrelationId::new();
    let root_trace = root.trace_id().to_string();
    let records = capture_trace(|| async {
        let client = StubClient {
            status: 200,
            body: ARTICLE.to_owned(),
        };
        let url: url::Url = "https://example.com/post".parse().expect("valid URL");
        let outcome = scrape_with_config(
            &client,
            &url,
            &ScraperConfig::default(),
            None,
            None,
            None,
            None,
            true,
            &root,
        )
        .await
        .expect("scrape must succeed");
        assert_eq!(outcome.results.len(), 1, "one document scraped");
    });

    let closes = span_closes(&records, "scrape_with_config");
    assert_eq!(closes.len(), 1, "exactly one scrape span must close");
    let fields = closes[0]["span_fields"]
        .as_object()
        .expect("span_fields object");
    assert_eq!(
        fields.get("outcome").and_then(serde_json::Value::as_str),
        Some("ok"),
        "recorded outcome must land in span_fields"
    );
    assert_eq!(
        fields.get("results").and_then(serde_json::Value::as_u64),
        Some(1),
        "the produced-document count must be recorded"
    );
    assert_eq!(
        fields.get("trace_id").and_then(serde_json::Value::as_str),
        Some(root_trace.as_str()),
        "the page identity must survive alongside the late records"
    );
}

/// #1610: the failure arm is the one operators search for. A 404 must
/// close the span as `outcome = "error"` with the classification in the
/// repo's snake_case `ErrorClass` spelling, never a formatted message.
#[test]
fn scrape_span_records_error_outcome_with_error_class() {
    let records = capture_trace(|| async {
        let client = StubClient {
            status: 404,
            body: "not found".to_owned(),
        };
        let url: url::Url = "https://example.com/missing".parse().expect("valid URL");
        let result = scrape_with_config(
            &client,
            &url,
            &ScraperConfig::default(),
            None,
            None,
            None,
            None,
            true,
            &CorrelationId::new(),
        )
        .await;
        assert!(result.is_err(), "a 404 must still be an error");
    });

    let closes = span_closes(&records, "scrape_with_config");
    assert_eq!(closes.len(), 1, "exactly one scrape span must close");
    let fields = closes[0]["span_fields"]
        .as_object()
        .expect("span_fields object");
    assert_eq!(
        fields.get("outcome").and_then(serde_json::Value::as_str),
        Some("error")
    );
    assert_eq!(
        fields
            .get("error_class")
            .and_then(serde_json::Value::as_str),
        Some("permanent_fatal"),
        "the 404 must classify with the shared ErrorClass spelling"
    );
    assert!(
        !fields.contains_key("results"),
        "a failed scrape must not report a document count"
    );
}

/// #1604: a failed batch URL emits the shared operational error contract
/// through `log_scrape_error` — `error`, `url`, `stage`, and the batch
/// run-root `trace_id` — not a bare interpolated warn.
#[test]
fn batch_failure_emits_log_scrape_error_contract() {
    let (buf, _guard) = LogBuffer::capture();

    let root = CorrelationId::new();
    let url: url::Url = "https://example.com/batch-fail".parse().unwrap();
    let results = vec![(
        url.clone(),
        Err::<ScrapeOutcome, ScraperError>(ScraperError::GlobalTimeout),
    )];

    let outcome = super::batch::collect_batch_outcome(results, &root);

    assert_eq!(outcome.failed.len(), 1, "failure must be recorded");
    assert_eq!(outcome.failed[0].url, url.to_string());
    let out = buf.text();
    assert!(out.contains("ERROR"), "must be ERROR level: {out}");
    assert!(
        out.contains("batch URL scrape failed"),
        "context message: {out}"
    );
    assert!(out.contains(&url.to_string()), "url field: {out}");
    assert!(out.contains("stage=scrape"), "stage field: {out}");
    assert!(
        out.contains(&root.trace_id().to_string()),
        "trace_id field: {out}"
    );
}
