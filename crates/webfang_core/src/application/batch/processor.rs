//! Batch processor — concurrent execution of multiple crawl jobs
//!
//! Uses [`tokio::sync::Semaphore`] for job-level concurrency control.
//! Each URL in the batch is a separate `crawl_site_with_options()` call, with
//! the run-wide [`EngineOptions`] built once by `build_batch_engine_options`
//! (#1369 — the knobless `crawl_site` entry is deprecated) and the job's ONE
//! run-root correlation adopted by every engine call (#1439 — carried via
//! [`BatchProcessor::with_correlation`], or minted once per job otherwise, so
//! a batch run reconstructs from a single trace identity).
//!
//! # Usage
//!
//! ```no_run
//! use webfang_core::application::batch::{BatchJob, BatchProcessor};
//! use webfang_core::domain::CrawlerConfig;
//! use url::Url;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = CrawlerConfig::new(Url::parse("https://example.com")?);
//! let job = BatchJob::new(
//!     "batch-1".to_string(),
//!     vec!["https://example.com".to_string()],
//!     config,
//! );
//!
//! let processor = BatchProcessor::new(3).unwrap();
//! let result = processor.process_batch(job).await?;
//!
//! println!("Processed {} URLs, {} succeeded, {} failed",
//!     result.total, result.succeeded, result.failed);
//! # Ok(())
//! # }
//! ```

use std::num::NonZeroUsize;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn, Instrument};

mod options;

#[cfg(test)]
use options::build_per_url_config;
pub(crate) use options::{
    build_batch_engine_options, build_batch_rate_limiter, process_single_url,
};

/// Errors that can occur during batch processing
#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    /// Batch contains no URLs
    #[error("batch contains no URLs")]
    EmptyBatch,

    /// Concurrency limit must be greater than zero
    #[error("max_concurrent must be > 0")]
    InvalidConcurrency,

    /// Semaphore was closed unexpectedly
    #[error("concurrency semaphore was closed")]
    SemaphoreClosed,

    /// Crawl operation failed
    #[error("crawl failed for {url}: {error}")]
    CrawlFailed {
        /// URL that failed to crawl
        url: String,
        /// The underlying crawl error
        error: CrawlError,
    },
}

use super::{BatchJob, BatchProgress};
use crate::application::crawler::content_sink::CrawlContentSink;
use crate::domain::downloader_factory::DownloaderFactory;
#[cfg(test)]
use crate::domain::JsStrategy;
use crate::domain::{CorrelationId, CrawlError};
use crate::error::ScraperError;

/// Result of processing a batch job
///
/// Not `Clone`: `errors` carries [`ScraperError`], which owns non-cloneable
/// sources (`std::io::Error`, boxed trait objects). Callers that need to
/// forward the errors must move them (#537).
#[derive(Debug)]
pub struct BatchResult {
    /// ID of the batch job
    pub job_id: String,
    /// Total number of URLs processed
    pub total: usize,
    /// Number of successfully processed URLs
    pub succeeded: usize,
    /// Number of failed URLs
    pub failed: usize,
    /// List of (url, error) for failed URLs.
    ///
    /// The [`ScraperError`] variant is preserved (not flattened to a string)
    /// so exit-code routing can aggregate severity via
    /// [`ScraperError::classify`] (#537).
    pub errors: Vec<(String, ScraperError)>,
    /// Failed URLs whose cause was NAME RESOLUTION — our resolver, not the site
    /// refusing us (#2823).
    ///
    /// Reported APART from [`Self::block_failures`]: the category taxonomy
    /// folds both into `Network`, so a run that only ever failed to resolve
    /// used to look exactly like a site that blocked every URL.
    pub resolution_failures: usize,
    /// Failed URLs the SITE refused (WAF challenge, 401/403/429, any other
    /// non-resolution failure) (#2823).
    pub block_failures: usize,
    /// Whether the run stopped dispatching because the resolver was down.
    ///
    /// `true` means some URLs were never attempted; `total` counts the ones
    /// that were.
    pub aborted_on_resolution: bool,
}

