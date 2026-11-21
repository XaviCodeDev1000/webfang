//! Batch scraping — bounded-concurrency fan-out over many URLs.
//!
//! Both public entries share one body (`scrape_multiple_inner`): the plain
//! variant and the paced variant (RC-1 slice 4, G2) differ only in whether
//! every fetch waits for a token from the shared bucket before its dial.

use super::scrape_with_config;
use super::types::{ScrapeBatchOutcome, ScrapeFailed, ScrapeOutcome};
use crate::application::http_client::HttpClientPort;
use crate::application::rate_limiter::{PacingContext, SharedRateLimiter};
use crate::domain::config::ScraperConfig;
use crate::domain::crawler_port::RobotsPort;
use crate::domain::CorrelationId;
use crate::error::Result;
use crate::infrastructure::observability::log_scrape_error;
use futures::stream::{self, StreamExt};
use tracing::{info, instrument};

/// Scrape multiple URLs with concurrency control
///
/// Uses `buffer_unordered` to limit concurrent requests, preventing:
/// - File descriptor exhaustion
/// - HDD thrashing (for systems with mechanical drives)
/// - Anti-bot detection (DDoS-like patterns)
///
/// Following **config-externalize**: Concurrency is configurable via ScraperConfig.
/// Following **async-concurrency-limit**: Uses buffer_unordered for concurrency control.
///
/// # Arguments
/// * `client` - HTTP client
/// * `urls` - URLs to scrape
/// * `config` - Scraper configuration
/// * `downloader` - Optional asset downloader
/// * `robots` - Optional robots.txt fetcher; when present (and `ignore_robots`
///   is false), disallowed URLs fail their per-URL check (#697)
/// * `ignore_robots` - Opt-out of robots.txt enforcement for this batch (#697)
///
/// # Returns
/// * `Vec<ScrapedContent>` - All successfully scraped content
///
/// # Note
/// Failed URLs are logged but don't stop the entire batch.
#[instrument(
    name = "scrape_multiple_with_limit",
    skip(client, urls, config, downloader, robots),
    fields(
        urls = urls.len(),
        concurrency = config.scraper_concurrency
    )
)]
pub async fn scrape_multiple_with_limit(
    client: &dyn HttpClientPort,
    urls: &[url::Url],
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    robots: Option<&dyn RobotsPort>,
    ignore_robots: bool,
) -> Result<ScrapeBatchOutcome> {
    scrape_multiple_inner(
        client,
        urls,
        config,
        downloader,
        robots,
        ignore_robots,
        None,
    )
    .await
}

/// Same as [`scrape_multiple_with_limit`] with optional pre-fetch pacing
/// (RC-1 slice 4, G2): when `pacing` is `Some`, every URL waits for a token
/// from the shared bucket BEFORE its fetch — guard-chain stage 2, the same
/// cadence policy the crawl engine and the CLI scrape phase use (no second
/// limiter implementation).
///
/// # Arguments
/// Additional to [`scrape_multiple_with_limit`]:
/// * `pacing` - Shared token bucket gating each fetch; `None` keeps the
///   unthrottled behavior (zero overhead).
#[instrument(
    name = "scrape_multiple_with_limit_paced",
    skip(client, urls, config, downloader, robots, pacing),
    fields(
        urls = urls.len(),
        concurrency = config.scraper_concurrency,
        paced = pacing.is_some()
    )
)]
pub async fn scrape_multiple_with_limit_paced(
    client: &dyn HttpClientPort,
    urls: &[url::Url],
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    robots: Option<&dyn RobotsPort>,
    ignore_robots: bool,
    pacing: Option<&SharedRateLimiter>,
) -> Result<ScrapeBatchOutcome> {
    scrape_multiple_inner(
        client,
        urls,
        config,
        downloader,
        robots,
        ignore_robots,
        pacing,
    )
    .await
}

