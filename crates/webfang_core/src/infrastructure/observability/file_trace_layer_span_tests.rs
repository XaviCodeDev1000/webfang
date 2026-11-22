use super::tests::{event_records, parse_single_event, read_jsonl_lines, span_close_records};
use super::*;
use serde_json::Value;
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn contract_span_field_present_inside_span() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!("my_op", request_id = 99);
        let _enter = span.enter();
        tracing::info!("inside");
    });

    let parsed = parse_single_event(&path);
    assert_eq!(
        parsed["span"].as_str(),
        Some("my_op"),
        "span name must be set inside a span"
    );
    assert!(
        parsed["span_id"].is_string(),
        "span_id must be present inside a span"
    );
}

#[test]
fn contract_captures_span_fields_from_scope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        // Root span carries the seed URL — the key field for offline
        // telemetry (S2). A nested child adds its own field.
        let root = tracing::info_span!("root_span", seed_url = "https://example.com");
        let _rg = root.enter();
        let child = tracing::info_span!("child_span", depth = 1);
        let _cg = child.enter();
        tracing::info!("inside child");
    });

    let parsed = parse_single_event(&path);
    let span_fields = parsed["span_fields"]
        .as_object()
        .expect("span_fields must be present when a span has fields");
    assert_eq!(
        span_fields["seed_url"],
        json!("https://example.com"),
        "root span seed_url must be captured via the span scope"
    );
    assert_eq!(
        span_fields["depth"],
        json!(1),
        "child span field must also be captured"
    );
}

/// Issue #501 regression: correlation identity declared at span creation
/// (the exact pattern of `scrape_single` / `crawl_page` /
/// `scrape_with_config`) must surface as `span_fields.correlation_id` and
/// `span_fields.trace_id` on every event inside the span. FileTraceLayer
/// snapshots fields in `on_new_span`, so anything not declared at creation
/// time is invisible offline — this test pins that contract.
#[test]
fn contract_captures_correlation_identity_declared_at_span_creation() {
    use crate::domain::value_objects::CorrelationId;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    // Fixed IDs keep the assertions exact-equality and deterministic.
    let trace_id = uuid::Uuid::parse_str("01949e0e-8b8e-7000-8000-000000000001").unwrap();
    let correlation = CorrelationId::new_with_ids(trace_id, 0x0000_0000_0000_0042);
    let expected_traceparent = correlation.to_string();

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!(
            "scrape_single",
            url = "https://example.com/page",
            correlation_id = %correlation,
            trace_id = %correlation.trace_id()
        );
        let _enter = span.enter();
        tracing::info!("fetching page");
    });

    let parsed = parse_single_event(&path);
    let span_fields = parsed["span_fields"]
        .as_object()
        .expect("span_fields must be present for spans declaring identity");
    assert_eq!(
        span_fields["correlation_id"],
        json!(expected_traceparent),
        "correlation_id must equal the W3C traceparent declared at span creation"
    );
    assert_eq!(
        span_fields["trace_id"],
        json!(trace_id.to_string()),
        "trace_id must equal the correlation's UUID declared at span creation"
    );
}

#[test]
fn contract_span_field_absent_outside_span() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("no span");
    });

    let parsed = parse_single_event(&path);
    assert!(
        parsed["span"].is_null(),
        "span must be null when not inside a span, got: {:?}",
        parsed["span"]
    );
}

#[test]
fn contract_nested_spans_track_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let outer = tracing::info_span!("outer");
        let _og = outer.enter();
        tracing::info!("in outer");

        let inner = tracing::info_span!("inner");
        let _ig = inner.enter();
        tracing::info!("in inner");
        drop(_ig);

        tracing::info!("back to outer");
    });

    let parsed = event_records(&path);
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0]["span"], "outer");
    assert_eq!(parsed[1]["span"], "inner");
    assert_eq!(parsed[2]["span"], "outer");
}

/// #1610 (OBS-P2-4): a field declared `Empty` and filled in with
/// `Span::record` after the span was created MUST reach the JSONL.
///
/// This is the span-outcome contract: `#[instrument]` can only see
/// arguments, so an outcome declared at creation is empty and recorded at
/// the end of the operation. Before `on_record` existed the value was
/// dropped and the span closed with no outcome at all.
#[test]
fn contract_span_close_carries_late_recorded_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!(
            "crawl_page",
            url = "https://example.com",
            outcome = tracing::field::Empty,
            http_status = tracing::field::Empty,
        );
        let _enter = span.enter();
        span.record("outcome", "ok");
        span.record("http_status", 200u64);
    });

    let closes = span_close_records(&path);
    assert_eq!(closes.len(), 1, "exactly one span_close record expected");
    let fields = closes[0]["span_fields"]
        .as_object()
        .expect("span_fields must be an object");
    assert_eq!(
        fields.get("outcome").and_then(Value::as_str),
        Some("ok"),
        "recorded outcome must land in span_fields"
    );
    assert_eq!(
        fields.get("http_status").and_then(Value::as_u64),
        Some(200),
        "recorded numeric field must land as a number"
    );
    assert_eq!(
        fields.get("url").and_then(Value::as_str),
        Some("https://example.com"),
        "creation-time fields must survive the merge"
    );
}

