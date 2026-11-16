//! Crawl task execution — per-page fetch, pipeline, and link extraction.
//!
//! Extracted from `engine.rs` (strangler fig, issue #439). Holds the free
//! functions spawned per discovered URL by `Engine::run()`:
//! `run_crawl_task` (the per-page worker) and `handle_crawl_result`
//! (task-completion bookkeeping). Both consume `Arc<CrawlTaskCtx>` and carry
//! no `Engine` state.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, instrument, warn};
use url::Url;

use super::checkpoint::BannedDomain;
use super::crawl_task_ctx::CrawlTaskCtx;
use super::ports::{waf_challenge_message, FetchOutcome};
use crate::application::pipeline::{ScrapedItem, StageOutcome};
use crate::application::rate_limiter::PacingContext;
use crate::application::url_filter::is_allowed;
use crate::domain::crawler_port::UrlSource;
use crate::domain::downloader_port::Cookie;
use crate::domain::session_port::SessionId;
use crate::domain::url_validation::is_internal_link;
use crate::domain::{CorrelationId, CrawlError, CrawlErrorCategory, DiscoveredUrl};
use crate::infrastructure::observability::{
    log_classified_error, log_scrape_error, log_scrape_error_with_cause,
};

/// Handle result from a completed crawl task
pub(crate) fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
) {
    match result {
        Ok(Ok(())) => {
            // Task completed successfully
        },
        Ok(Err(e)) => handle_task_error(e, error_count, error_breakdown),
        Err(e) => handle_join_error(e, error_count, error_breakdown),
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
) {
    if matches!(e, CrawlError::Cancelled) {
        debug!("Task cancelled by engine shutdown");
        return;
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

/// Execute a single crawl task using shared context.
///
/// Extracted from the inline async block in `Engine::run()` to reduce
/// the per-spawn clone surface from 18 individual `Arc::clone()` calls
/// to a single `Arc<CrawlTaskCtx>` clone.
pub(crate) async fn run_crawl_task(
    ctx: Arc<CrawlTaskCtx>,
    discovered_url: DiscoveredUrl,
) -> Result<(), CrawlError> {
    // Per-page identity minted BEFORE the pacing wait (#1610, OBS-H2). The
    // wait used to happen with no identity at all, so it belonged to no span
    // and could not be joined back to the page it delayed; minting here makes
    // the wait attributable AND lets the `crawl_page` span carry the wait it
    // absorbed (`rate_limit_wait_ms`).
    let page_correlation = ctx.correlation_id.child();
    let wait_ctx = PacingContext::bare("crawl_discovery")
        .with_url(discovered_url.url.as_str())
        .with_correlation(&page_correlation);
    let wait = match ctx
        .rate_limiter
        .until_ready_or_cancel_observed(&wait_ctx, &ctx.cancel_token)
        .await
    {
        Ok(waited) => waited,
        Err(_cancelled) => {
            debug!("Rate limit wait cancelled by engine shutdown");
            return Err(CrawlError::Cancelled);
        },
    };

    run_crawl_task_inner(ctx, discovered_url, page_correlation, wait).await
}

/// Inner implementation of [`run_crawl_task`] — carries the per-page
/// `crawl_page` span (#519).
///
/// The `#[instrument]` span declares the per-page identity (`correlation_id`,
/// `trace_id`) AT CREATION time (#501), and the outcome it can only know at
/// the end (`outcome`, `error_class`, `http_status`) is declared `Empty` and
/// filled in with `Span::record` — the FileTraceLayer merges late records into
/// the same snapshot (`on_record`, #1610). The instrumented span lifecycle is
/// async-safe: no `enter()` guard crosses an `.await` (#519).
#[instrument(
    name = "crawl_page",
    skip(ctx, page_correlation, discovered_url, rate_limit_wait_ms),
    fields(
        correlation_id = %page_correlation,
        trace_id = %page_correlation.trace_id(),
        url = %discovered_url.url,
        depth = discovered_url.depth,
        rate_limit_wait_ms = rate_limit_wait_ms.as_millis() as u64,
        outcome = tracing::field::Empty,
        http_status = tracing::field::Empty,
        error_class = tracing::field::Empty
    )
)]
async fn run_crawl_task_inner(
    ctx: Arc<CrawlTaskCtx>,
    discovered_url: DiscoveredUrl,
    page_correlation: CorrelationId,
    rate_limit_wait_ms: Duration,
) -> Result<(), CrawlError> {
    let url_str = discovered_url.url.as_str().to_string();
    let url_depth = discovered_url.depth;

    // Acquire a per-domain session when the pool is enabled. `Err(())` means
    // the pool had no available session for this domain — skip the URL.
    let session_id = match acquire_session(&ctx, &url_str) {
        Ok(id) => id,
        Err(()) => {
            record_page_outcome("session_unavailable", None, None);
            return Ok(());
        },
    };

    debug!("Crawling: {} (depth={})", url_str, url_depth);

    let parsed_url =
        url::Url::parse(&url_str).map_err(|e| CrawlError::Internal(format!("invalid URL: {e}")))?;

    let outcome = match fetch_page(&ctx, &parsed_url, &url_str, &page_correlation).await {
        Ok(outcome) => outcome,
        Err(e) => {
            record_page_outcome("error", None, Some(e.classify()));
            return Err(e);
        },
    };
    // The HTTP status travels on the SPAN too (#1610, OBS-P2-4): the outcome
    // was previously only derivable by joining separate events.
    tracing::Span::current().record("http_status", u64::from(outcome.status));

    // Crash-injection: fetch completed, nothing processed yet (batch path).
    crate::cli::crash_points::hit(crate::cli::crash_points::MID_FETCH);
    // The post-redirect URL is where the content actually lives. It drives
    // output keying, deduplication, relative-link resolution, and the recorded
    // result, so aliases collapse onto one document instead of producing
    // duplicate content (#651, Bug 3).
    let final_url = outcome.final_url.clone();
    let response = outcome.body;
    let fetched_cookies = outcome.cookies;

    ingest_cookies(&ctx, &fetched_cookies).await;
    report_session_success(&ctx, &url_str, session_id);

    ctx.pages_crawled
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    capture_content(&ctx, final_url.as_str(), &response);

    // Crash-injection: fetched + spooled; clean/validate/extraction not run.
    crate::cli::crash_points::hit(crate::cli::crash_points::POST_FETCH_PRE_EXTRACT);
    if !run_pipeline(&ctx, final_url.as_str(), &response, outcome.status).await {
        record_page_outcome("pipeline_skipped", None, None);
        return Ok(());
    }

    let mut result_url = discovered_url.clone();
    result_url.url = final_url.clone();
    if let Err(e) = ctx.collector.send_result(result_url).await {
        debug!("Failed to send result: {}", e);
    }

    extract_and_queue_links(&ctx, &response, final_url.as_str(), url_depth, &final_url).await;

    record_page_outcome("ok", Some(outcome.status), None);
    Ok(())
}

/// Record how a `crawl_page` ended on its own span (#1610, OBS-P2-4).
///
/// `#[instrument]` can only see arguments, so the outcome is declared `Empty`
/// and filled in here. Without it the span closed as a bare "it took N ms"
/// and every answer to "did this page work" had to be reassembled from
/// neighbouring events.
fn record_page_outcome(
    outcome: &'static str,
    http_status: Option<u16>,
    error_class: Option<crate::error::ErrorClass>,
) {
    let span = tracing::Span::current();
    span.record("outcome", outcome);
    if let Some(status) = http_status {
        span.record("http_status", u64::from(status));
    }
    if let Some(class) = error_class {
        // `display` wraps the Display impl as a tracing Value; the JSONL
        // spells the class exactly as the error events do. The value is passed
        // WITHOUT a borrow: `Span::record` is generic over `Q: Value`, and the
        // strict gate denies `needless_borrows_for_generic_args`.
        span.record("error_class", tracing::field::display(class));
    }
}

/// Acquire a per-domain session from the pool, if one is configured.
///
/// Returns `Ok(Some(id))` on acquisition, `Ok(None)` when no pool is enabled,
/// and `Err(())` when the pool exists but has no available session (the caller
/// should skip the URL).
fn acquire_session(ctx: &CrawlTaskCtx, url_str: &str) -> Result<Option<SessionId>, ()> {
    let Some(ref pool) = ctx.session_pool else {
        return Ok(None);
    };
    let domain = url::Url::parse(url_str)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default();
    match pool.acquire(&domain) {
        Some(id) => Ok(Some(id)),
        None => {
            debug!("Domain {} has no available sessions, skipping", domain);
            Err(())
        },
    }
}

/// Fetch the page, handling WAF challenges (banning the domain) and logging
/// fetch failures before propagating the error.
async fn fetch_page(
    ctx: &CrawlTaskCtx,
    parsed_url: &Url,
    url_str: &str,
    page_correlation: &CorrelationId,
) -> Result<FetchOutcome, CrawlError> {
    match ctx.fetcher.fetch_page(parsed_url, &ctx.config).await {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            if let Some(waf_msg) = waf_challenge_message(&e) {
                ban_waf_domain(ctx, parsed_url, &waf_msg);
                log_scrape_error(
                    &waf_msg,
                    url_str,
                    "fetch",
                    Some(page_correlation),
                    "WAF challenge detected",
                );
            } else {
                log_scrape_error_with_cause(
                    &e,
                    url_str,
                    "fetch",
                    Some(page_correlation),
                    "page fetch failed",
                );
            }
            Err(e)
        },
    }
}

