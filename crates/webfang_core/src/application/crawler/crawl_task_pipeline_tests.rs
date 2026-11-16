use super::tests_support::*;
use super::*;

use crate::domain::downloader_port::Cookie;
use crate::infrastructure::observability::test_log_capture::LogBuffer;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use url::Url;

// ── Tests: pipeline ──

#[tokio::test]
async fn test_pipeline_skip_returns_early() {
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .pipeline(Arc::new(MockPipeline {
            behavior: PipelineBehavior::Filtered,
        }))
        .build();

    let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
    assert!(result.is_ok());
    let sent = sent.lock().expect("lock not poisoned");
    assert!(sent.is_empty());
}

#[tokio::test]
async fn test_pipeline_reject_returns_early() {
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .pipeline(Arc::new(MockPipeline {
            behavior: PipelineBehavior::Rejected(
                crate::domain::pipeline_item::RejectReason::EmptyContent,
            ),
        }))
        .build();

    let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
    assert!(result.is_ok());
    let sent = sent.lock().expect("lock not poisoned");
    assert!(sent.is_empty());
}

#[tokio::test]
async fn test_pipeline_failed_logs_class_as_structured_field() {
    let (buf, _guard) = LogBuffer::capture();
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .pipeline(Arc::new(MockPipeline {
            behavior: PipelineBehavior::Failed(crate::domain::error::ErrorClass::TransientBackoff),
        }))
        .build();

    let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
    assert!(result.is_ok());
    assert!(sent.lock().expect("lock not poisoned").is_empty());

    let out = buf.text();
    assert!(
        out.contains("class=TransientBackoff"),
        "ErrorClass must travel as a structured field, got: {out}"
    );
    assert!(out.contains("ERROR"), "should be ERROR level: {out}");
    assert!(
        out.contains("stage=pipeline"),
        "stage field must identify the pipeline: {out}"
    );
}

#[tokio::test]
async fn test_no_pipeline_skips_processing() {
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector).build();

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    let sent = sent.lock().expect("lock not poisoned");
    assert_eq!(sent.len(), 1);
}

// ── Tests: link extraction and depth ──

#[tokio::test]
async fn test_depth_below_max_extracts_links() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .max_depth(3)
        .link_extractor(Arc::new(MockLinkExtractor {
            behavior: ExtractBehavior::Links(vec!["https://example.com/page2".to_string()]),
        }))
        .build();
    let queue = Arc::clone(&ctx.queue);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    assert!(!queue.is_empty().await);
}

#[tokio::test]
async fn test_depth_at_max_skips_extraction() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .max_depth(1)
        .link_extractor(Arc::new(MockLinkExtractor {
            behavior: ExtractBehavior::Links(vec!["https://example.com/page2".to_string()]),
        }))
        .build();
    let queue = Arc::clone(&ctx.queue);

    run_crawl_task(ctx, test_url("https://example.com/", 1))
        .await
        .expect("crawl should succeed");
    assert!(queue.is_empty().await);
}

// ── Tests: robots.txt ──

#[tokio::test]
async fn test_robots_denied_link_not_queued() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .robots(Arc::new(MockRobotsChecker {
            allowed: false,
            call_count: Arc::new(AtomicUsize::new(0)),
        }))
        .link_extractor(Arc::new(MockLinkExtractor {
            behavior: ExtractBehavior::Links(vec!["https://example.com/blocked".to_string()]),
        }))
        .build();
    let queue = Arc::clone(&ctx.queue);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    assert!(queue.is_empty().await);
}

#[tokio::test]
async fn test_robots_allowed_link_queued() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .robots(Arc::new(MockRobotsChecker {
            allowed: true,
            call_count: Arc::new(AtomicUsize::new(0)),
        }))
        .link_extractor(Arc::new(MockLinkExtractor {
            behavior: ExtractBehavior::Links(vec!["https://example.com/allowed".to_string()]),
        }))
        .build();
    let queue = Arc::clone(&ctx.queue);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    assert!(!queue.is_empty().await);
}

