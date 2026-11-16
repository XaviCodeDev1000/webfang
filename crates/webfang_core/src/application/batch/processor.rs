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

use super::BatchJob;
use crate::application::crawler::content_sink::CrawlContentSink;
use crate::application::crawler::engine::EngineOptions;
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::budget::detector::SystemDetector;
use crate::domain::budget::BudgetModel;
use crate::domain::{CorrelationId, CrawlError, CrawlErrorCategory, CrawlerConfig, JsStrategy};
use crate::error::ScraperError;
use crate::ValidUrl;

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

/// Build the per-URL [`CrawlerConfig`] used by [`process_single_url`].
///
/// #1215 (F-38): batch SCRAPES each input URL — exactly one page per URL.
/// The base config carries the CLI crawl budget (`max_depth`/`max_pages`),
/// which must NOT expand batch seeds: feeding it through verbatim turned
/// every seed into a BFS crawl and duplicated export records (F-02). Depth 0
/// fetches the seed only (link extraction is skipped once
/// `url_depth >= max_depth`); `max_pages 1` is the backstop so the seed
/// itself can never be budget-starved. Every other knob (patterns, robots,
/// TLS profile, timeouts, delay, budget overrides) still rides along.
fn build_per_url_config(
    url: &str,
    base_config: &CrawlerConfig,
) -> Result<CrawlerConfig, CrawlError> {
    let parsed_url =
        url::Url::parse(url).map_err(|e| CrawlError::InvalidUrl(format!("{url}: {e}")))?;
    // Apply URL hardening policy (scheme allow-list + credential strip) to batch-file/stdin URLs.
    // This closes audit finding F-R3-1 (batch entry parity with #1240/#1260).
    let valid_url = ValidUrl::try_from_url(parsed_url)
        .map_err(|e| CrawlError::InvalidUrl(format!("{url}: {e}")))?;

    Ok(CrawlerConfig::builder(valid_url.as_url().clone())
        .max_depth(0)
        .max_pages(1)
        .concurrency(base_config.concurrency)
        .delay_ms(base_config.delay_ms)
        .jitter_ms(base_config.jitter_ms)
        .timeout_secs(base_config.timeout_secs)
        .ignore_robots(base_config.ignore_robots)
        .tls_emulation(base_config.tls_emulation)
        .exclude_patterns(base_config.exclude_patterns.clone())
        .include_patterns(base_config.include_patterns.clone())
        // Bug R2-1: operator budget overrides staged on the base config
        // must survive the per-URL rebuild or the Engine re-derives auto
        // tiers, silently ignoring --concurrency / --rate-limit-burst.
        .budget_overrides(base_config.budget_overrides)
        .build())
}

/// Build the RUN-scoped token bucket shared by every URL of one batch run (#1428).
///
/// Each batch URL runs its own engine (`process_single_url` →
/// `crawl_site_with_options`), and every engine mints a fresh limiter with
/// a full burst — so without this shared bucket `--delay-ms` is inert on
/// the batch path: each URL starts with a full burst and cross-URL spacing
/// is structurally impossible. The bucket is built from the SAME two inputs
/// `Engine::run` uses — `delay_ms` as the refill period and the budget
/// model's independent burst tier — so batch pacing shares the crawl
/// cadence policy (AGENTS.md fetch guard-chain stage 2: pacing happens
/// BEFORE any network is touched).
///
/// `delay_ms == 0` AND `jitter_ms == 0` returns `None`: no bucket is
/// allocated and each engine keeps its historical per-engine limiter, which
/// keeps an unthrottled run identical to the pre-fix behavior (zero
/// overhead, zero drift) — the same contract as
/// `build_scrape_rate_limiter` (`scrape_flow.rs`). A jitter on its own IS a
/// pacing request, so it wires the bucket even with a zero delay (#2610).
///
/// A construction failure degrades to `None` with a WARN, mirroring the
/// scrape-path precedent. It is unreachable in practice:
/// `SharedRateLimiter::new` rejects only a zero period (clamped to 1 ms)
/// or a zero burst (the budget tier is non-zero by construction).
fn build_batch_rate_limiter(base_config: &CrawlerConfig) -> Option<SharedRateLimiter> {
    // A zero delay means "no pacing" — but only when the operator asked for
    // no pacing. A jitter on its own IS a pacing request, so the run-scoped
    // limiter is built even with a zero base delay (#2610): the gate then
    // spaces the requests over the clamped 1 ms floor, exactly like the
    // crawl path's `delay_ms.max(1)`.
    if base_config.delay_ms == 0 && base_config.jitter_ms == 0 {
        return None;
    }
    let burst = BudgetModel::build(base_config.budget_overrides, &SystemDetector)
        .burst()
        .get();
    match SharedRateLimiter::new(
        &RateLimiterConfig::new(base_config.delay_ms, burst).with_jitter(base_config.jitter_ms),
    ) {
        Ok(limiter) => {
            info!(
                delay_ms = base_config.delay_ms,
                jitter_ms = base_config.jitter_ms,
                burst,
                "batch rate limiter wired (run scope)"
            );
            Some(limiter)
        },
        Err(e) => {
            warn!(
            error = %e,
            delay_ms = base_config.delay_ms,
            "batch rate limiter unavailable — continuing with per-engine limiters"
            );
            None
        },
    }
}

