//! Batch manager — queue-based orchestrator for multiple batch jobs
//!
//! The [`BatchManager`] holds a queue of [`BatchJob`]s and processes them
//! through the existing [`BatchProcessor`]. It adds:
//!
//! - Job queue management (submit, status查询)
//! - URL ingestion from stdin or files
//! - Batch result aggregation
//!
//! # Architecture
//!
//! ```text
//! CLI (--batch / --batch-file)
//!     ↓
//! BatchManager::from_urls()
//!     ↓
//! BatchJob (created per input set)
//!     ↓
//! BatchProcessor::process_batch()
//!     ↓
//! BatchResult
//! ```

use std::num::NonZeroUsize;
use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::input_lines::{parse_url_lines, warn_rejected_lines, BatchInputLines};
use super::processor::{BatchError, BatchProcessor, BatchResult};
use super::{BatchJob, BatchJobStatus};
use crate::application::crawler::content_sink::CrawlContentSink;
use crate::domain::CrawlerConfig;
use crate::error::ScraperError;

/// Queue-based manager for batch crawl jobs
///
/// Holds pending jobs and processes them through [`BatchProcessor`].
/// Use [`BatchManager::from_urls`] to create from a list of URLs,
/// or [`BatchManager::from_file`] / [`BatchManager::from_stdin`] for I/O sources.
pub struct BatchManager {
    processor: BatchProcessor,
    jobs: Vec<BatchJob>,
    /// Input accounting when the URLs came from `--batch-file`/stdin (#2795);
    /// `None` when the manager was built from in-process URLs.
    input_stats: Option<BatchInputLines>,
}

impl BatchManager {
    /// Create a new batch manager with the given concurrency limit
    ///
    /// `max_concurrent` is typed [`NonZeroUsize`]: a zero limit would starve
    /// the batch semaphore, so the invalid case cannot even be constructed
    /// (#2355).
    #[must_use]
    pub fn new(max_concurrent: NonZeroUsize) -> Self {
        Self {
            processor: BatchProcessor::new_nonzero(max_concurrent),
            jobs: Vec::new(),
            input_stats: None,
        }
    }

    /// Create a batch manager from a list of URLs using a shared config
    ///
    /// All URLs land in ONE [`BatchJob`] so [`BatchProcessor::process_batch`]
    /// can fan them out across its semaphore. One job per URL (the previous
    /// shape) made `--batch-concurrency` a no-op: each job held a single URL,
    /// the semaphore never contended, and jobs ran strictly sequentially
    /// (#653).
    pub fn from_urls(
        urls: Vec<String>,
        config: CrawlerConfig,
        max_concurrent: NonZeroUsize,
    ) -> Self {
        let mut manager = Self::new(max_concurrent);
        if urls.is_empty() {
            return manager;
        }
        manager
            .jobs
            .push(BatchJob::new("batch-all".to_string(), urls, config));
        manager
    }

    /// Total number of URLs queued across every job.
    ///
    /// [`job_count`](Self::job_count) is now 1 for a whole batch (#653), so
    /// callers reporting batch size must use this instead.
    #[must_use]
    pub fn url_count(&self) -> usize {
        self.jobs.iter().map(|j| j.urls.len()).sum()
    }

    /// Flatten all URLs across every job. Used by the dry-run path to report
    /// the exact set that would be scraped without performing any fetch.
    pub fn urls(&self) -> Vec<String> {
        self.jobs
            .iter()
            .flat_map(|j| j.urls.iter().cloned())
            .collect()
    }

    /// Capture every fetched page body into `sink` (#631).
    ///
    /// Propagates to the underlying [`BatchProcessor`] so the CLI can export
    /// the crawled content instead of discarding it with the crawl metadata.
    #[must_use]
    pub fn with_content_sink(mut self, sink: std::sync::Arc<dyn CrawlContentSink>) -> Self {
        self.processor = self.processor.with_content_sink(sink);
        self
    }

    /// Adopt `root` as the run identity of every job this manager runs
    /// (#1439 — propagates to the underlying [`BatchProcessor`], where each
    /// per-URL crawl engine shares it).
    #[must_use]
    pub fn with_correlation(mut self, root: crate::domain::CorrelationId) -> Self {
        self.processor = self.processor.with_correlation(root);
        self
    }

    /// Create a batch manager from a single batch job
    pub fn with_job(mut self, job: BatchJob) -> Self {
        self.jobs.push(job);
        self
    }

    /// Input accounting for `--batch-file`/stdin sources, when present (#2795).
    #[must_use]
    pub fn input_lines(&self) -> Option<&BatchInputLines> {
        self.input_stats.as_ref()
    }