/// Batch processor with concurrency control
///
/// Uses [`tokio::sync::Semaphore`] to limit the number of concurrent
/// crawl operations. This prevents resource exhaustion when processing
/// large batches of URLs.
#[derive(Clone)]
pub struct BatchProcessor {
    max_concurrent_jobs: usize,
    semaphore: Arc<Semaphore>,
    /// Optional sink that captures every fetched page body (#631).
    ///
    /// Shared across all concurrent crawls in the batch, so the CLI ends up
    /// with one collection covering every URL in the run.
    content_sink: Option<Arc<dyn CrawlContentSink>>,
    /// Run-root correlation identity (#1439).
    ///
    /// `Some` carries the calling operation's root: every per-URL crawl
    /// engine of the job adopts it, so the batch trace reconstructs from the
    /// ONE identity the CLI/MCP announced. `None` keeps a standalone library
    /// job honest: `process_batch_cancellable` mints exactly one root at
    /// the job boundary — never one per URL (the old #687-class split).
    correlation: Option<CorrelationId>,
    /// Optional fetch-downloader factory (#2823).
    ///
    /// `None` (production) keeps the batch on the static `fetch_url` path, the
    /// historical behavior. `Some` replaces the transport for the whole run, so
    /// a caller can inject a resolver that never answers and exercise the
    /// resolution-abort path deterministically — no real network, no real DNS.
    downloader_factory: Option<Arc<dyn DownloaderFactory>>,
}

impl BatchProcessor {
    /// Create a new batch processor with the given concurrency limit
    ///
    /// # Arguments
    ///
    /// * `max_concurrent` - Maximum number of concurrent crawl operations
    ///
    /// # Errors
    ///
    /// Returns [`BatchError::InvalidConcurrency`] if `max_concurrent` is 0.
    pub fn new(max_concurrent: usize) -> Result<Self, BatchError> {
        NonZeroUsize::new(max_concurrent)
            .map(Self::new_nonzero)
            .ok_or(BatchError::InvalidConcurrency)
    }

    /// Create a batch processor from an already-validated concurrency limit
    ///
    /// A batch `Semaphore` bound of 0 would exhaust every permit before the
    /// first job, so the zero case is excluded by the type itself instead of
    /// a runtime check (#2355).
    #[must_use]
    pub fn new_nonzero(max_concurrent: NonZeroUsize) -> Self {
        Self {
            max_concurrent_jobs: max_concurrent.get(),
            semaphore: Arc::new(Semaphore::new(max_concurrent.get())),
            content_sink: None,
            correlation: None,
            downloader_factory: None,
        }
    }

    /// Capture every fetched page body into `sink` (#631).
    ///
    /// Without a sink the batch crawl discards page content and the CLI has
    /// nothing to export — the root cause of `--batch` writing zero files.
    #[must_use]
    pub fn with_content_sink(mut self, sink: Arc<dyn CrawlContentSink>) -> Self {
        self.content_sink = Some(sink);
        self
    }

    /// Replace the run's fetch transport with `factory` (#2823).
    ///
    /// Production leaves this unset and the batch keeps the static
    /// `fetch_url` path. It exists so the resolution-abort path can be driven
    /// by an injected resolver instead of a real one: a batch pointed at a
    /// dead resolver is otherwise only reproducible with a real network.
    #[must_use]
    pub fn with_downloader_factory(mut self, factory: Arc<dyn DownloaderFactory>) -> Self {
        self.downloader_factory = Some(factory);
        self
    }

    /// Adopt `root` as the run identity of every job this processor runs
    /// (#1439).
    ///
    /// The calling operation (CLI batch run, MCP batch tool) owns ONE
    /// run-root and hands it here, so every per-URL crawl engine shares the
    /// trace identity the operation announced. Unset, a standalone library
    /// job still gets exactly one root — minted once per
    /// [`process_batch_cancellable`](Self::process_batch_cancellable) call,
    /// never per URL.
    #[must_use]
    pub fn with_correlation(mut self, root: CorrelationId) -> Self {
        self.correlation = Some(root);
        self
    }

