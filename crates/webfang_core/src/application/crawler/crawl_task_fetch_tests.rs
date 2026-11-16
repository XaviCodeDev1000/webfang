use super::tests_support::*;
use super::*;

use crate::domain::session_port::SessionId;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use url::Url;

// ── Tests: fetch success path ──

#[tokio::test]
async fn test_fetch_success_sends_to_collector() {
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector).build();
    let url = test_url("https://example.com/page1", 0);

    let result = run_crawl_task(ctx, url).await;
    assert!(result.is_ok());
    let sent = sent.lock().expect("lock not poisoned");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url.as_str(), "https://example.com/page1");
}

#[tokio::test]
async fn test_redirect_propagates_final_url_to_collector() {
    // Bug 3 (#651): a redirect /redirect/5 -> /final must be recorded under
    // /final, not the requested /redirect/5, or aliases collapse into
    // duplicate content.
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .fetcher(Arc::new(MockPageFetcher {
            behavior: FetchBehavior::Success("<html></html>".to_string(), Vec::new()),
            final_url: Some(Url::parse("https://example.com/final").expect("valid URL")),
        }))
        .build();
    let url = test_url("https://example.com/redirect/5", 0);

    let result = run_crawl_task(ctx, url).await;
    assert!(
        result.is_ok(),
        "redirect crawl should succeed, got: {:?}",
        result.err()
    );
    let sent = sent.lock().expect("lock not poisoned");
    assert_eq!(
        sent.len(),
        1,
        "exactly one result must reach the collector after a redirect"
    );
    assert_eq!(sent[0].url.as_str(), "https://example.com/final");
}

#[tokio::test]
async fn test_fetch_success_increments_pages_crawled() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector).build();
    let pages = Arc::clone(&ctx.pages_crawled);

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    assert_eq!(pages.load(Ordering::Relaxed), 1);
}

// ── Tests: WAF error handling ──

#[tokio::test]
async fn test_fetch_waf_error_bans_domain() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .fetcher(Arc::new(MockPageFetcher {
            behavior: FetchBehavior::WafError("Cloudflare".to_string()),
            final_url: None,
        }))
        .build();
    let banned = Arc::clone(&ctx.banned_domains);

    let result = run_crawl_task(ctx, test_url("https://protected.com/page", 0)).await;
    assert!(result.is_err());
    let domains = banned.read().expect("lock not poisoned");
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0].domain, "protected.com");
}

#[tokio::test]
async fn test_fetch_waf_error_returns_err() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .fetcher(Arc::new(MockPageFetcher {
            behavior: FetchBehavior::WafError("Cloudflare".to_string()),
            final_url: None,
        }))
        .build();

    let result = run_crawl_task(ctx, test_url("https://protected.com/", 0)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_fetch_generic_error_returns_err_no_ban() {
    let (collector, _) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .fetcher(Arc::new(MockPageFetcher {
            behavior: FetchBehavior::NetworkError("connection refused".to_string()),
            final_url: None,
        }))
        .build();
    let banned = Arc::clone(&ctx.banned_domains);

    let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
    assert!(result.is_err());
    let domains = banned.read().expect("lock not poisoned");
    assert!(domains.is_empty());
}

// ── Tests: session pool ──

#[tokio::test]
async fn test_session_pool_acquire_none_skips_url() {
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .session_pool(Arc::new(MockSessionPool {
            acquire_result: None,
            success_calls: Arc::new(Mutex::new(Vec::new())),
        }))
        .build();

    let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
    assert!(result.is_ok());
    let sent = sent.lock().expect("lock not poisoned");
    assert!(sent.is_empty());
}

#[tokio::test]
async fn test_session_pool_acquire_some_continues() {
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .session_pool(Arc::new(MockSessionPool {
            acquire_result: Some(SessionId(0)),
            success_calls: Arc::new(Mutex::new(Vec::new())),
        }))
        .build();

    let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
    assert!(result.is_ok());
    let sent = sent.lock().expect("lock not poisoned");
    assert_eq!(sent.len(), 1);
}

/// Budget-consumption accounting (issue #1599): a pipeline-rejected
/// page consumes neither budget (no collector send) nor error budget
/// (no error-counter increment) — the run simply continues, so further
/// dispatch stays permitted. Characterization pin: accounting was
/// already correct pre-fix (only dispatch overshot); this guards GREEN
/// against a fix that miscounts rejections.
#[tokio::test]
async fn test_pipeline_rejection_consumes_neither_budget_nor_errors() {
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .pipeline(Arc::new(MockPipeline {
            behavior: PipelineBehavior::Rejected(
                crate::domain::pipeline_item::RejectReason::EmptyContent,
            ),
        }))
        .build();

    let result = run_crawl_task(Arc::clone(&ctx), test_url("https://example.com/", 0)).await;
    assert!(result.is_ok());
    assert!(sent.lock().expect("lock not poisoned").is_empty());
    assert_eq!(
        ctx.error_count.load(Ordering::SeqCst),
        0,
        "rejection must not touch the error counters"
    );
    assert!(
        ctx.error_breakdown
            .iter()
            .all(|counter| counter.load(Ordering::Relaxed) == 0),
        "rejection must not touch the error breakdown"
    );
}

#[tokio::test]
async fn test_session_pool_report_success_called() {
    let (collector, _) = mock_collector();
    let success_calls = Arc::new(Mutex::new(Vec::new()));
    let ctx = TestCtxBuilder::new(collector)
        .session_pool(Arc::new(MockSessionPool {
            acquire_result: Some(SessionId(0)),
            success_calls: Arc::clone(&success_calls),
        }))
        .build();

    run_crawl_task(ctx, test_url("https://example.com/", 0))
        .await
        .expect("crawl should succeed");
    let calls = success_calls.lock().expect("lock not poisoned");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "example.com");
    assert_eq!(calls[0].1, SessionId(0));
}
