use super::tests_support::*;
use super::*;

use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// --- handle_crawl_result unit tests (PR #476) ---

fn counters() -> (Arc<AtomicUsize>, Arc<[AtomicUsize; 8]>) {
    (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(std::array::from_fn(|_| AtomicUsize::new(0))),
    )
}

#[test]
fn success_leaves_counters_unchanged() {
    let (error_count, breakdown) = counters();
    handle_crawl_result(Ok(Ok(())), &error_count, &breakdown);
    assert_eq!(error_count.load(Ordering::SeqCst), 0);
    assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
}

#[test]
fn crawl_error_increments_count_and_own_category() {
    let (error_count, breakdown) = counters();
    handle_crawl_result(Ok(Err(CrawlError::Timeout)), &error_count, &breakdown);
    assert_eq!(error_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        breakdown[CrawlErrorCategory::Timeout.index()].load(Ordering::SeqCst),
        1
    );
    for cat in CrawlErrorCategory::ALL {
        if cat != CrawlErrorCategory::Timeout {
            assert_eq!(breakdown[cat.index()].load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn http_403_increments_waf_category() {
    // Issue #603: a 403 must be counted as errors_waf, not network/http.
    let (error_count, breakdown) = counters();
    handle_crawl_result(
        Ok(Err(CrawlError::Http {
            status: 403,
            url: "https://example.com".into(),
        })),
        &error_count,
        &breakdown,
    );
    assert_eq!(error_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        breakdown[CrawlErrorCategory::Waf.index()].load(Ordering::SeqCst),
        1
    );
    for cat in CrawlErrorCategory::ALL {
        if cat != CrawlErrorCategory::Waf {
            assert_eq!(
                breakdown[cat.index()].load(Ordering::SeqCst),
                0,
                "category {cat} must remain zero for a 403"
            );
        }
    }
}

#[test]
fn crawl_error_category_is_derived_from_error() {
    let (error_count, breakdown) = counters();
    handle_crawl_result(
        Ok(Err(CrawlError::Parse("bad html".into()))),
        &error_count,
        &breakdown,
    );
    assert_eq!(error_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        breakdown[CrawlErrorCategory::Extraction.index()].load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        breakdown[CrawlErrorCategory::Timeout.index()].load(Ordering::SeqCst),
        0,
        "category must follow the error, not a fixed index"
    );
}

#[tokio::test]
async fn join_error_increments_panic_category() {
    let (error_count, breakdown) = counters();
    let join_err = tokio::spawn(async { panic!("boom") }).await.unwrap_err();
    handle_crawl_result(Err(join_err), &error_count, &breakdown);
    assert_eq!(error_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        breakdown[CrawlErrorCategory::Panic.index()].load(Ordering::SeqCst),
        1
    );
    for cat in CrawlErrorCategory::ALL {
        if cat != CrawlErrorCategory::Panic {
            assert_eq!(breakdown[cat.index()].load(Ordering::SeqCst), 0);
        }
    }
}

// ========================================================================
// Cancellation tests (#509)
// ========================================================================

#[test]
fn cancelled_result_is_not_counted_as_error() {
    let (error_count, breakdown) = counters();
    handle_crawl_result(Ok(Err(CrawlError::Cancelled)), &error_count, &breakdown);
    assert_eq!(
        error_count.load(Ordering::SeqCst),
        0,
        "shutdown cancellation must not inflate the error count"
    );
    assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
}

#[tokio::test]
async fn aborted_join_result_is_not_counted_as_panic() {
    let (error_count, breakdown) = counters();
    let handle = tokio::spawn(std::future::pending::<Result<(), CrawlError>>());
    handle.abort();
    let join_err = handle.await.unwrap_err();
    assert!(join_err.is_cancelled());

    handle_crawl_result(Err(join_err), &error_count, &breakdown);
    assert_eq!(
        error_count.load(Ordering::SeqCst),
        0,
        "drain-time aborts are a control signal, not panics"
    );
    assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
}

#[tokio::test]
async fn run_crawl_task_cancelled_during_rate_limit_wait() {
    // 60s refill with burst 1: consuming the burst forces the task into
    // the rate-limit wait, where only cancellation can free it.
    let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(60_000, 1))
        .expect("valid rate limiter config");
    limiter.until_ready().await;

    let cancel = CancellationToken::new();
    let (collector, sent) = mock_collector();
    let ctx = TestCtxBuilder::new(collector)
        .rate_limiter(limiter)
        .cancel_token(cancel.clone())
        .build();

    cancel.cancel();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        run_crawl_task(ctx, test_url("https://example.com/page", 0)),
    )
    .await;

    assert!(
        matches!(result, Ok(Err(CrawlError::Cancelled))),
        "worker parked on the rate limiter must abort within the bound"
    );
    assert!(
        sent.lock().map(|s| s.is_empty()).unwrap_or(false),
        "cancelled task must not emit results"
    );
}