    /// Get the maximum concurrency limit
    #[must_use]
    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent_jobs
    }

    /// Process a batch job, crawling all URLs concurrently
    ///
    /// Returns a [`BatchResult`] with success/failure counts and error details.
    /// All tasks complete before returning (graceful shutdown).
    ///
    /// # Errors
    ///
    /// Returns an error if the batch job itself is malformed (e.g., empty URLs).
    #[instrument(name = "process_batch", skip(self, job), fields(job_id = %job.id, url_count = job.urls.len()))]
    pub async fn process_batch(&self, job: BatchJob) -> Result<BatchResult, BatchError> {
        self.process_batch_cancellable(job, &CancellationToken::new())
            .await
    }

    /// Process a batch job, stopping early when `cancel` is fired.
    ///
    /// On cancellation no further URL is dispatched, but every already-spawned
    /// task is drained to completion so its captured content still reaches the
    /// sink — an abrupt drop would lose the pages already fetched (#653).
    ///
    /// # Errors
    ///
    /// Same as [`process_batch`](Self::process_batch).
    #[instrument(name = "process_batch_cancellable", skip(self, job, cancel), fields(job_id = %job.id, url_count = job.urls.len()))]
    pub async fn process_batch_cancellable(
        &self,
        mut job: BatchJob,
        cancel: &CancellationToken,
    ) -> Result<BatchResult, BatchError> {
        if job.urls.is_empty() {
            return Err(BatchError::EmptyBatch);
        }

        info!(
            "Starting batch job {} with {} URLs (concurrency: {})",
            job.id,
            job.urls.len(),
            self.max_concurrent_jobs
        );

        job.start();
        let progress = job.progress.clone();
        let job_id = job.id.clone();
        let base_config = job.config.clone();
        // #1369: the options are run-wide — built once from the batch's real
        // sources (base config + shared sink) and cloned per URL task.
        // `EngineOptions: Clone` is cheap (owned scalars + Arc handles).
        // #1428: the token bucket is run-wide too — one shared limiter for
        // the whole run, so `--delay-ms` paces across URLs instead of
        // granting each URL a fresh burst. `None` at `delay_ms == 0`
        // keeps unthrottled runs on the historical per-engine limiters.
        let mut base_options = build_batch_engine_options(
            &base_config,
            self.content_sink.clone(),
            self.downloader_factory.clone(),
        );
        base_options.rate_limiter = build_batch_rate_limiter(&base_config);
        // #1439: exactly ONE run-root per job. The production caller's root
        // arrives through `with_correlation`; a standalone library job mints
        // its single root HERE, at the operation boundary — never per URL
        // (the engine-side per-crawl minting is what split the batch trace).
        // `CorrelationId::default()` IS a fresh mint (`Default` delegates to
        // `new`), so the standalone contract stays explicit.
        let run_root = self.correlation.clone().unwrap_or_default();

        let mut join_set = JoinSet::new();
        let mut errors: Vec<(String, ScraperError)> = Vec::new();
        let mut tally = BatchTally::default();

        let mut cancelled = false;
        let mut aborted_on_resolution = false;
        for url_str in &job.urls {
            if cancel.is_cancelled() {
                cancelled = true;
                warn!(
                    job_id = %job.id,
                    "shutdown requested — no further URLs will be dispatched"
                );
                break;
            }

            // Harvest what ALREADY finished before waiting for the next
            // permit: the outcome of a task that just completed is what can
            // abort the run, and the permit it freed is exactly what let us
            // get here. Without this the loop would dispatch the whole list
            // before reading a single outcome (#2823).
            if drain_finished(&mut join_set, &mut tally, &progress, &mut errors, &run_root) {
                aborted_on_resolution = true;
                break;
            }

            let url = url_str.clone();
            let config = base_config.clone();
            let options = base_options.clone();
            let correlation = run_root.clone();
            let permit = self
                .semaphore
                .clone()
                .acquire_owned()
                .await
                // LCOV_EXCL_LINE defensive: semaphore-closed — acquire_owned fails only when the batch governor is shut down
                .map_err(|_| BatchError::SemaphoreClosed)?;

            // The permit we just got was freed by a completion. Read that
            // outcome BEFORE dispatching another URL, so the abort fires on
            // the result itself instead of one URL later (#2823).
            if drain_finished(&mut join_set, &mut tally, &progress, &mut errors, &run_root) {
                aborted_on_resolution = true;
                break;
            }

            progress.start_one();

            join_set.spawn(
                async move {
                    let _permit = permit; // Hold permit for duration of task
                    let result = process_single_url(&url, config, options, &correlation)
                        .await
                        .map(|_| ());
                    (url, result)
                }
                .in_current_span(),
            );
        }

        // Drain whatever is still in flight. An abort does NOT cancel these:
        // an abrupt drop would lose the pages already fetched (#653).
        while let Some(joined) = join_set.join_next().await {
            if record_batch_outcome(&mut tally, &progress, &mut errors, joined, &run_root) {
                aborted_on_resolution = true;
            }
        }

        if aborted_on_resolution {
            log_resolution_abort(&job.id, &tally);
        }

        let succeeded = progress.completed();
        let failed = progress.failed();
        let total = progress.total();

        if cancelled {
            job.fail("cancelled by shutdown signal".to_string());
        } else if aborted_on_resolution {
            job.fail("aborted: the resolver is not answering".to_string());
        } else {
            job.complete();
        }

        info!(
            "Batch job {} completed: {succeeded}/{total} succeeded, {failed} failed",
            job.id
        );

        Ok(BatchResult {
            job_id,
            total,
            succeeded,
            failed,
            errors,
            resolution_failures: tally.resolution_failures,
            block_failures: tally.block_failures,
            aborted_on_resolution,
        })
    }
}

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
struct BatchTally {
    /// Failed URLs attributed to name resolution.
    resolution_failures: usize,
    /// Failed URLs the site refused (everything else).
    block_failures: usize,
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
fn record_batch_outcome(
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
fn log_resolution_abort(job_id: &str, tally: &BatchTally) {
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
fn drain_finished(
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

#[cfg(test)]
#[path = "processor_resolution_tests.rs"]
mod resolution_tests;

#[cfg(test)]
mod tests {
    /// #1615 (F11 / G-3 / G-4 / G-5): these tests drive real fetches against
    /// wiremock's 127.0.0.1, which the literal-IP entry guard now refuses.
    /// `EnvGuard::entry_guard_off()` disarms the guard for its lifetime — the
    /// named constructor is the repo's rule, so each call site stays one line.
    fn entry_off() -> webfang_test_utils::EnvGuard {
        webfang_test_utils::EnvGuard::entry_guard_off()
    }
    use super::*;
    use crate::application::batch::{BatchJob, BatchJobStatus, BatchProgress};
    use crate::domain::CrawlerConfig;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use url::Url;

    #[test]
    fn test_batch_processor_creation() {
        let processor = BatchProcessor::new(5).unwrap();
        assert_eq!(processor.max_concurrent(), 5);
    }

    #[test]
    fn test_batch_processor_zero_concurrency_returns_error() {
        let result = BatchProcessor::new(0);
        assert!(result.is_err(), "zero concurrency should return Err");
        let err = result.err().unwrap();
        assert!(
            matches!(err, BatchError::InvalidConcurrency),
            "expected InvalidConcurrency, got: {err}"
        );
    }

    #[tokio::test]
    async fn cancelled_batch_dispatches_no_urls() {
        // Regression for #653: a shutdown signal must stop the batch from
        // dispatching further URLs instead of running to completion.
        let processor = BatchProcessor::new(2).unwrap();
        let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let job = BatchJob::new(
            "cancelled".to_string(),
            (0..8).map(|i| format!("https://example.com/{i}")).collect(),
            config,
        );

        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = processor
            .process_batch_cancellable(job, &cancel)
            .await
            .expect("a cancelled batch still reports its (empty) result");

        assert_eq!(result.succeeded, 0, "no URL may be crawled after shutdown");
        assert_eq!(result.failed, 0, "skipped URLs are not failures");
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_process_empty_batch() {
        let processor = BatchProcessor::new(3).unwrap();
        let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let job = BatchJob::new("test-1".to_string(), vec![], config);

        let result = processor.process_batch(job).await;
        assert!(matches!(result, Err(BatchError::EmptyBatch)));
    }

    #[tokio::test]
    async fn test_batch_progress_concurrent_updates() {
        let progress = BatchProgress::new(100);
        let mut join_set = JoinSet::new();

        // Deterministic outcomes: first 50 succeed, last 50 fail
        for i in 0..100 {
            let p = progress.clone();
            join_set.spawn(async move {
                p.start_one();
                tokio::task::yield_now().await;
                if i < 50 {
                    p.complete_one();
                    true
                } else {
                    p.fail_one();
                    false
                }
            });
        }

        let mut successes = 0;
        let mut failures = 0;
        while let Some(result) = join_set.join_next().await {
            if result.unwrap() {
                successes += 1;
            } else {
                failures += 1;
            }
        }

        assert_eq!(successes + failures, 100);
        assert_eq!(progress.completed(), 50);
        assert_eq!(progress.failed(), 50);
        assert!(progress.is_complete());
    }

    #[test]
    fn test_batch_result_display() {
        let result = BatchResult {
            job_id: "test-1".to_string(),
            total: 10,
            succeeded: 8,
            failed: 2,
            errors: vec![
                (
                    "https://example.com/404".to_string(),
                    ScraperError::http(404, "https://example.com/404"),
                ),
                (
                    "https://example.com/timeout".to_string(),
                    ScraperError::Network(Box::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Timeout",
                    ))),
                ),
            ],
            resolution_failures: 1,
            block_failures: 1,
            aborted_on_resolution: false,
        };

        assert_eq!(result.total, 10);
        assert_eq!(result.succeeded, 8);
        assert_eq!(result.failed, 2);
        assert_eq!(result.errors.len(), 2);
    }

    #[test]
    fn test_batch_progress_clone() {
        let progress = BatchProgress::new(5);
        progress.complete_one();
        progress.complete_one();

        let cloned = progress.clone();
        assert_eq!(cloned.total(), 5);
        assert_eq!(cloned.completed(), 2);
    }

    // =====================================================================
    // BatchResult structure tests
    // =====================================================================

    #[test]
    fn test_batch_result_all_succeeded() {
        let result = BatchResult {
            job_id: "job-ok".to_string(),
            total: 3,
            succeeded: 3,
            failed: 0,
            errors: vec![],
            resolution_failures: 0,
            block_failures: 0,
            aborted_on_resolution: false,
        };
        assert_eq!(result.total, result.succeeded);
        assert_eq!(result.failed, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_batch_result_all_failed() {
        let result = BatchResult {
            job_id: "job-fail".to_string(),
            total: 2,
            succeeded: 0,
            failed: 2,
            errors: vec![
                (
                    "https://a.com".to_string(),
                    ScraperError::Internal("error a".to_string()),
                ),
                (
                    "https://b.com".to_string(),
                    ScraperError::Internal("error b".to_string()),
                ),
            ],
            resolution_failures: 0,
            block_failures: 2,
            aborted_on_resolution: false,
        };
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, result.total);
        assert_eq!(result.errors.len(), 2);
    }

    #[test]
    fn test_batch_result_counts_consistent() {
        let errors: Vec<(String, ScraperError)> = (0..5)
            .map(|i| {
                (
                    format!("url-{i}"),
                    ScraperError::Internal(format!("err-{i}")),
                )
            })
            .collect();
        let result = BatchResult {
            job_id: "job-mixed".to_string(),
            total: 10,
            succeeded: 5,
            failed: 5,
            errors,
            resolution_failures: 0,
            block_failures: 5,
            aborted_on_resolution: false,
        };
        assert_eq!(result.succeeded + result.failed, result.total);
        assert_eq!(result.errors.len(), result.failed);
    }

    // =====================================================================
    // BatchProcessor concurrency tests
    // =====================================================================

    #[test]
    fn test_batch_processor_various_concurrency_values() {
        for n in [1, 2, 4, 8, 16] {
            let processor = BatchProcessor::new(n).unwrap();
            assert_eq!(processor.max_concurrent(), n);
        }
    }

    #[test]
    fn test_batch_processor_single_concurrency() {
        let processor = BatchProcessor::new(1).unwrap();
        assert_eq!(processor.max_concurrent(), 1);
    }

    // =====================================================================
    // BatchJob status transitions
    // =====================================================================

    #[test]
    fn test_batch_job_lifecycle() {
        let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let mut job = BatchJob::new(
            "lifecycle".to_string(),
            vec!["https://example.com".to_string()],
            config,
        );

        assert_eq!(job.status, BatchJobStatus::Pending);

        job.start();
        assert_eq!(job.status, BatchJobStatus::Running);

        job.complete();
        assert_eq!(job.status, BatchJobStatus::Completed);
    }

    #[test]
    fn test_batch_job_failure_state() {
        let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let mut job = BatchJob::new("fail-job".to_string(), vec![], config);

        job.fail("network error".to_string());
        assert_eq!(
            job.status,
            BatchJobStatus::Failed("network error".to_string())
        );
    }

    #[test]
    fn test_batch_job_status_display() {
        assert_eq!(BatchJobStatus::Pending.to_string(), "Pending");
        assert_eq!(BatchJobStatus::Running.to_string(), "Running");
        assert_eq!(BatchJobStatus::Completed.to_string(), "Completed");
        assert_eq!(
            BatchJobStatus::Failed("oops".to_string()).to_string(),
            "Failed: oops"
        );
    }

    // =====================================================================
    // BatchProgress edge cases
    // =====================================================================

    #[test]
    fn test_batch_progress_percent_partial() {
        let progress = BatchProgress::new(4);
        progress.start_one();
        progress.complete_one();
        progress.start_one();
        progress.fail_one();

        assert!((progress.percent() - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_batch_progress_in_progress_count() {
        let progress = BatchProgress::new(5);
        assert_eq!(progress.in_progress(), 0);

        progress.start_one();
        assert_eq!(progress.in_progress(), 1);

        progress.start_one();
        assert_eq!(progress.in_progress(), 2);

        progress.complete_one();
        assert_eq!(progress.in_progress(), 1);

        progress.fail_one();
        assert_eq!(progress.in_progress(), 0);
    }

    // =====================================================================
    // BatchError display tests
    // =====================================================================

    #[test]
    fn test_batch_error_empty_batch_display() {
        let err = BatchError::EmptyBatch;
        assert!(err.to_string().contains("no URLs"));
    }

    #[test]
    fn test_batch_error_crawl_failed_display() {
        let err = BatchError::CrawlFailed {
            url: "https://example.com".to_string(),
            error: CrawlError::InvalidUrl("bad url".to_string()),
        };
        let msg = err.to_string();
        assert!(msg.contains("example.com"));
        assert!(msg.contains("crawl failed"));
    }

    #[test]
    fn test_batch_error_semaphore_closed_display() {
        let err = BatchError::SemaphoreClosed;
        assert!(err.to_string().contains("semaphore"));
    }

    // =====================================================================
    // Bug R2-1: BudgetOverrides must survive the per-URL config rebuild
    // =====================================================================

    #[test]
    fn per_url_config_carries_budget_overrides() {
        // The batch path rebuilds a fresh CrawlerConfig for every URL; the
        // operator overrides staged on the base config must ride along or the
        // Engine silently re-derives the auto tiers.
        let overrides = crate::domain::budget::BudgetOverrides {
            crawl: crate::domain::budget::tiers::CrawlConcurrency::new(3).ok(),
            rate_burst: crate::domain::budget::tiers::BurstPermits::new(7).ok(),
            ..crate::domain::budget::BudgetOverrides::default()
        };
        let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
            .budget_overrides(overrides)
            .build();

        let per_url =
            build_per_url_config("https://example.org/seed", &base).expect("valid per-URL seed");

        assert_eq!(
            per_url.budget_overrides.crawl.map(|c| c.get()),
            Some(3),
            "explicit --concurrency must survive the per-URL rebuild"
        );
        assert_eq!(
            per_url.budget_overrides.rate_burst.map(|b| b.get()),
            Some(7),
            "explicit --rate-limit-burst must survive the per-URL rebuild"
        );
    }

    #[test]
    fn per_url_config_carries_jitter_ms() {
        // Same class as the budget overrides above: a knob staged on the
        // base config must survive the per-URL rebuild, or the per-URL
        // engine builds a limiter WITHOUT the jitter (#2610).
        let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
            .jitter_ms(250)
            .build();

        let per_url =
            build_per_url_config("https://example.org/seed", &base).expect("valid per-URL seed");

        assert_eq!(
            per_url.jitter_ms, 250,
            "the configured jitter must survive the per-URL rebuild"
        );
    }

    #[test]
    fn per_url_config_forces_seed_only_scrape() {
        // #1215 (F-38): the batch path rebuilds a fresh CrawlerConfig for
        // every URL, and the rebuild MUST pin seed-only scope no matter what
        // crawl budget the base config carries — otherwise every seed is
        // BFS-expanded and export records duplicate (F-02).
        let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
            .max_depth(2)
            .max_pages(10)
            .build();

        let per_url =
            build_per_url_config("https://example.org/seed", &base).expect("valid per-URL seed");

        assert_eq!(
            per_url.max_depth, 0,
            "batch per-URL crawl must be seed-only (max_depth 0)"
        );
        assert_eq!(
            per_url.max_pages, 1,
            "batch per-URL crawl must fetch exactly one page (max_pages 1)"
        );
        assert_eq!(
            per_url.seed_url.as_str(),
            "https://example.org/seed",
            "the batch URL becomes the seed"
        );
    }

    #[test]
    fn per_url_config_rejects_invalid_url() {
        let base = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let err = build_per_url_config("not a url", &base);
        assert!(matches!(err, Err(CrawlError::InvalidUrl(_))));
    }

    // =====================================================================
    // #1428: the run-scoped limiter is built from the base config, once
    // per run — one shared bucket, so `--delay-ms` paces across URLs.
    // =====================================================================

    #[test]
    fn batch_rate_limiter_none_at_zero_delay() {
        // No pacing at all — no delay AND no jitter — means "no pacing": no
        // bucket is allocated and each engine keeps its historical
        // per-engine limiter (zero overhead, zero drift — the
        // `build_scrape_rate_limiter` contract in `scrape_flow.rs`).
        let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
            .delay_ms(0)
            .build();

        assert!(
            build_batch_rate_limiter(&base).is_none(),
            "zero delay must build no bucket"
        );
    }

    #[test]
    fn batch_rate_limiter_some_for_a_jitter_without_a_delay() {
        // A jitter alone IS a pacing request (#2610) — the crawl path
        // already answers that way, so the batch path must not answer
        // "no pacing" for the same input.
        let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
            .delay_ms(0)
            .jitter_ms(500)
            .build();

        assert!(
            build_batch_rate_limiter(&base).is_some(),
            "a jitter without a delay must still build the run-scoped bucket"
        );
    }

    #[cfg_attr(
        miri,
        ignore = "quanta CPUID detection (raw-cpuid inline asm) unsupported by Miri"
    )]
    #[test]
    fn batch_rate_limiter_some_at_positive_delay() {
        let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
            .delay_ms(400)
            .build();

        assert!(
            build_batch_rate_limiter(&base).is_some(),
            "a positive delay must build the run-scoped bucket"
        );
    }

    /// #1369 seam pin: every knob of the batch `EngineOptions` must come from
    /// an explicit source — the robots value from the base config, the sink
    /// from the processor, and the knobs the batch has no source for set to
    /// the values the old `crawl_site` entry hardcoded (never a silent
    /// default that could drift with `EngineOptions::default()`).
    #[test]
    fn batch_engine_options_make_every_knob_source_explicit() {
        let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
            .ignore_robots(true)
            .build();
        let options = build_batch_engine_options(&base, None, None);
        assert!(
            options.ignore_robots,
            "the batch's robots source is the base config"
        );
        assert!(
            options.checkpoint_path.is_none(),
            "batch has no checkpoint source — must stay off explicitly"
        );
        assert!(
            !options.session_pool_enabled,
            "batch has no pool source — must stay off explicitly"
        );
        assert_eq!(
            options.js_strategy,
            JsStrategy::Static,
            "batch has no js-strategy source — must stay static explicitly"
        );
        assert!(
            options.downloader_factory.is_none(),
            "no factory means the recorded strategy cannot render — the
             static batch path must not grow one by accident"
        );
        assert!(
            options.content_sink.is_none(),
            "no sink attached leaves the batch metadata-only (the #631 gotcha)"
        );
        assert!(
                options.rate_limiter.is_none(),
                "the pacer is NOT part of the options bag — `process_batch_cancellable` wires \
                 the run-scoped bucket from `build_batch_rate_limiter` onto the built options (#1428)"
            );

        let sink = Arc::new(crate::application::crawler::content_sink::InMemoryContentSink::new());
        let options = build_batch_engine_options(&base, Some(sink), None);
        assert!(
            options.content_sink.is_some(),
            "the processor's shared sink must ride on the options"
        );

        let enforcing = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        assert!(
            !build_batch_engine_options(&enforcing, None, None).ignore_robots,
            "the default config's respect-robots must flow through both ways"
        );
    }

    /// #1375 drift guard: the transport scalars the batch does NOT source
    /// explicitly ride `..Default::default()` in
    /// [`build_batch_engine_options`]. That equivalence was audited against
    /// the old hardcoded `crawl_site_inner` policy when #1369 migrated the
    /// batch; this pin re-derives the same values from independent literals
    /// (never from `EngineOptions::default()` itself), so a future evolution
    /// of the default breaks THIS test with a named knob instead of silently
    /// re-policing every `--batch` transport knob.
    ///
    /// If this test fails after an intentional default change, the batch
    /// policy must be re-audited and the new values pinned here — the fix is
    /// never to delete the pin (that reintroduces silent drift, #1375).
    #[test]
    fn batch_engine_options_transport_scalars_pin_the_audited_policy() {
        let base = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let options = build_batch_engine_options(&base, None, None);

        // Expected values are literals copied from the audited
        // `crawl_site_inner` TransportPolicy (engine.rs #1369 migration),
        // NOT reads of `EngineOptions::default()`.
        assert_eq!(
            options.checkpoint_interval, 100,
            "checkpoint interval the old entry hardcoded"
        );
        assert_eq!(
            options.tls_emulation,
            wreq_util::Profile::Chrome145,
            "TLS fingerprint the old entry hardcoded"
        );
        assert!(!options.ignore_waf, "WAF bypass stays off in batch");
        assert_eq!(
            options.max_retries, 3,
            "retry ceiling the old entry hardcoded"
        );
        assert_eq!(
            options.backoff_base_ms, 1000,
            "backoff base the old entry hardcoded"
        );
        assert_eq!(
            options.backoff_max_ms, 10_000,
            "backoff ceiling the old entry hardcoded"
        );
        assert_eq!(
            options.obscura_binary, "obscura",
            "hybrid layer-2 binary default the old entry hardcoded"
        );
        assert_eq!(
            options.post_load_wait,
            crate::domain::PostLoadWait::Idle,
            "chromium settlement default the old entry hardcoded"
        );
        assert!(
            !options.autoscale_enabled,
            "RAM autoscaling stays off in batch"
        );
    }

    /// Shared in-flight gauge + six-node star topology (seed + 5 leaves) for
    /// the R2-1 diagnostics: counts every request and the high-water mark of
    /// concurrent responses so the scheduler bound derived from the operator
    /// override is observable end to end.
    struct SixNodeGauge {
        inflight: Arc<AtomicUsize>,
        max_inflight: Arc<AtomicUsize>,
        total_requests: Arc<AtomicUsize>,
        seed_uri: String,
    }

    impl Clone for SixNodeGauge {
        fn clone(&self) -> Self {
            Self {
                inflight: Arc::clone(&self.inflight),
                max_inflight: Arc::clone(&self.max_inflight),
                total_requests: Arc::clone(&self.total_requests),
                seed_uri: self.seed_uri.clone(),
            }
        }
    }

    impl wiremock::Respond for SixNodeGauge {
        fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
            let current = self.inflight.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            self.max_inflight.fetch_max(current, AtomicOrdering::SeqCst);
            self.total_requests.fetch_add(1, AtomicOrdering::SeqCst);
            // Force overlap when the scheduler bound allows parallel fetches,
            // so an over-broad bound is actually observed by the high-water mark.
            std::thread::sleep(std::time::Duration::from_millis(30));
            self.inflight.fetch_sub(1, AtomicOrdering::SeqCst);
            if request.url.path() == "/" {
                let links: String = (0..5)
                    .map(|i| format!(r#"<a href="{}/p{i}">n{i}</a>"#, self.seed_uri))
                    .collect();
                wiremock::ResponseTemplate::new(200)
                    .set_body_string(format!("<html><body>{links}</body></html>"))
            } else {
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("<html><body>leaf</body></html>")
            }
        }
    }

    /// Six-node diagnostic (bug R2-1, scope pinned by #1215): with `crawl = 1`
    /// staged as an operator override on the batch base config, the Engine
    /// reached through `BatchProcessor.process_single_url ->
    /// crawl_site_with_options`
    /// must still honor the explicit `--concurrency` override — and, since
    /// #1215, it scrapes the seed ONLY: the base config's `max_depth 1` /
    /// `max_pages 10` must NOT expand into the 5 discovered leaves, even
    /// though the same gauge topology would expose such an expansion.
    #[cfg(not(miri))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn batch_mode_scrapes_seed_only_with_concurrency_override() {
        let _entry_off = entry_off();
        use crate::domain::budget::{
            tiers::{BurstPermits, CrawlConcurrency},
            BudgetOverrides,
        };

        let server = wiremock::MockServer::start().await;
        let gauge = SixNodeGauge {
            inflight: Arc::new(AtomicUsize::new(0)),
            max_inflight: Arc::new(AtomicUsize::new(0)),
            total_requests: Arc::new(AtomicUsize::new(0)),
            seed_uri: server.uri(),
        };
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(gauge.clone())
            .mount(&server)
            .await;

        let seed = Url::parse(&format!("{}/", server.uri())).unwrap();
        let base_config = CrawlerConfig::builder(seed)
            .max_depth(1)
            .max_pages(10)
            .concurrency(std::num::NonZeroUsize::new(16).expect("16 is non-zero")) // configured value must be beaten by the override
            .timeout_secs(5)
            .ignore_robots(true)
            .budget_overrides(BudgetOverrides {
                crawl: CrawlConcurrency::new(1).ok(),
                rate_burst: BurstPermits::new(4).ok(),
                ..BudgetOverrides::default()
            })
            .build();

        let job = BatchJob::new(
            "r2-1-six-node".to_string(),
            vec![format!("{}/", server.uri())],
            base_config,
        );
        let processor = BatchProcessor::new(2).unwrap();
        let result = processor.process_batch(job).await.unwrap();

        assert_eq!(result.succeeded, 1, "the single batch URL must succeed");
        assert_eq!(
            gauge.total_requests.load(AtomicOrdering::SeqCst),
            1,
            "#1215: batch scrapes the seed only — the 5 discovered leaves must NOT be crawled"
        );
        assert_eq!(
            gauge.max_inflight.load(AtomicOrdering::SeqCst),
            1,
            "override crawl=1 must cap concurrent fetches at 1 through batch mode"
        );
    }
}