#[tokio::test]
async fn test_ignore_robots_bypasses_check() {
    let (collector, _) = mock_collector();
    let call_count = Arc::new(AtomicUsize::new(0));
    let ctx = TestCtxBuilder::new(collector)
        .ignore_robots(true)
        .robots(Arc::new(MockRobotsChecker {
            allowed: false,
            call_count: Arc::clone(&call_count),
        }))
        .link_extractor(Arc::new(MockLinkExtractor {
            behavior: ExtractBehavior::Links(vec!["https://example.com/page".to_string()]),
        }))
        .build();
    let queue = Arc::clone(&ctx.queue);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    assert!(!queue.is_empty().await);
    assert_eq!(call_count.load(Ordering::SeqCst), 0);
}

// ── Tests: deduplication ──

#[tokio::test]
async fn test_duplicate_link_not_queued() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .link_extractor(Arc::new(MockLinkExtractor {
            behavior: ExtractBehavior::Links(vec!["https://example.com/dup".to_string()]),
        }))
        .build();
    let queue = Arc::clone(&ctx.queue);
    // Pre-enqueue the URL so the queue's dedup set (`seen`) already contains
    // it. Enqueue-time dedup lives in `push_prioritized`, not in the `visited`
    // set (which is reserved for dispatch-time dedup — see #479).
    let seed = test_url("https://example.com/", 0);
    let dup = DiscoveredUrl::html(
        Url::parse("https://example.com/dup").expect("valid test URL"),
        1,
        seed.url.clone(),
    );
    assert!(queue.push_prioritized(dup, UrlSource::Link).await);

    run_crawl_task(ctx, seed)
        .await
        .expect("crawl should succeed");
    // Only the pre-seeded entry remains — the task's duplicate was rejected.
    assert_eq!(queue.len().await, 1);
}

// ── Tests: external links ──

#[tokio::test]
async fn test_external_link_not_queued() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .link_extractor(Arc::new(MockLinkExtractor {
            behavior: ExtractBehavior::Links(vec!["https://other.com/page".to_string()]),
        }))
        .build();
    let queue = Arc::clone(&ctx.queue);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    assert!(queue.is_empty().await);
}

// ── Tests: link extraction error ──

#[tokio::test]
async fn test_link_extraction_error_increments_error_count() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .link_extractor(Arc::new(MockLinkExtractor {
            behavior: ExtractBehavior::Error("parse failed".to_string()),
        }))
        .build();
    let error_count = Arc::clone(&ctx.error_count);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed despite extraction error");
    assert_eq!(error_count.load(Ordering::SeqCst), 1);
}

// ── Tests: cookies ──

#[tokio::test]
async fn test_cookies_ingested_into_bridge() {
    let (collector, _) = mock_collector();
    let cookie = Cookie {
        name: "session".to_string(),
        value: "abc123".to_string(),
        domain: "example.com".to_string(),
        path: "/".to_string(),
        http_only: true,
        secure: true,
    };
    let ctx = TestCtxBuilder::new(collector)
        .fetcher(Arc::new(MockPageFetcher {
            behavior: FetchBehavior::Success("<html></html>".to_string(), vec![cookie]),
            final_url: None,
        }))
        .build();
    let bridge = Arc::clone(&ctx.cookie_bridge);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    let b = bridge.read().await;
    assert_eq!(b.len(), 1);
}

#[tokio::test]
async fn test_no_cookies_skips_bridge() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .fetcher(Arc::new(MockPageFetcher {
            behavior: FetchBehavior::Success("<html></html>".to_string(), Vec::new()),
            final_url: None,
        }))
        .build();
    let bridge = Arc::clone(&ctx.cookie_bridge);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    let b = bridge.read().await;
    assert_eq!(b.len(), 0);
}
