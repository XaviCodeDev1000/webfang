//! Crawl task execution — per-page fetch, pipeline, and link extraction.
//!
//! Extracted from `engine.rs` (strangler fig, issue #439). Holds the free
//! functions spawned per discovered URL by `Engine::run()`:
//! `run_crawl_task` (the per-page worker) and `handle_crawl_result`
//! (task-completion bookkeeping). Both consume `Arc<CrawlTaskCtx>` and carry
//! no `Engine` state.

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, instrument};

use super::crawl_task_ctx::CrawlTaskCtx;
use super::crawl_task_fetch::{
    acquire_session, capture_content, fetch_page, ingest_cookies, report_session_success,
};
use super::crawl_task_pipeline::{extract_and_queue_links, run_pipeline};
use crate::application::rate_limiter::PacingContext;
use crate::domain::{CorrelationId, CrawlError, DiscoveredUrl};

// The `#[path]` test modules glob `use super::*`; these names moved with the
// pipeline block to `crawl_task_pipeline.rs` (T7) but the tests still resolve
// them through here, so they stay importable under the same cfg as the mods.
#[cfg(all(test, not(miri)))]
use crate::application::pipeline::{ScrapedItem, StageOutcome};
#[cfg(all(test, not(miri)))]
use crate::domain::crawler_port::UrlSource;
#[cfg(all(test, not(miri)))]
use crate::domain::CrawlErrorCategory;

pub(crate) use super::crawl_task_errors::handle_crawl_result;

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