/// Ban a domain that triggered a WAF challenge, unless it is already banned.
fn ban_waf_domain(ctx: &CrawlTaskCtx, parsed_url: &Url, waf_msg: &str) {
    let Some(domain) = parsed_url.host_str() else {
        return;
    };
    let banned = BannedDomain {
        domain: domain.to_string(),
        banned_until: None,
        reason: waf_msg.to_string(),
    };
    if let Ok(mut domains) = ctx.banned_domains.write() {
        if !domains.iter().any(|d| d.domain == domain) {
            domains.push(banned);
            warn!(domain = %domain, reason = %waf_msg, "domain banned due to WAF challenge");
        }
    }
}

/// Ingest any cookies returned by the fetch into the shared cookie bridge.
///
/// #1119: the bridge is a `tokio::sync::RwLock`, so the write lock is
/// awaited — a contended lock yields the worker instead of parking an
/// executor thread — and the guard is dropped at the end of the block,
/// never held across an `.await`.
async fn ingest_cookies(ctx: &CrawlTaskCtx, fetched_cookies: &[Cookie]) {
    if fetched_cookies.is_empty() {
        return;
    }
    let mut bridge = ctx.cookie_bridge.write().await;
    for cookie in fetched_cookies {
        bridge.add(cookie.clone());
    }
}

