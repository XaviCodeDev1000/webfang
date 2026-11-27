// Test policy (#1951): this binary uses a scoped subscriber and no global
// state; `expect` is confined to fixtures and assertions.
#![cfg_attr(test, allow(clippy::expect_used))]

//! S4 observability contract: every sandbox decision reaches FileTraceLayer
//! JSONL with a top-level `trace_id` and non-empty structured fields.
//!
//! The events are emitted synchronously inside a span, so a scoped subscriber
//! on a current-thread runtime is enough — no global dispatcher, no real
//! browser, no network. The exit status comes from a deterministic shell exit.

use std::fs;
use std::path::Path;
use std::process::{Command, ExitStatus};

use serde_json::Value;
use tracing_subscriber::layer::SubscriberExt;

use webfang_core::infrastructure::downloader::obscura_downloader::{
    trace_browser_exit, trace_sandbox_applied, trace_sandbox_denials, trace_sandbox_skipped,
};
use webfang_core::infrastructure::observability::FileTraceLayer;

const TASK_ID: &str = "tenant-sandbox-trace";

fn read_events(path: &Path) -> Vec<Value> {
    let raw = fs::read_to_string(path).expect("trace file must be readable");
    raw.lines()
        .map(|line| serde_json::from_str(line).expect("trace line must be JSON"))
        .collect()
}

fn field_text(event: &Value, key: &str) -> String {
    match &event["fields"][key] {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        _ => String::new(),
    }
}

fn assert_sandbox_event<'a>(events: &'a [Value], name: &str, fields: &[&str]) -> &'a Value {
    let event = events
        .iter()
        .find(|event| event["fields"]["event"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("missing sandbox event {name} in trace"));
    for field in fields {
        let value = field_text(event, field);
        assert!(!value.is_empty(), "event {name} has empty field {field}");
    }
    let trace_id = event["trace_id"].as_str().unwrap_or_default();
    assert!(
        !trace_id.is_empty(),
        "event {name} has no top-level trace_id"
    );
    assert_eq!(
        event["span_fields"]["tenant"].as_str(),
        Some(TASK_ID),
        "event {name} must inherit the run span's correlation field"
    );
    event
}

#[test]
fn sandbox_decisions_are_traced_with_non_empty_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let trace_path = tmp.path().join("trace.jsonl");
    // A deterministic nonzero exit is all the test needs; use the host shell
    // so the same contract runs on Linux/macOS and Windows runners.
    let (shell, shell_args): (&str, &[&str]) = if cfg!(windows) {
        ("cmd", &["/C", "exit 7"])
    } else {
        ("/bin/sh", &["-c", "exit 7"])
    };
    let status: ExitStatus = Command::new(shell)
        .args(shell_args)
        .status()
        .expect("shell exit status");

    {
        let layer = FileTraceLayer::new(trace_path.clone()).expect("file trace layer");
        let subscriber = tracing_subscriber::Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("sandbox_trace_unit", tenant = TASK_ID);
            let _guard = span.enter();
            trace_sandbox_applied(2, TASK_ID, "/usr/bin/obscura");
            trace_sandbox_skipped(2, TASK_ID, "arapuca binary not found", "/usr/bin/obscura");
            trace_sandbox_denials("seccomp: blocked syscall 101 (ptrace)\n", 2, TASK_ID);
            trace_browser_exit(Some(&status), 2, TASK_ID);
        });
    }

    let events = read_events(&trace_path);
    let applied = assert_sandbox_event(&events, "sandbox_applied", &["layer", "task_id", "binary"]);
    assert_eq!(applied["fields"]["binary"], "/usr/bin/obscura");

    let skipped = assert_sandbox_event(
        &events,
        "sandbox_skipped",
        &["layer", "task_id", "reason", "binary"],
    );
    assert_eq!(skipped["fields"]["reason"], "arapuca binary not found");

    let denial = assert_sandbox_event(
        &events,
        "sandbox_denial",
        &["errno", "syscall", "syscall_nr", "layer", "task_id"],
    );
    assert_eq!(denial["fields"]["errno"], "EPERM");
    assert_eq!(denial["fields"]["syscall"], "ptrace");

    let exit = assert_sandbox_event(
        &events,
        "browser_exit",
        &["exit_status", "success", "layer", "task_id"],
    );
    assert_eq!(exit["fields"]["success"], false);
}
