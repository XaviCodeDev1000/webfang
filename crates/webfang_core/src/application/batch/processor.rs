//! Batch processor — concurrent execution of multiple crawl jobs
//!
//! Uses [`tokio::sync::Semaphore`] for job-level concurrency control.
//! Each URL in the batch is a separate `crawl_site_with_options()` call, with
//! the run-wide [`crate::application::crawler::engine::EngineOptions`] built once by `build_batch_engine_options`
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

use super::processor_url::{
    build_batch_engine_options, build_batch_rate_limiter, process_single_url,
};
use super::BatchJob;
use crate::application::crawler::content_sink::CrawlContentSink;
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
        let mut base_options = build_batch_engine_options(&base_config, self.content_sink.clone());
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

        let mut cancelled = false;
        for url_str in &job.urls {
            if cancel.is_cancelled() {
                cancelled = true;
                warn!(
                    job_id = %job.id,
                    "shutdown requested — no further URLs will be dispatched"
                );
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

            progress.start_one();

            join_set.spawn(
                async move {
                    let _permit = permit; // Hold permit for duration of task
                    let result = process_single_url(&url, config, options, &correlation).await;
                    (url, result)
                }
                .in_current_span(),
            );
        }

        // Collect results as tasks complete
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok((url, Ok(_))) => {
                    progress.complete_one();
                    info!("Completed crawl for {url}");
                },
                Ok((url, Err(e))) => {
                    progress.fail_one();
                    // Preserve the full variant through the CrawlError ->
                    // ScraperError conversion (#537): severity routing needs
                    // classify(), which a flattened string cannot provide.
                    let scraper_err = ScraperError::from(e);
                    crate::infrastructure::observability::log_scrape_error_with_cause(
                        &scraper_err,
                        &url,
                        "batch_crawl",
                        Some(&run_root),
                        "batch URL crawl failed",
                    );
                    errors.push((url, scraper_err));
                },
                Err(e) => {
                    progress.fail_one();
                    error!(error = %e, "batch crawl task panicked");
                    errors.push((
                        "unknown".to_string(),
                        ScraperError::Internal(format!("task-panic: {e}")),
                    ));
                },
            }
        }

        let succeeded = progress.completed();
        let failed = progress.failed();
        let total = progress.total();

        if cancelled {
            job.fail("cancelled by shutdown signal".to_string());
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
        })
    }
}

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

#[cfg(test)]
#[path = "processor_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "processor_run_tests.rs"]
mod tests_run;
