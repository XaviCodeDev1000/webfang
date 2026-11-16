//! The task-completion bookkeeping — the error counters and the cause slot.
//!
//! #2824: the failures are COUNTED, not kept, so the run remembers the FIRST
//! classified transport cause: it is the one cause the application boundary
//! needs to name the failure it reports.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use super::*;
use crate::domain::error::cause::NetworkCause;

fn counters() -> (
    Arc<AtomicUsize>,
    Arc<[AtomicUsize; 8]>,
    Arc<OnceLock<NetworkCause>>,
) {
    (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(std::array::from_fn(|_| AtomicUsize::new(0))),
        Arc::new(OnceLock::new()),
    )
}

#[test]
fn success_leaves_counters_unchanged() {
    let (error_count, breakdown, transport_cause) = counters();
    handle_crawl_result(Ok(Ok(())), &error_count, &breakdown, &transport_cause);
    assert_eq!(error_count.load(Ordering::SeqCst), 0);
    assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
}

#[test]
fn crawl_error_increments_count_and_own_category() {
    let (error_count, breakdown, transport_cause) = counters();
    handle_crawl_result(
        Ok(Err(CrawlError::Timeout)),
        &error_count,
        &breakdown,
        &transport_cause,
    );
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
    let (error_count, breakdown, transport_cause) = counters();
    handle_crawl_result(
        Ok(Err(CrawlError::Http {
            status: 403,
            url: "https://example.com".into(),
        })),
        &error_count,
        &breakdown,
        &transport_cause,
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
    let (error_count, breakdown, transport_cause) = counters();
    handle_crawl_result(
        Ok(Err(CrawlError::Parse("bad html".into()))),
        &error_count,
        &breakdown,
        &transport_cause,
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

/// #2824: the first classified transport cause of the run is remembered.
///
/// The failures are COUNTED, not kept: without this slot the application
/// boundary receives a count and no diagnosis at all.
#[test]
fn a_classified_transport_failure_is_remembered() {
    use crate::domain::error::cause::{ClassifiedNetworkError, NetworkCause};

    let (error_count, breakdown, transport_cause) = counters();
    let classified = CrawlError::Download(Box::new(
        crate::domain::downloader_port::DownloadError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            crate::domain::error::cause::CauseCarrier::new(ClassifiedNetworkError::new(
                NetworkCause::ConnectRefused,
                Box::new(std::io::Error::other("connection refused")),
            )),
        )),
    ));

    handle_crawl_result(
        Ok(Err(classified)),
        &error_count,
        &breakdown,
        &transport_cause,
    );

    assert_eq!(
        transport_cause.get().copied(),
        Some(NetworkCause::ConnectRefused),
        "the classified cause must survive the bookkeeping"
    );
}

/// #2824: an error with no classified cause records none — the slot never
/// guesses.
#[test]
fn an_unclassified_failure_remembers_no_cause() {
    let (error_count, breakdown, transport_cause) = counters();

    handle_crawl_result(
        Ok(Err(CrawlError::Parse("bad html".into()))),
        &error_count,
        &breakdown,
        &transport_cause,
    );

    assert_eq!(
        transport_cause.get().copied(),
        None,
        "an unclassified failure must not invent a cause"
    );
}

#[tokio::test]
async fn join_error_increments_panic_category() {
    let (error_count, breakdown, transport_cause) = counters();
    let join_err = tokio::spawn(async { panic!("boom") }).await.unwrap_err();
    handle_crawl_result(Err(join_err), &error_count, &breakdown, &transport_cause);
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
    let (error_count, breakdown, transport_cause) = counters();
    handle_crawl_result(
        Ok(Err(CrawlError::Cancelled)),
        &error_count,
        &breakdown,
        &transport_cause,
    );
    assert_eq!(
        error_count.load(Ordering::SeqCst),
        0,
        "shutdown cancellation must not inflate the error count"
    );
    assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
}

#[tokio::test]
async fn aborted_join_result_is_not_counted_as_panic() {
    let (error_count, breakdown, transport_cause) = counters();
    let handle = tokio::spawn(std::future::pending::<Result<(), CrawlError>>());
    handle.abort();
    let join_err = handle.await.unwrap_err();
    assert!(join_err.is_cancelled());

    handle_crawl_result(Err(join_err), &error_count, &breakdown, &transport_cause);
    assert_eq!(
        error_count.load(Ordering::SeqCst),
        0,
        "drain-time aborts are a control signal, not panics"
    );
    assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
}
