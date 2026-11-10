use super::*;
use tracing_subscriber::layer::SubscriberExt;
use url::Url;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

// ——— #1610 crawl run outcome + duration contract (OBS-H7) ———

/// #1610 (OBS-H7): the crawl entry span must say how the run ENDED, and
/// the `crawl completed` event must carry a millisecond duration.
///
/// Before the outcome contract the span closed with configuration only,
/// and `duration_secs` rounded every sub-second run to 0 — the crawl cost
/// was recoverable only from the `span_close` record, which no summary
/// query can aggregate. The crawl runs against a wiremock seed (the
/// proven shape of the sibling entry tests) under a real FileTraceLayer,
/// so both records are read back from the parsed JSONL.
#[test]
fn crawl_site_with_options_span_records_outcome_and_summary_duration_ms() {
    // #1615 (F11 / G-3 / G-4 / G-5): this path applies the literal-IP
    // entry guard, which refuses wiremock's 127.0.0.1.
    let _entry_off = webfang_test_utils::EnvGuard::entry_guard_off();

    let dir = tempfile::tempdir().expect("temp dir");
    let trace_path = dir.path().join("trace.jsonl");
    let layer = crate::infrastructure::observability::FileTraceLayer::new(trace_path.clone())
        .expect("trace layer");
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
    // Multi-threaded, like the sibling entry tests: the crawl spawns
    // workers. Only the entry span and the summary event are asserted —
    // both are produced on THIS thread, where the layer is installed.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");

    tracing::dispatcher::with_default(&dispatch, || {
        runtime.block_on(async {
            let server = MockServer::start().await;
            let port = server.address().port();
            Mock::given(path("/"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"),
                )
                .mount(&server)
                .await;

            let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
            let config = CrawlerConfig::builder(seed)
                .max_depth(0)
                .max_pages(10)
                .ignore_robots(true)
                .build();
            let result = crawl_site_with_options(
                config,
                EngineOptions {
                    ignore_robots: true,
                    ..Default::default()
                },
                &CorrelationId::new(),
            )
            .await
            .expect("crawl must succeed");
            assert_eq!(result.total_pages, 1, "the seed page is crawled");
        });
    });
    drop(runtime);
    // Flush before reading: the layer buffers and only drains on drop.
    drop(dispatch);

    let records: Vec<serde_json::Value> = std::fs::read_to_string(&trace_path)
        .expect("trace file")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
        .collect();

    let closes: Vec<_> = records
        .iter()
        .filter(|r| r["record"] == "span_close" && r["span"] == "crawl_site_with_options")
        .collect();
    assert_eq!(closes.len(), 1, "exactly one crawl entry span closes");
    let span_fields = closes[0]["span_fields"].as_object().expect("span_fields");
    assert_eq!(
        span_fields.get("outcome").and_then(|v| v.as_str()),
        Some("ok"),
        "a completed crawl must close its span with a verdict"
    );
    assert!(
        span_fields.contains_key("correlation_id"),
        "the run-root identity must survive the late outcome record"
    );

    let summaries: Vec<_> = records
        .iter()
        .filter(|r| r["message"] == "crawl completed")
        .collect();
    assert_eq!(summaries.len(), 1, "exactly one crawl summary");
    let summary_fields = summaries[0]["fields"].as_object().expect("summary fields");
    let duration_ms = summary_fields
        .get("duration_ms")
        .and_then(serde_json::Value::as_u64)
        .expect("the summary must report milliseconds, not whole seconds only");
    assert!(
        summary_fields.contains_key("duration_secs"),
        "the historical field must survive — other tooling reads it"
    );
    let span_ms = closes[0]["span_duration_ms"]
        .as_u64()
        .expect("the span reports its own lifetime");
    assert!(
        duration_ms <= span_ms,
        "the summary is emitted inside the span, so its elapsed time ({duration_ms}ms) cannot exceed the span lifetime ({span_ms}ms)"
    );
}
