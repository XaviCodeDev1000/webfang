//! The run summary — the counters the run hands back to its caller.
//!
//! The summary is the boundary where the run's failures stop being internal
//! bookkeeping and become the caller's verdict: the error total, the
//! per-category breakdown (#374), the SEPARATE resolution counter (#2823) and
//! the FIRST classified transport cause (#2824).
//!
//! Extracted from `lifecycle.rs` so the lifecycle file stays under the
//! ~500-line rule: a pure move, no behaviour change.

use tracing::info;

use super::Engine;
use crate::domain::{CrawlErrorCategory, CrawlResult};

impl Engine {
    /// Snapshot the collector and the error counters into a [`CrawlResult`].
    ///
    /// Shared by the success tail of [`Self::run`] and
    /// [`Self::fail_run_with_partials`]: the summary a caller sees must be
    /// identical on both paths (#2338) — one extraction, one place.
    ///
    /// Callers must have released the task context first, so every collector
    /// sender is gone and the mpsc drain terminates.
    pub(super) async fn collect_result_summary(&mut self) -> CrawlResult {
        let collected_urls = std::mem::take(&mut self.collector).collect().await;
        let total_pages = collected_urls.len();
        let errors = self.error_count.load(std::sync::atomic::Ordering::SeqCst);
        let breakdown = self.collect_error_breakdown();
        let mut summary = CrawlResult::new(collected_urls, total_pages, errors, breakdown);
        // The first classified transport cause of the run travels with the
        // summary (#2824): the failures themselves are counted, not kept, and
        // the application boundary needs one cause to name the failure it reports.
        if let Some(cause) = self.transport_cause.get().copied() {
            summary = summary.with_transport_cause(cause);
        }
        // Fetch failures attributed to NAME RESOLUTION (#2823): counted apart
        // from the category breakdown, which folds every transport cause into
        // `Network`.
        summary.resolution_failures = self
            .resolution_failures
            .load(std::sync::atomic::Ordering::SeqCst);
        summary
    }

    /// Build the per-category error breakdown map (issue #374).
    fn collect_error_breakdown(&self) -> std::collections::BTreeMap<CrawlErrorCategory, usize> {
        CrawlErrorCategory::ALL
            .iter()
            .filter_map(|cat| {
                let count =
                    self.error_breakdown[cat.index()].load(std::sync::atomic::Ordering::SeqCst);
                (count > 0).then_some((*cat, count))
            })
            .collect()
    }

    /// Emit the structured crawl summary (issue #356 Fase 4, error breakdown #374).
    pub(super) fn log_crawl_summary(
        &self,
        total_pages: usize,
        errors: usize,
        start: std::time::Instant,
    ) {
        let duration = start.elapsed();
        let succeeded = total_pages.saturating_sub(errors);
        let summary_rate = if duration.as_secs_f64() > 0.0 {
            total_pages as f64 / duration.as_secs_f64()
        } else {
            0.0
        };
        info!(
            total_pages = total_pages,
            succeeded = succeeded,
            errors = errors,
            errors_waf = self.error_breakdown[CrawlErrorCategory::Waf.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_http = self.error_breakdown[CrawlErrorCategory::Http.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_timeout = self.error_breakdown[CrawlErrorCategory::Timeout.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_network = self.error_breakdown[CrawlErrorCategory::Network.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_rate_limit = self.error_breakdown[CrawlErrorCategory::RateLimit.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_extraction = self.error_breakdown[CrawlErrorCategory::Extraction.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_internal = self.error_breakdown[CrawlErrorCategory::Internal.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_panic = self.error_breakdown[CrawlErrorCategory::Panic.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            duration_secs = duration.as_secs(),
            // #1610 (OBS-H7): `duration_secs` is whole seconds, so every run
            // under one second reported 0 and the crawl cost was only
            // recoverable from the span_close. The millisecond figure makes
            // the event queryable on its own; `duration_secs` is kept
            // untouched because existing tooling reads it.
            duration_ms = duration.as_millis() as u64,
            pages_per_sec = summary_rate,
            trace_id = %self.correlation_id.trace_id(),
            "crawl completed"
        );
    }
}
