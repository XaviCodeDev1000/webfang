use super::*;
use serde_json::Value;
use std::path::Path;
use tracing_subscriber::layer::SubscriberExt;

// Each test creates its own Dispatch to avoid cross-test interference.

#[test]
fn contract_creates_file_at_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let _layer = FileTraceLayer::new(path.clone()).unwrap();
    assert!(
        path.exists(),
        "trace file must be created at the given path"
    );
}

#[test]
fn contract_creates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join("trace.jsonl");
    let _layer = FileTraceLayer::new(path.clone()).unwrap();
    assert!(path.exists(), "parent directories must be created");
}

#[test]
fn contract_emits_valid_jsonl_per_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("hello");
    });

    let lines = read_jsonl_lines(&path);
    assert_eq!(lines.len(), 1, "should have exactly one JSONL line");
    let parsed: Value = serde_json::from_str(&lines[0]).unwrap();
    assert!(parsed.is_object(), "line must be a JSON object");
}

#[test]
fn contract_timestamp_is_rfc3339() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("check timestamp");
    });

    let parsed = parse_single_event(&path);
    let ts = parsed["timestamp"]
        .as_str()
        .expect("timestamp must be a string");
    assert!(ts.ends_with('Z'), "timestamp must end with Z, got: {ts}");
    assert!(
        ts.contains('T'),
        "timestamp must contain T separator, got: {ts}"
    );
    assert!(
        ts.len() >= 20,
        "timestamp must be full ISO format, got: {ts}"
    );
    let parts: Vec<&str> = ts.split('T').collect();
    assert_eq!(parts.len(), 2, "must have date and time separated by T");
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    assert_eq!(date_parts.len(), 3, "date must be YYYY-MM-DD");
    assert_eq!(date_parts[0].len(), 4, "year must be 4 digits");
}

#[test]
fn contract_emits_level_target_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::warn!(target: "my_target", "oops");
    });

    let parsed = parse_single_event(&path);
    assert_eq!(parsed["level"], "WARN");
    assert_eq!(parsed["target"], "my_target");
    assert_eq!(parsed["message"], "oops");
}

#[test]
fn contract_emits_trace_id_and_span_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!("test_span");
        let _enter = span.enter();
        tracing::info!("with ids");
    });

    let parsed = parse_single_event(&path);
    let trace_id = parsed["trace_id"]
        .as_str()
        .expect("trace_id must be present");
    assert_eq!(trace_id.len(), 16, "trace_id must be 16 hex chars");
    assert!(
        parsed["span_id"].as_str().is_some(),
        "span_id must be present inside a span, got: {:?}",
        parsed["span_id"]
    );
    let span_id = parsed["span_id"].as_str().unwrap();
    assert_eq!(span_id.len(), 16, "span_id must be 16 hex chars");
}

#[test]
fn contract_emits_structured_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!(request_id = 42, user = "alice", "processing");
    });

    let parsed = parse_single_event(&path);
    let fields = parsed["fields"]
        .as_object()
        .expect("fields must be a JSON object");
    assert_eq!(fields["request_id"], json!(42));
    assert_eq!(fields["user"], json!("alice"));
}

#[test]
fn contract_message_not_duplicated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("unique_msg_12345");
    });

    let parsed = parse_single_event(&path);
    assert_eq!(parsed["message"], "unique_msg_12345");
    if let Some(fields) = parsed["fields"].as_object() {
        assert!(
            !fields.contains_key("message"),
            "message must not be duplicated in fields"
        );
    }
}

#[test]
fn contract_multiple_events_append_sequentially() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("first");
        tracing::debug!("second");
        tracing::error!("third");
    });

    let parsed = parse_lines(&path);
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0]["message"], "first");
    assert_eq!(parsed[0]["level"], "INFO");
    assert_eq!(parsed[1]["message"], "second");
    assert_eq!(parsed[1]["level"], "DEBUG");
    assert_eq!(parsed[2]["message"], "third");
    assert_eq!(parsed[2]["level"], "ERROR");
}

#[test]
fn contract_thread_safety_concurrent_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    let mut handles = vec![];
    for i in 0..10 {
        let d = dispatch.clone();
        handles.push(std::thread::spawn(move || {
            tracing::dispatcher::with_default(&d, || {
                tracing::info!(thread_id = i, "msg from thread {i}");
            });
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let lines = read_jsonl_lines(&path);
    assert_eq!(lines.len(), 10, "must have 10 lines from 10 threads");

    for (i, line) in lines.iter().enumerate() {
        let parsed: Value = serde_json::from_str(line).unwrap();
        assert!(parsed.is_object(), "line {i} must be a JSON object");
        let fields = parsed["fields"]
            .as_object()
            .expect("fields must be present");
        assert!(
            fields.contains_key("thread_id"),
            "thread_id field must be captured, missing in line {i}"
        );
    }
}

#[test]
fn contract_truncates_on_creation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    std::fs::write(&path, "old data\n").unwrap();

    let _layer = FileTraceLayer::new(path.clone()).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.is_empty(),
        "file must be truncated on creation, got: {content:?}"
    );
}

#[test]
fn contract_drop_flushes_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("buffered event");
    });

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.is_empty(), "Drop must flush buffered data to disk");
}

pub(super) fn read_jsonl_lines(path: &Path) -> Vec<String> {
    let content = std::fs::read_to_string(path).unwrap();
    content
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

pub(super) fn parse_single_event(path: &Path) -> Value {
    let events = event_records(path);
    assert_eq!(events.len(), 1, "expected exactly 1 event record");
    events.into_iter().next().unwrap()
}

pub(super) fn parse_lines(path: &Path) -> Vec<Value> {
    read_jsonl_lines(path)
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Event records only — span-close records carry a `"record"` discriminator,
/// event records do not.
pub(super) fn event_records(path: &Path) -> Vec<Value> {
    parse_lines(path)
        .into_iter()
        .filter(|v| v.get("record").is_none())
        .collect()
}

/// Span-close records only (those with `"record": "span_close"`).
pub(super) fn span_close_records(path: &Path) -> Vec<Value> {
    parse_lines(path)
        .into_iter()
        .filter(|v| v["record"] == "span_close")
        .collect()
}
