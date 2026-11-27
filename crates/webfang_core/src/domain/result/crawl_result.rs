//! Crawl result type
//!
//! Represents the outcome of a crawling operation.

use std::collections::BTreeMap;

use crate::domain::crawl_job::DiscoveredUrl;
use crate::domain::error::cause::NetworkCause;
use crate::domain::CrawlErrorCategory;

/// What a resumed crawl decided to skip and what it decided to re-drive.
///
/// Path-free by contract: the counts are domain knowledge, the state-file path
/// is the CLI's to name (#3097 / T4). Produced from the loaded checkpoint by
/// the same `visited ∩ settled` rule the scheduler applies, so the number the
/// user reads is the number the run acted on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumeStats {
    /// URLs the resume skipped: `visited ∩ settled`.
    ///
    /// Named for what it is, not for the set it came from: the checkpoint's
    /// `visited` holds every dispatched URL, and only its terminal subset is
    /// skipped here (#3079).
    pub skipped: usize,
    /// Terminal verdicts the checkpoint carried.
    pub settled: usize,
    /// Visited URLs that are NOT terminal — dispatched by a previous run and
    /// re-driven now because their page was never produced (#3079).
    pub re_driving: usize,
    /// The restored frontier, with settled URLs filtered out.
    pub queued: usize,
}

/// Crawl result containing discovered URLs
///
/// Following **api-must-use** and **api-non-exhaustive**.
#[derive(Debug, Clone, Default)]
#[must_use]
#[non_exhaustive]
pub struct CrawlResult {
    /// All discovered URLs
    pub urls: Vec<DiscoveredUrl>,
    /// Total number of pages crawled
    pub total_pages: usize,
    /// Number of errors encountered
    pub errors: usize,
    /// Error counts by category (issue #374)
    pub error_breakdown: BTreeMap<CrawlErrorCategory, usize>,
    /// The first classified transport cause of the run (#2824).
    ///
    /// The per-task failures are COUNTED, not kept: this is the one cause the
    /// application boundary needs to name the failure it reports. `None` when
    /// no task failed with a classified transport failure.
    pub transport_cause: Option<NetworkCause>,
    /// Fetch failures attributed to NAME RESOLUTION — our resolver, not the
    /// site refusing us (#2823).
    ///
    /// Deliberately OUTSIDE `error_breakdown`: the category taxonomy folds
    /// every transport cause into [`CrawlErrorCategory::Network`] (the sibling
    /// taxonomy issue owns that split), while a batch run needs to tell "the
    /// site refused us" from "the resolver is down" to decide whether to keep
    /// dispatching. A separate count is what lets it do that without moving a
    /// category.
    pub resolution_failures: usize,
    /// The resume decision this run took, when it resumed from a checkpoint
    /// (#3097 / T4).
    ///
    /// `None` on a fresh run and on a run whose checkpoint carried no
    /// dispatched URL: there is nothing to report, and reporting zeroes would
    /// claim a resume that never happened.
    pub resume: Option<ResumeStats>,
}

impl CrawlResult {
    /// Create a new crawl result
    pub fn new(
        urls: Vec<DiscoveredUrl>,
        total_pages: usize,
        errors: usize,
        error_breakdown: BTreeMap<CrawlErrorCategory, usize>,
    ) -> Self {
        Self {
            urls,
            total_pages,
            errors,
            error_breakdown,
            transport_cause: None,
            resolution_failures: 0,
            resume: None,
        }
    }

    /// Attach the resume decision this run took (#3097 / T4).
    pub fn with_resume(mut self, stats: ResumeStats) -> Self {
        self.resume = Some(stats);
        self
    }

    /// Attach the classified transport cause of the run (#2824).
    pub fn with_transport_cause(mut self, cause: NetworkCause) -> Self {
        self.transport_cause = Some(cause);
        self
    }

    /// Create an empty crawl result
    pub fn empty() -> Self {
        Self::default()
    }

    /// Check if the result is empty
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_crawl_result_empty() {
        let result = CrawlResult::empty();
        assert!(result.is_empty());
        assert_eq!(result.total_pages, 0);
        assert_eq!(result.errors, 0);
        assert!(result.error_breakdown.is_empty());
    }

    #[test]
    fn test_crawl_result_new() {
        let url = Url::parse("https://example.com").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::html(url, 0, parent);
        let result = CrawlResult::new(vec![discovered], 1, 0, BTreeMap::new());

        assert!(!result.is_empty());
        assert_eq!(result.total_pages, 1);
        assert_eq!(result.errors, 0);
        assert_eq!(result.urls.len(), 1);
        assert!(result.error_breakdown.is_empty());
    }

    #[test]
    fn test_crawl_result_with_breakdown() {
        let mut breakdown = BTreeMap::new();
        breakdown.insert(CrawlErrorCategory::Waf, 3);
        breakdown.insert(CrawlErrorCategory::Timeout, 2);
        let result = CrawlResult::new(vec![], 10, 5, breakdown);

        assert_eq!(result.errors, 5);
        assert_eq!(result.error_breakdown.len(), 2);
        assert_eq!(result.error_breakdown[&CrawlErrorCategory::Waf], 3);
        assert_eq!(result.error_breakdown[&CrawlErrorCategory::Timeout], 2);
    }
}