    /// Read URLs from a file (one URL per line, blank lines and `#` comments
    /// ignored), retaining the per-line input accounting (#2795)
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if the file cannot be read.
    pub fn from_file(
        path: &Path,
        config: CrawlerConfig,
        max_concurrent: NonZeroUsize,
    ) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(path)?;
        let (urls, stats) = parse_url_lines(&content);
        Ok(Self::from_parsed_lines(urls, stats, config, max_concurrent))
    }

    /// Read URLs from stdin (one URL per line), retaining the per-line input
    /// accounting (#2795)
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if stdin cannot be read.
    pub fn from_stdin(
        config: CrawlerConfig,
        max_concurrent: NonZeroUsize,
    ) -> Result<Self, std::io::Error> {
        let mut content = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut content)?;
        let (urls, stats) = parse_url_lines(&content);
        Ok(Self::from_parsed_lines(urls, stats, config, max_concurrent))
    }

    /// Assemble a manager from already-parsed input lines: warn about the
    /// rejected ones, then retain the accounting so the CLI can reconcile it
    /// (#2795).
    fn from_parsed_lines(
        urls: Vec<String>,
        stats: BatchInputLines,
        config: CrawlerConfig,
        max_concurrent: NonZeroUsize,
    ) -> Self {
        warn_rejected_lines(&stats.rejected_details);
        let mut manager = Self::from_urls(urls, config, max_concurrent);
        manager.input_stats = Some(stats);
        manager
    }

    /// Process all queued jobs sequentially
    ///
    /// Each job is processed via [`BatchProcessor::process_batch`].
    /// Returns a vector of [`BatchResult`] for each job.
    pub async fn process_all(&self) -> Vec<Result<BatchResult, BatchError>> {
        self.process_all_cancellable(&CancellationToken::new())
            .await
    }

    /// Process all queued jobs, stopping early when `cancel` is fired.
    ///
    /// The in-flight job drains before returning, so its captured pages still
    /// reach the sink; remaining jobs are skipped (#653).
    pub async fn process_all_cancellable(
        &self,
        cancel: &CancellationToken,
    ) -> Vec<Result<BatchResult, BatchError>> {
        let mut results = Vec::with_capacity(self.jobs.len());
        for job in &self.jobs {
            if cancel.is_cancelled() {
                warn!(
                    remaining = self.jobs.len() - results.len(),
                    "shutdown requested — skipping remaining batch jobs"
                );
                break;
            }
            info!("Processing batch job: {}", job.id);
            results.push(
                self.processor
                    .process_batch_cancellable(job.clone(), cancel)
                    .await,
            );
        }
        results
    }

    /// Process all queued jobs and return an aggregated summary
    pub async fn process_all_summary(&self) -> BatchManagerSummary {
        self.process_all_summary_cancellable(&CancellationToken::new())
            .await
    }

    /// Process all queued jobs with cancellation support and aggregate them.
    pub async fn process_all_summary_cancellable(
        &self,
        cancel: &CancellationToken,
    ) -> BatchManagerSummary {
        let results = self.process_all_cancellable(cancel).await;
        // #2795: the input accounting travels WITH the summary so the CLI can
        // reconcile lines read vs valid/rejected/blank/comment in one line.
        let mut summary = BatchManagerSummary {
            input_lines: self.input_stats.clone(),
            ..BatchManagerSummary::default()
        };

        // Consume by value: `ScraperError` is not `Clone`, so error tuples are
        // moved into the summary rather than cloned (#537).
        for result in results {
            match result {
                Ok(r) => {
                    summary.total_urls += r.total;
                    summary.succeeded += r.succeeded;
                    summary.failed += r.failed;
                    // #2823: resolution and site refusals are two populations,
                    // reported apart so a resolver outage cannot masquerade as
                    // a site blocking the run.
                    summary.resolution_failures += r.resolution_failures;
                    summary.block_failures += r.block_failures;
                    summary.aborted_on_resolution |= r.aborted_on_resolution;
                    summary.errors.extend(r.errors);
                },
                Err(e) => {
                    summary.failed += 1;
                    // `BatchError` is not a `ScraperError`; wrap into Internal
                    // (classifies as InternalFatal) so batch-level failures
                    // escalate the exit code.
                    summary.errors.push((
                        "batch-error".to_string(),
                        ScraperError::Internal(format!("batch-error: {e}")),
                    ));
                },
            }
        }

        summary
    }

    /// Get the number of queued jobs
    #[must_use]
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Get all job statuses
    #[must_use]
    pub fn statuses(&self) -> Vec<(&str, &BatchJobStatus)> {
        self.jobs
            .iter()
            .map(|j| (j.id.as_str(), &j.status))
            .collect()
    }
}

