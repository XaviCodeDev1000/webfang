//! Crawl-task error bookkeeping.
//!
//! The completion counters every spawned page task feeds: the error total, the
//! per-category breakdown, and — since #2823 — the SEPARATE resolution counter
//! that lets a batch tell "the site refused us" from "the resolver is down".
//!
//! Extracted from `crawl_task.rs` to keep that grandfathered file from growing:
//! a pure move, no behaviour change.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, OnceLock};

use tracing::{debug, warn};

use crate::domain::error::cause::{cause_in_chain, NetworkCause};
use crate::domain::{CrawlError, CrawlErrorCategory};

/// Handle result from a completed crawl task
pub(crate) fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
    resolution_failures: &Arc<AtomicUsize>,
    transport_cause: &Arc<OnceLock<NetworkCause>>,
) {
    match result {
        Ok(Ok(())) => {
            // Task completed successfully
        },
        Ok(Err(e)) => handle_task_error(
            e,
            error_count,
            error_breakdown,
            resolution_failures,
            transport_cause,
        ),
        Err(e) => handle_join_error(e, error_count, error_breakdown),
    }
}

/// Whether a task failure is a PERMANENT verdict for its URL (#3079).
///
/// `CrawlCheckpoint.visited` is marked at DISPATCH, not at completion, so a
/// resume cannot treat it as "done": a page still in memory when the process
/// died was never exported. This predicate is the narrower set that a resume
/// may safely skip — the failures no retry can change:
///
/// - an HTTP 4xx other than 408/429 (408 request timeout and 429 rate limit
///   are paced retries, not verdicts),
/// - the same 4xx carried by a transport `Network` failure,
/// - an invalid URL, a URL refused by the crawl filters, an unsupported
///   content type.
///
/// Everything else (timeouts, 5xx, exhausted retries, WAF challenges, parse
/// and budget stops, cancellation, internal failures) stays re-drivable: a
/// resumed run is a fresh chance, and settling on it would drop pages
/// silently (E13).
pub(crate) fn is_terminal_settled(error: &CrawlError) -> bool {
    fn is_permanent_4xx(status: u16) -> bool {
        (400..500).contains(&status) && status != 408 && status != 429
    }

    match error {
        CrawlError::Http { status, .. } => is_permanent_4xx(*status),
        CrawlError::Network {
            status_code: Some(status),
            ..
        } => is_permanent_4xx(*status),
        // A `Network` failure with no status never reached an answer.
        CrawlError::Network {
            status_code: None, ..
        } => false,
        CrawlError::InvalidUrl(_)
        | CrawlError::UrlExcluded(_)
        | CrawlError::InvalidContentType(_) => true,
        // Exhaustive on purpose: a new `CrawlError` variant has to decide
        // whether a resume may skip it, instead of defaulting to either.
        CrawlError::Parse(_)
        | CrawlError::Io(_)
        | CrawlError::Internal(_)
        | CrawlError::InvalidSession(_)
        | CrawlError::SitemapNotFound(_)
        | CrawlError::Storage(_)
        | CrawlError::Checkpoint(_)
        | CrawlError::SessionPool(_)
        | CrawlError::Discovery(_)
        | CrawlError::Download(_)
        | CrawlError::WafChallenge { .. }
        | CrawlError::RetryExhausted { .. }
        | CrawlError::TransientHttp { .. }
        | CrawlError::RateLimited(_)
        | CrawlError::Timeout { .. }
        | CrawlError::Connection { .. }
        | CrawlError::RequestFailed(_)
        | CrawlError::Transport { .. }
        | CrawlError::ResourceExhausted { .. }
        | CrawlError::SitemapEmpty
        | CrawlError::SitemapDepthExceeded
        | CrawlError::SemaphoreInanition
        | CrawlError::Cancelled
        | CrawlError::PartialRun { .. }
        | CrawlError::MaxDepthExceeded { .. }
        | CrawlError::MaxPagesExceeded { .. } => false,
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
    transport_cause: &Arc<OnceLock<NetworkCause>>,
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
    // #2824: the FIRST classified transport cause of the run. The failures are
    // counted, not kept, and the application boundary needs one cause to name
    // the failure it reports.
    if let Some(cause) = cause_in_chain(&e) {
        let _ = transport_cause.set(cause);
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
