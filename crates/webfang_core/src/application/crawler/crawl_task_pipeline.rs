//! Content-pipeline and link-extraction stage of the per-page crawl task.
//!
//! Moved verbatim from `crawl_task.rs` (mission #2827, task T7): running the
//! configured content pipeline, and extracting + queueing internal links.
//!
//! Security-sensitive: `extract_and_queue_links` implements the robots +
//! `is_internal_link` + `is_allowed` gating and the `Extraction` error
//! counter — moved byte-for-byte, nothing cleaned up.

use tracing::{debug, warn};
use url::Url;

use super::crawl_task_ctx::CrawlTaskCtx;
use crate::application::pipeline::{ScrapedItem, StageOutcome};
use crate::application::url_filter::is_allowed;
use crate::domain::crawler_port::UrlSource;
use crate::domain::url_validation::is_internal_link;
use crate::domain::{CrawlErrorCategory, DiscoveredUrl};
use crate::infrastructure::observability::{log_classified_error, log_scrape_error_with_cause};

/// Run the content pipeline if configured. Returns `true` when processing
/// should continue (no pipeline, or `Continue`); `false` when the item was
/// filtered, rejected, or failed (the caller should stop processing this page).
pub(super) async fn run_pipeline(
    ctx: &CrawlTaskCtx,
    url_str: &str,
    response: &str,
    status_code: u16,
) -> bool {
    let Some(ref pipeline) = ctx.pipeline else {
        return true;
    };
    let item = ScrapedItem {
        url: url_str.to_string(),
        raw_html: response.to_string(),
        text_content: None,
        metadata: std::collections::HashMap::new(),
        status_code,
        embeddings: None,
    };
    match pipeline.execute_pipeline(item).await {
        StageOutcome::Continue(_) => true,
        StageOutcome::Filtered { reason } => {
            debug!(url = %url_str, reason = %reason, "pipeline filtered item");
            false
        },
        StageOutcome::Rejected { reason } => {
            warn!(url = %url_str, reason = %reason, "pipeline rejected item");
            false
        },
        StageOutcome::Failed { class } => {
            // #865: the ErrorClass travels as a structured tracing field
            // (never string-formatted into the message) so per-class
            // runner policy is observable. Control flow is unchanged:
            // observability only, retry wiring is deferred (matrix L151).
            log_classified_error(
                &class,
                class,
                url_str,
                "pipeline",
                Some(&ctx.correlation_id),
                "pipeline failed",
            );
            false
        },
    }
}

/// Extract links from the page and enqueue internal, allowed, robots-permitted
/// links for further crawling (subject to the configured max depth).
pub(super) async fn extract_and_queue_links(
    ctx: &CrawlTaskCtx,
    response: &str,
    url_str: &str,
    url_depth: u8,
    parent_url: &Url,
) {
    if url_depth >= ctx.config.max_depth {
        return;
    }
    match ctx.link_extractor.extract_links(response, url_str) {
        Ok(links) => {
            for link in links {
                if let Ok(parsed_link) = Url::parse(&link) {
                    if let Some(seed_domain) = ctx.config.seed_url.host_str() {
                        let link_domain = parsed_link.host_str().unwrap_or("");
                        // Enqueue-time dedup is owned by the queue's `seen` set
                        // inside `push_prioritized`. The `visited` set is marked
                        // only at dispatch time (`CrawlScheduler::record_visit`),
                        // so a freshly discovered link must NOT be pre-marked
                        // here: doing so made `next_url` discard it as already
                        // visited, silently dropping every internal link (#479).
                        if is_internal_link(&link, seed_domain)
                            && is_allowed(&link, &ctx.config)
                            && (ctx.ignore_robots
                                || ctx
                                    .robots_checker
                                    .is_robots_allowed(&link, link_domain)
                                    .await)
                        {
                            let new_discovered =
                                DiscoveredUrl::html(parsed_link, url_depth + 1, parent_url.clone());
                            ctx.queue
                                .push_prioritized(new_discovered, UrlSource::Link)
                                .await;
                        }
                    }
                }
            }
        },
        Err(e) => {
            log_scrape_error_with_cause(
                &e,
                url_str,
                "extract_links",
                Some(&ctx.correlation_id),
                "failed to extract links from page",
            );
            ctx.error_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ctx.error_breakdown[CrawlErrorCategory::Extraction.index()]
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    }
}
