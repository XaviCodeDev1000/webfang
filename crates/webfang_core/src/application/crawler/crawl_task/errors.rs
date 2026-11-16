//! Crawl-task error bookkeeping.
//!
//! The completion counters every spawned page task feeds: the error total, the
//! per-category breakdown, and — since #2823 — the SEPARATE resolution counter
//! that lets a batch tell "the site refused us" from "the resolver is down".
//!
//! Extracted from `crawl_task.rs` to keep that grandfathered file from growing:
//! a pure move, no behaviour change.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use tracing::{debug, warn};

use crate::domain::{CrawlError, CrawlErrorCategory};

/// Handle result from a completed crawl task
pub(crate) fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
    resolution_failures: &Arc<AtomicUsize>,
) {
    match result {
        Ok(Ok(())) => {
            // Task completed successfully
        },
        Ok(Err(e)) => handle_task_error(e, error_count, error_breakdown, resolution_failures),
        Err(e) => handle_join_error(e, error_count, error_breakdown),
    }
}

/// Bookkeeping for a task that returned an operational error.
///
/// Shutdown cancellation is a control signal, not a failure (#509), so it
/// must not inflate the crawl error counters.
fn handle_task_error(
    e: CrawlError,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
    resolution_failures: &Arc<AtomicUsize>,
) {
    if matches!(e, CrawlError::Cancelled) {
        debug!("Task cancelled by engine shutdown");
        return;
    }
    // #2823: a name-resolution failure is counted APART from the category
    // breakdown, which folds every transport cause into `Network`. The batch
    // reads this counter to tell "the site refused us" from "the resolver is
    // down".
    if crate::domain::error::mapping::is_resolution_failure(&e) {
        resolution_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let category = CrawlErrorCategory::from(&e);
    warn!(error = %e, category = ?category, "crawl task failed");
    error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    error_breakdown[category.index()].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Bookkeeping for a task that failed to join (abort or panic).
fn handle_join_error(
    e: tokio::task::JoinError,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
) {
    if e.is_cancelled() {
        // JoinSet abort (shutdown drain timeout, #509) — logged once at
        // the drain site; not counted as a panic.
        debug!("Task aborted during shutdown drain");
        return;
    }
    warn!(error = %e, "crawl task panicked");
    error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    error_breakdown[CrawlErrorCategory::Panic.index()]
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}