/// Build the run-wide [`EngineOptions`] shared by every URL of one batch run.
///
/// #1369: the batch entry used to route through the knobless `crawl_site` /
/// `crawl_site_capturing` shims, which hard-wired the engine transport policy
/// inside the engine. The explicit seam makes each knob's source visible:
///
/// - `ignore_robots`: the batch's only robots source is the base
///   [`CrawlerConfig`] — `build_per_url_config` copies the same value into
///   every per-URL config — so the run-wide option mirrors it. Identical to
///   what `crawl_site_inner` derived per call before.
/// - `content_sink`: the shared sink attached via
///   [`BatchProcessor::with_content_sink`] (#631); `None` keeps the batch
///   metadata-only (bodies discarded — the historical zero-files gotcha).
/// - `checkpoint_path` / `session_pool_enabled` / `js_strategy` /
///   `downloader_factory`: the batch has no operator source for these, so
///   they are set **explicitly** to the values the old entry hardcoded
///   (off / off / static / `None`) — never silently via a drifted default.
///   The remaining transport scalars ride `EngineOptions::default()`, which
///   is field-for-field the old `crawl_site_inner` policy (Chrome145, 3
///   retries, 1s/10s backoff, 100-page checkpoint interval).
///
/// Built once per run and cloned per URL task.
///
/// NOTE: the run-scoped pacer lives OUTSIDE this bag — see
/// [`build_batch_rate_limiter`], wired onto the built options by
/// `process_batch_cancellable`. The split keeps this constructor a pure
/// DTO build (its seam test pins every knob source) while the limiter
/// documents its own budget-derived source separately.
fn build_batch_engine_options(
    base_config: &CrawlerConfig,
    content_sink: Option<Arc<dyn CrawlContentSink>>,
) -> EngineOptions {
    EngineOptions {
        checkpoint_path: None,
        session_pool_enabled: false,
        ignore_robots: base_config.ignore_robots,
        js_strategy: JsStrategy::Static,
        downloader_factory: None,
        content_sink,
        ..Default::default()
    }
}

/// Process a single URL by creating a CrawlerConfig and running the explicit
/// engine entry with the run-wide options and the job's run-root
///
/// Creates a new seed-only `CrawlerConfig` for the given URL (#1215: one
/// page per URL, never a BFS expansion) and captures the fetched body into
/// the shared sink when one is attached (carried on `options.content_sink`,
/// #1369). `correlation` (#1439) is the job's ONE run-root, adopted by the
/// crawl engine so every URL of the batch shares the operation's identity.
///
/// Returns `Err(CrawlError)` if the crawl result has any errors (e.g., timeouts),
/// ensuring the batch processor correctly counts failed URLs.
async fn process_single_url(
    url: &str,
    base_config: CrawlerConfig,
    options: EngineOptions,
    correlation: &CorrelationId,
) -> Result<crate::domain::CrawlResult, CrawlError> {
    let config = build_per_url_config(url, &base_config)?;

    let result =
        crate::application::crawler::engine::crawl_site_with_options(config, options, correlation)
            .await?;

    // Treat any crawl errors (timeouts, etc.) as failures for batch processing,
    // but preserve severity (#537): the engine already partitioned them into
    // `error_breakdown` (issue #374). A genuinely-internal category (storage,
    // checkpoint, parse, panic) is a real bug and must stay `Internal` so the
    // run exits 3 (issue #537, phase 1). A purely transient/external failure
    // set (timeout, network, http, rate-limit, waf) must surface as a transient
    // `CrawlError` so it classifies `TransientRetriable`/`Backoff`/`Permanent`
    // and exits 69 — NOT as a bug (exit 3).
    if result.errors > 0 {
        let breakdown = &result.error_breakdown;

        // Defensive: errors reported without a category breakdown are treated
        // as internal failures (fail-safe → exit 3), matching the classify()
        // safety net for genuinely unknown errors.
        if breakdown.is_empty() {
            return Err(CrawlError::Internal(format!(
                "crawl completed with {} error(s)",
                result.errors
            )));
        }

        // A genuinely-internal category means a real bug. Mixed batches are
        // dominated by the worst severity, so any such category escalates to
        // exit 3 (matches `scraper_failure_for_internal_fatal` in the CLI).
        let has_internal_bug = [
            CrawlErrorCategory::Internal,
            CrawlErrorCategory::Extraction,
            CrawlErrorCategory::Panic,
        ]
        .iter()
        .any(|c| breakdown.get(c).copied().unwrap_or(0) > 0);
        if has_internal_bug {
            return Err(CrawlError::Internal(format!(
                "crawl completed with {} error(s)",
                result.errors
            )));
        }

        // Purely transient/external: surface as a transient error → exit 69.
        // A timeout is the most common batch failure here.
        if breakdown
            .get(&CrawlErrorCategory::Timeout)
            .copied()
            .unwrap_or(0)
            > 0
        {
            return Err(CrawlError::Timeout);
        }
        return Err(CrawlError::Connection {
            message: format!("crawl completed with {} transient error(s)", result.errors),
            source: None,
        });
    }

    Ok(result)
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