/// Aggregated summary across all batch jobs in a manager
#[derive(Debug, Default)]
pub struct BatchManagerSummary {
    /// Total number of URLs across all jobs
    pub total_urls: usize,
    /// Number of successfully processed URLs
    pub succeeded: usize,
    /// Number of failed URLs
    pub failed: usize,
    /// Failed URLs attributed to NAME RESOLUTION — our resolver, not the site
    /// refusing us (#2823).
    pub resolution_failures: usize,
    /// Failed URLs the site refused (everything else) (#2823).
    pub block_failures: usize,
    /// Whether any job stopped dispatching because the resolver was down.
    pub aborted_on_resolution: bool,
    /// Errors from failed jobs, preserving the [`ScraperError`] variant so
    /// exit-code routing can aggregate severity via
    /// [`ScraperError::classify`] (#537).
    pub errors: Vec<(String, ScraperError)>,
    /// Input accounting when the URLs came from `--batch-file`/stdin (#2795);
    /// lets the CLI summary reconcile lines read vs valid/rejected/blank/
    /// comment. `None` when the manager was built from in-process URLs.
    pub input_lines: Option<BatchInputLines>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn test_config() -> CrawlerConfig {
        CrawlerConfig::new(Url::parse("https://example.com").unwrap())
    }

    /// Test-side lift of a raw literal into the `NonZeroUsize` the batch
    /// constructors now require (#2355).
    fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("test concurrency must be non-zero")
    }

    #[test]
    fn test_manager_from_urls() {
        let urls = vec![
            "https://example.com".to_string(),
            "https://example.com/about".to_string(),
        ];
        let manager = BatchManager::from_urls(urls, test_config(), nz(3));
        // #653: every URL shares ONE job so the processor's semaphore governs
        // concurrency instead of the manager's sequential job loop.
        assert_eq!(manager.job_count(), 1);
        assert_eq!(manager.url_count(), 2);
    }

    #[test]
    fn test_manager_from_file() {
        let dir = std::env::temp_dir().join("batch_manager_test");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("urls.txt");
        std::fs::write(&file_path, "https://a.com\nhttps://b.com\n").unwrap();

        let manager = BatchManager::from_file(&file_path, test_config(), nz(2)).unwrap();
        assert_eq!(manager.job_count(), 1);
        assert_eq!(manager.url_count(), 2);
        // #2795: a file source retains its input accounting.
        let stats = manager.input_lines().expect("file source must carry stats");
        assert_eq!((stats.lines_read, stats.valid, stats.rejected), (2, 2, 0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_manager_urls_flattens_all_jobs() {
        // Regression for #784: the dry-run path lists batch URLs via
        // `urls()`, so it must flatten every job's URL list correctly.
        let urls = vec![
            "https://a.com".to_string(),
            "https://b.com/page".to_string(),
        ];
        let manager = BatchManager::from_urls(urls, test_config(), nz(2));
        assert_eq!(manager.urls(), vec!["https://a.com", "https://b.com/page"]);

        let empty = BatchManager::new(nz(2));
        assert!(empty.urls().is_empty());
    }

    #[test]
    fn test_manager_statuses() {
        let urls = vec!["https://a.com".to_string()];
        let manager = BatchManager::from_urls(urls, test_config(), nz(1));
        let statuses = manager.statuses();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].0, "batch-all");
        assert_eq!(*statuses[0].1, BatchJobStatus::Pending);
    }

    #[tokio::test]
    async fn test_process_all_empty() {
        let manager = BatchManager::new(nz(3));
        let results = manager.process_all().await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn cancelled_manager_skips_every_queued_job() {
        // Regression for #653: SIGINT/SIGTERM must stop the batch queue rather
        // than being ignored until the whole run finishes.
        let urls = vec!["https://a.com".to_string(), "https://b.com".to_string()];
        let manager = BatchManager::from_urls(urls, test_config(), nz(2));

        let cancel = CancellationToken::new();
        cancel.cancel();

        let results = manager.process_all_cancellable(&cancel).await;
        assert!(
            results.is_empty(),
            "no job may start after a shutdown signal"
        );
    }

    #[tokio::test]
    async fn test_process_all_summary_empty() {
        let manager = BatchManager::new(nz(3));
        let summary = manager.process_all_summary().await;
        assert_eq!(summary.total_urls, 0);
        assert_eq!(summary.succeeded, 0);
        assert_eq!(summary.failed, 0);
    }

    #[test]
    fn test_manager_with_job() {
        let config = test_config();
        let job = BatchJob::new(
            "custom-job".to_string(),
            vec!["https://example.com".to_string()],
            config,
        );
        let manager = BatchManager::new(nz(2)).with_job(job);
        assert_eq!(manager.job_count(), 1);
        assert_eq!(manager.statuses()[0].0, "custom-job");
    }
}
