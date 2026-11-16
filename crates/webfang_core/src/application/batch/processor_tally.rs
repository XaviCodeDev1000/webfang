//! Resolver-abort tally for a batch run (#2823).
//!
//! Extracted from `processor.rs` to keep that grandfathered file under the
//! 500-line limit. Pure move: the counters and the abort decision are unchanged.

use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::application::batch::BatchProgress;
use crate::domain::{CorrelationId, CrawlError};
use crate::error::ScraperError;

/// How many CONSECUTIVE name-resolution failures abort a batch run (#2823).
///
/// Below this, dispatch is already amortized and aborting would punish an
/// isolated failure. At this, the resolver is plainly down: every remaining
/// URL would pay the same connect timeout, which is exactly the "50 URLs
/// frenadas como si fuera un WAF" the issue describes.
const RESOLUTION_ABORT_THRESHOLD: usize = 5;

/// Run-level tally of a batch: the two failure populations and the streak.
///
/// Kept as one value so the abort decision and the reported counters can
/// never disagree — they are read from the same place (#2823).
#[derive(Debug, Default)]
pub(super) struct BatchTally {
    /// Failed URLs attributed to name resolution.
    pub(super) resolution_failures: usize,
    /// Failed URLs the site refused (everything else).
    pub(super) block_failures: usize,
    /// Unbroken run of resolution failures ending at the last finished URL.
    consecutive_resolutions: usize,
}

impl BatchTally {
    /// Whether the streak has reached the abort threshold.
    fn resolver_is_down(&self) -> bool {
        self.consecutive_resolutions >= RESOLUTION_ABORT_THRESHOLD
    }
}

/// Record one finished URL and report whether the run must stop dispatching.
///
/// The outcome is classified by CAUSE, never by message text: a resolution
/// failure is OURS, and the streak it feeds is what tells a broken resolver
/// apart from a site that is refusing us (#2823).
///
/// Returns `true` when the resolver streak reached
/// [`RESOLUTION_ABORT_THRESHOLD`] and no further URL should be dispatched.
pub(super) fn record_batch_outcome(
    tally: &mut BatchTally,
    progress: &BatchProgress,
    errors: &mut Vec<(String, ScraperError)>,
    joined: Result<(String, Result<(), CrawlError>), tokio::task::JoinError>,
    run_root: &CorrelationId,
) -> bool {
    match joined {
        Ok((url, Ok(_))) => {
            progress.complete_one();
            // A URL that succeeded proves the resolver answered for it.
            tally.consecutive_resolutions = 0;
            info!("Completed crawl for {url}");
            false
        },
        Ok((url, Err(e))) => {
            progress.fail_one();
            // Preserve the full variant through the CrawlError ->
            // ScraperError conversion (#537): severity routing needs
            // classify(), which a flattened string cannot provide.
            let scraper_err = ScraperError::from(e);
            if crate::domain::error::mapping::is_resolution_failure(&scraper_err) {
                tally.resolution_failures += 1;
                tally.consecutive_resolutions += 1;
            } else {
                tally.block_failures += 1;
                tally.consecutive_resolutions = 0;
            }
            crate::infrastructure::observability::log_scrape_error_with_cause(
                &scraper_err,
                &url,
                "batch_crawl",
                Some(run_root),
                "batch URL crawl failed",
            );
            errors.push((url, scraper_err));
            tally.resolver_is_down()
        },
        Err(e) => {
            progress.fail_one();
            // A panic is neither population: it says nothing about the
            // resolver, so it breaks the streak without joining either
            // counter.
            tally.consecutive_resolutions = 0;
            error!(error = %e, "batch crawl task panicked");
            errors.push((
                "unknown".to_string(),
                ScraperError::Internal(format!("task-panic: {e}")),
            ));
            false
        },
    }
}

/// The run-level trace for a resolver abort: ONE event, not one per URL.
pub(super) fn log_resolution_abort(job_id: &str, tally: &BatchTally) {
    warn!(
        job_id,
        resolution_failures = tally.resolution_failures,
        block_failures = tally.block_failures,
        threshold = RESOLUTION_ABORT_THRESHOLD,
        "batch aborted — the resolver is not answering"
    );
}

/// Drain every task that has ALREADY finished, returning whether the
/// resolver streak reached the abort threshold (#2823).
///
/// Non-blocking by construction: `try_join_next` never waits, so the dispatch
/// loop reads the outcomes it needs before spending another permit.
pub(super) fn drain_finished(
    join_set: &mut JoinSet<(String, Result<(), CrawlError>)>,
    tally: &mut BatchTally,
    progress: &BatchProgress,
    errors: &mut Vec<(String, ScraperError)>,
    run_root: &CorrelationId,
) -> bool {
    let mut aborted = false;
    while let Some(joined) = join_set.try_join_next() {
        if record_batch_outcome(tally, progress, errors, joined, run_root) {
            aborted = true;
            break;
        }
    }
    aborted
}