/// Report a successful fetch back to the session pool for the acquired session.
fn report_session_success(ctx: &CrawlTaskCtx, url_str: &str, session_id: Option<SessionId>) {
    let Some(ref pool) = ctx.session_pool else {
        return;
    };
    let Some(id) = session_id else {
        return;
    };
    if let Ok(parsed) = url::Url::parse(url_str) {
        if let Some(domain) = parsed.host_str() {
            pool.report_success(domain, id);
        }
    }
}

/// Hand the fetched body to the content sink when one is configured (#631).
///
/// Capture happens right after a successful fetch and before pipeline gating,
/// so a `Skip`/`Reject` outcome cannot silently drop the page from the export
/// set. The sink is synchronous and must not block.
fn capture_content(ctx: &CrawlTaskCtx, url_str: &str, response: &str) {
    let Some(ref sink) = ctx.content_sink else {
        return;
    };
    sink.capture(url_str, response);
}

/// Run the content pipeline if configured. Returns `true` when processing
/// should continue (no pipeline, or `Continue`); `false` when the item was
/// filtered, rejected, or failed (the caller should stop processing this page).
async fn run_pipeline(ctx: &CrawlTaskCtx, url_str: &str, response: &str, status_code: u16) -> bool {
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
async fn extract_and_queue_links(
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

#[cfg(all(test, not(miri)))]
#[path = "crawl_task_support_tests.rs"]
mod tests_support;

#[cfg(all(test, not(miri)))]
#[path = "crawl_task_fetch_tests.rs"]
mod tests_fetch;

#[cfg(all(test, not(miri)))]
#[path = "crawl_task_pipeline_tests.rs"]
mod tests_pipeline;

#[cfg(all(test, not(miri)))]
#[path = "crawl_task_errors_tests.rs"]
mod tests_errors;
