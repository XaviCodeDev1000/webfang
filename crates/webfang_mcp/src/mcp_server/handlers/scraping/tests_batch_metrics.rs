use super::serialization::record_batch_metrics_by_domain;
use super::tests_common::*;
use super::*;
// --- record_batch_metrics_by_domain (#696) ---

/// Build a minimal successful `ScrapedContent` for a given URL.
fn scraped(url: &str) -> webfang_core::domain::entities::content::ScrapedContent {
    webfang_core::domain::entities::content::ScrapedContent {
        title: "t".into(),
        content: "c".into(),
        url: webfang_core::domain::ValidUrl::try_from_url(url::Url::parse(url).expect("valid url"))
            .expect("test fixture URL must be a plain http(s) URL"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
        correlation_id: None,
        quality_hint: None,
    }
}

/// Build a minimal `ScrapeFailed` for a given URL.
fn failed(url: &str) -> webfang_core::application::scraper_service::ScrapeFailed {
    webfang_core::application::scraper_service::ScrapeFailed {
        url: url.to_string(),
        error: "boom".into(),
        category: webfang_core::application::scraper_service::ScrapeErrorCategory::PermanentFatal,
    }
}

/// Record a batch outcome against the handler's metrics (#696 tests).
fn record(
    handler: &McpHandler,
    outcome: webfang_core::application::scraper_service::ScrapeBatchOutcome,
) {
    record_batch_metrics_by_domain(
        &handler.state,
        &outcome,
        Instant::now(),
        &webfang_core::domain::CorrelationId::new(),
    );
}

#[tokio::test]
async fn batch_metrics_multi_domain_attributed_per_host() {
    // OBS-MCP-BATCH-001: a batch spanning two hosts must record one
    // event per REAL host, not everything under `urls.first()`.
    let (handler, _tmp) = test_handler().await;
    let outcome = webfang_core::application::scraper_service::ScrapeBatchOutcome {
        results: vec![
            scraped("https://example.com/a"),
            scraped("https://books.toscrape.com/b"),
        ],
        failed: vec![],
    };
    record(&handler, outcome);

    let snap = handler.state.metrics_snapshot();
    assert_eq!(snap.total_events, 2, "one event per domain");
    assert_eq!(snap.domains.len(), 2, "two distinct domains recorded");
    assert_eq!(snap.domains["example.com"].pages, 1, "example.com pages");
    assert_eq!(
        snap.domains["books.toscrape.com"].pages, 1,
        "books.toscrape.com pages"
    );
}

#[tokio::test]
async fn batch_metrics_single_domain_regression_guard() {
    let (handler, _tmp) = test_handler().await;
    let outcome = webfang_core::application::scraper_service::ScrapeBatchOutcome {
        results: vec![
            scraped("https://example.com/a"),
            scraped("https://example.com/b"),
        ],
        failed: vec![],
    };
    record(&handler, outcome);

    let snap = handler.state.metrics_snapshot();
    assert_eq!(snap.total_events, 1, "single domain = single event");
    assert_eq!(snap.domains["example.com"].pages, 2, "both pages counted");
    assert_eq!(snap.success_count, 1, "all-success batch is Success");
}

#[tokio::test]
async fn batch_metrics_mixed_outcomes_per_domain() {
    // A domain with both a success and a failure is Partial; a domain
    // with only failures is Error.
    let (handler, _tmp) = test_handler().await;
    let outcome = webfang_core::application::scraper_service::ScrapeBatchOutcome {
        results: vec![scraped("https://example.com/ok")],
        failed: vec![
            failed("https://example.com/bad"),
            failed("https://broken.test/x"),
        ],
    };
    record(&handler, outcome);

    let snap = handler.state.metrics_snapshot();
    assert_eq!(snap.domains.len(), 2);
    assert_eq!(snap.partial_count, 1, "mixed domain is Partial");
    assert_eq!(snap.error_count, 1, "all-failed domain is Error");
    assert_eq!(snap.domains["example.com"].pages, 2, "ok + bad counted");
    assert_eq!(snap.domains["broken.test"].pages, 1);
}
