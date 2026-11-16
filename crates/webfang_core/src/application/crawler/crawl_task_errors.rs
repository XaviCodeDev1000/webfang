//! Error-path bookkeeping for finished crawl tasks (#2827).
//!
//! `handle_crawl_result` dispatches a completed task's outcome to the error
//! counters. Re-exported by `crawl_task` so the
//! `crawler::crawl_task::handle_crawl_result` path is unchanged; this module
//! is where the per-cause breakdown of #2824 lands.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use tracing::{debug, warn};

use crate::domain::{CrawlError, CrawlErrorCategory};

/// Handle result from a completed crawl task
pub(crate) fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
) {
    match result {
        Ok(Ok(())) => {
            // Task completed successfully
        },
        Ok(Err(e)) => handle_task_error(e, error_count, error_breakdown),
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
) {
    if matches!(e, CrawlError::Cancelled) {
        debug!("Task cancelled by engine shutdown");
        return;
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