/// Pacing scope of the batch path.
///
/// Exported so the tests that assert on the published pacing events bind to
/// the PRODUCTION value instead of mirroring the literal (#2214): a mirrored
/// string can drift, and a drifted scope makes the test fail for the wrong
/// reason.
pub const BATCH_SCOPE: &str = "batch_scrape";

/// Shared body of both batch entries: collect per-URL outcomes with bounded
/// concurrency, optionally gating every fetch behind the shared token bucket
/// BEFORE its dial (guard-chain stage 2, RC-1 slice 4).
#[allow(clippy::too_many_lines)]
async fn scrape_multiple_inner(
    client: &dyn HttpClientPort,
    urls: &[url::Url],
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    robots: Option<&dyn RobotsPort>,
    ignore_robots: bool,
    pacing: Option<&SharedRateLimiter>,
) -> Result<ScrapeBatchOutcome> {
    if urls.is_empty() {
        return Ok(ScrapeBatchOutcome {
            results: Vec::new(),
            failed: Vec::new(),
        });
    }

    // One run-root identity for the whole batch (#501): every
    // `scrape_with_config` call derives its own child from it, so the trace
    // is shared across the batch while each page span stays unique.
    let root_correlation = CorrelationId::new();
    info!(
        correlation_id = %root_correlation,
        trace_id = %root_correlation.trace_id(),
        "scrape_multiple identity"
    );

    info!(
        "🌐 Scraping {} URLs with concurrency limit {}",
        urls.len(),
        config.scraper_concurrency
    );

    // Pair each URL with its result so failures can report which URL failed
    // (ScrapeOutcome doesn't carry the source URL).
    let results: Vec<(url::Url, Result<ScrapeOutcome>)> = stream::iter(urls.to_vec())
        .map(|url| {
            let config = config.clone();
            let root = root_correlation.clone();
            async move {
                // G2 (RC-1 slice 4): pre-fetch pacing — the wait happens
                // BEFORE the network is touched, mirroring the crawl engine
                // and the CLI scrape phase (one shared cadence policy, no
                // second limiter implementation). #1610: measured and
                // emitted, so a slow batch can be told apart from a batch
                // that was being paced.
                if let Some(limiter) = pacing {
                    let wait_ctx = PacingContext::bare(BATCH_SCOPE).with_url(url.as_str());
                    limiter.until_ready_observed(&wait_ctx).await;
                }
                let result = scrape_with_config(
                    client,
                    &url,
                    &config,
                    downloader,
                    None,
                    None,
                    robots,
                    ignore_robots,
                    &root,
                )
                .await;
                (url, result)
            }
        })
        .buffer_unordered(config.scraper_concurrency)
        .collect()
        .await;

    let outcome = collect_batch_outcome(results, &root_correlation);

    info!(
        "✅ Scraped {} pages from {} URLs ({} failed)",
        outcome.results.len(),
        urls.len(),
        outcome.failed.len()
    );
    Ok(outcome)
}

/// Split collected per-URL outcomes into successes and #591 failure records.
///
/// `root_correlation` is the batch run-root identity (#501): each failed URL
/// emits its operational error event through `log_scrape_error` carrying that
/// correlation, matching the page/crawl failure contract (issue #1604).
pub(super) fn collect_batch_outcome(
    results: Vec<(url::Url, Result<ScrapeOutcome>)>,
    root_correlation: &CorrelationId,
) -> ScrapeBatchOutcome {
    let mut all_content = Vec::new();
    let mut failed = Vec::new();
    for (url, result) in results {
        match result {
            Ok(outcome) => all_content.extend(outcome.results),
            Err(e) => {
                let url_str = url.to_string();
                log_scrape_error(
                    &e,
                    &url_str,
                    "scrape",
                    Some(root_correlation),
                    "batch URL scrape failed",
                );
                failed.push(ScrapeFailed {
                    url: url_str,
                    error: e.to_string(),
                    category: e.classify().into(),
                });
            },
        }
    }
    ScrapeBatchOutcome {
        results: all_content,
        failed,
    }
}