/// #1610 (OBS-P2-4): a field recorded mid-span must also reach the
/// `span_fields` of every event emitted AFTER the record — otherwise a
/// failure event raised while the outcome was still unknown would show a
/// half-populated span.
#[test]
fn contract_late_recorded_field_visible_on_later_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!(
            "crawl_page",
            url = "https://example.com",
            outcome = tracing::field::Empty,
        );
        let _enter = span.enter();
        span.record("outcome", "error");
        tracing::info!(page_failed = true, "page failed");
    });

    let events: Vec<Value> = read_jsonl_lines(&path)
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["record"].is_null())
        .collect();
    assert_eq!(events.len(), 1, "one event expected");
    let fields = events[0]["span_fields"]
        .as_object()
        .expect("span_fields must be an object");
    assert_eq!(
        fields.get("outcome").and_then(Value::as_str),
        Some("error"),
        "later event must see the recorded outcome"
    );
}

#[test]
fn contract_span_close_emits_duration_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!("op");
        let _enter = span.enter();
    });

    let closes = span_close_records(&path);
    assert_eq!(closes.len(), 1, "exactly one span_close record expected");
    assert_eq!(closes[0]["span"], "op");
    let dur = &closes[0]["span_duration_ms"];
    assert!(
        dur.is_number(),
        "span_duration_ms must be a number, got: {dur}"
    );
    assert!(
        dur.as_f64().is_some_and(|d| d >= 0.0),
        "span_duration_ms must be >= 0, got: {dur}"
    );
}

#[test]
fn contract_span_close_has_correlation_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!("op");
        let _enter = span.enter();
    });

    let closes = span_close_records(&path);
    assert_eq!(closes.len(), 1);
    let span_id = closes[0]["span_id"]
        .as_str()
        .expect("span_id must be a string");
    assert_eq!(span_id.len(), 16, "span_id must be 16 hex chars");
    assert!(
        span_id.chars().all(|c| c.is_ascii_hexdigit()),
        "span_id must be hex, got: {span_id}"
    );
    let trace_id = closes[0]["trace_id"]
        .as_str()
        .expect("trace_id must be a string");
    assert_eq!(trace_id.len(), 16, "trace_id must be 16 hex chars");
    assert!(
        trace_id.chars().all(|c| c.is_ascii_hexdigit()),
        "trace_id must be hex, got: {trace_id}"
    );
}

#[test]
fn contract_nested_span_close_has_parent_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let outer = tracing::info_span!("outer");
        let _og = outer.enter();
        let inner = tracing::info_span!("inner");
        let _ig = inner.enter();
    });

    let closes = span_close_records(&path);
    let inner = closes
        .iter()
        .find(|v| v["span"] == "inner")
        .expect("inner close record must exist");
    let outer = closes
        .iter()
        .find(|v| v["span"] == "outer")
        .expect("outer close record must exist");
    assert_eq!(
        inner["parent_id"], outer["span_id"],
        "inner parent_id must equal outer span_id"
    );
}

#[test]
fn contract_span_close_captures_span_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!("op", seed_url = "https://example.com");
        let _enter = span.enter();
    });

    let closes = span_close_records(&path);
    assert_eq!(closes.len(), 1);
    assert_eq!(
        closes[0]["span_fields"]["seed_url"],
        json!("https://example.com"),
        "span field must be captured on close"
    );
}

#[test]
fn contract_span_close_duration_reflects_elapsed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!("slow_op");
        let _enter = span.enter();
        std::thread::sleep(std::time::Duration::from_millis(15));
    });

    let closes = span_close_records(&path);
    assert_eq!(closes.len(), 1);
    let dur = closes[0]["span_duration_ms"]
        .as_u64()
        .expect("span_duration_ms must be a non-negative integer");
    assert!(
        dur >= 10,
        "duration must reflect the ~15ms sleep (>= 10ms margin), got: {dur}"
    );
}
