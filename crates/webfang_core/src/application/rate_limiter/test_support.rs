//! Shared test helpers for the rate limiter's own tests.
//!
//! The trace capture lives in its own module so every test module of the
//! limiter shares ONE implementation: the production [`FileTraceLayer`] is
//! the assertion surface, not a mock — what a test proves is that the event
//! survives the same path `--trace-file` uses.
//!
//! [`FileTraceLayer`]: crate::infrastructure::observability::FileTraceLayer

use tracing_subscriber::layer::SubscriberExt;

/// `tracing` caches the per-callsite `Interest` process-wide: the first
/// thread to reach the `rate limit wait` callsite WITHOUT a subscriber
/// caches `Interest::never()` forever, which would silently turn these
/// assertions into a test of nothing. Installing a global sink subscriber
/// once makes every callsite register as always-interested; the per-test
/// `FileTraceLayer` below still collects what it needs.
static GLOBAL_SUBSCRIBER: std::sync::Once = std::sync::Once::new();

pub(super) fn ensure_global_subscriber() {
    GLOBAL_SUBSCRIBER.call_once(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .finish(),
        );
    });
}

/// Run `body` under a real `FileTraceLayer` and return the JSONL records.
///
/// The body drives its own runtime (governor needs a Tokio timer), so the
/// whole future stays inside the `with_default` scope and every event it
/// emits is captured.
pub(super) fn capture_trace<F, Fut>(body: F) -> Vec<serde_json::Value>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    ensure_global_subscriber();
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
    std::fs::read_to_string(&path)
        .expect("trace file")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
        .collect()
}

/// The pacing events of a captured trace.
pub(super) fn wait_events(records: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    records
        .iter()
        .filter(|r| r["message"] == "rate limit wait")
        .collect()
}

/// The WARN events of a captured trace.
pub(super) fn warn_events(records: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    records
        .iter()
        .filter(|r| r["level"].as_str() == Some("WARN"))
        .collect()
}
