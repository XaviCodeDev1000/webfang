//! Scraper service — Main orchestration use case
//!
//! This module coordinates the scraping workflow:
//! 1. Fetch HTML via HTTP client
//! 2. Extract content using Readability or fallback
//! 3. Download assets if configured
//! 4. Return structured ScrapedContent
//!
//! # Rules Applied
//!
//! - **config-externalize**: Concurrency is configurable via ScraperConfig
//! - **async-concurrency-limit**: Uses buffer_unordered for concurrency control

mod batch;
pub(crate) mod page_content;
pub mod types;

pub use batch::{scrape_multiple_with_limit, scrape_multiple_with_limit_paced, BATCH_SCOPE};
pub use page_content::extract_original_title;
pub use types::{
    ScrapeBatchOutcome, ScrapeErrorCategory, ScrapeFailed, ScrapeOutcome,
    MAX_INSTRUMENTED_BODY_SIZE,
};

use crate::application::adaptive_engine::AdaptiveSelectorEngine;
#[cfg(feature = "adaptive-selectors")]
use crate::application::extraction::adaptive_selector_repair;

// Re-exports preserve the historical `scraper_service::*` public paths after
// the #443 decomposition into focused application modules. Callers (MCP
// handlers, `crawler::discovery`, integration tests) keep resolving unchanged.
pub use crate::application::asset_download::{
    download_asset_urls, download_assets_if_enabled, extract_asset_urls,
};
pub use crate::application::extraction::{extract_with_selector, scrape_with_readability};
pub use crate::application::spa_detection::{
    detect_spa_content, SpaDetectionResult, MIN_CONTENT_CHARS,
};

use crate::application::http_client::HttpClientPort;
use crate::domain::config::ScraperConfig;
use crate::domain::crawler_port::{RobotsDecision, RobotsPort};
use crate::domain::{CorrelationId, DomInspectorPort};
use crate::error::{Result, ScraperError};
use tracing::{info, instrument};

/// Scrape a URL with asset downloading configuration
///
/// Correlation contract (#501): the caller owns the run-root identity
/// (`root_correlation`); this use case derives one page child from it
/// (same `trace_id`, fresh `span_id`) so every page of the operation stays
/// reconstructable under a single trace while each page is distinguishable.
///
/// # Arguments
/// * `client` - HTTP client
/// * `url` - URL to scrape
/// * `config` - Scraper configuration with download options
/// * `downloader` - Optional asset downloader
/// * `inspector` - Optional DOM inspector for selector diagnostics (None for non-MCP paths)
/// * `robots` - Optional robots.txt fetcher; when present (and `ignore_robots`
///   is false), disallowed URLs are rejected BEFORE any page fetch (#697)
/// * `ignore_robots` - Opt-out of robots.txt enforcement for this call (#697)
/// * `root_correlation` - Run-root correlation identity of the enclosing operation
///
/// # Returns
/// * `ScrapeOutcome` - Scraped content results + CSS selector extraction result
///
/// # Errors
/// Returns `ScraperError::Http` for HTTP errors, `ScraperError::Network` for
/// connection errors, and `ScraperError::WafBlocked` (provider `"robots.txt"`)
/// when the URL is disallowed by robots.txt (#697 audit decision — the variant
/// is reused because a dedicated robots variant does not exist).
// The parameter list is this use case's full dependency set: the same
// justification `scrape_urls` carries (#705) — bundling them into a struct
// would only move the identical wiring one level up.
#[allow(clippy::too_many_arguments)]
pub async fn scrape_with_config(
    client: &dyn HttpClientPort,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    inspector: Option<&dyn DomInspectorPort>,
    engine: Option<&AdaptiveSelectorEngine>,
    robots: Option<&dyn RobotsPort>,
    ignore_robots: bool,
    root_correlation: &CorrelationId,
) -> Result<ScrapeOutcome> {
    // Robots.txt gate runs in the OUTER wrapper, before the span and before
    // any fetch, so a disallowed URL never touches the network (#697) and the
    // inner instrumented signature stays untouched.
    enforce_robots_policy(url, robots, ignore_robots).await?;

    // Per-page identity derived from the run root BEFORE the span so the span
    // can declare it (`#[instrument]` only sees function parameters — #501).
    let page_correlation = root_correlation.child();
    scrape_with_config_inner(
        client,
        url,
        config,
        downloader,
        inspector,
        engine,
        page_correlation,
    )
    .await
}

/// Enforce the robots.txt policy for one URL before any page fetch (#697).
///
/// Single source of truth for the robots decision — shared by the crawl path
/// ([`scrape_with_config`]) and the MCP direct-fetch tools via
/// `McpState::robots_denied_for` (#749): `ignore_robots` opts out, a `None`
/// fetcher means "no enforcement available" (fail-open), and a denial emits
/// `tracing::warn!("robots_txt_denied")` and returns
/// [`ScraperError::waf_blocked`] with provider `"robots.txt"` — the WafBlocked
/// variant is deliberately reused for robots.txt denials per the #705 audit
/// decision (`ScraperError` has no dedicated robots variant).
///
/// A policy refusal is NOT a robots denial (#1301): when the SSRF literal-IP
/// entry guard rejects the URL, no socket ever opens and robots.txt was never
/// consulted — reporting that as `WafBlocked(url, "robots.txt")` invents a
/// WAF that does not exist. Since #1329 the port reports the typed verdict
/// [`RobotsDecision::PolicyRefused`] carrying the guard's own cause, so this
/// gate does NOT re-validate the URL (single guard call site in the chain):
/// the refusal surfaces as [`ScraperError::Network`] with the guard's cause
/// ("SSRF detectado: ..."), so connect-family failures keep their real
/// cause. Genuine rules denials below keep the WAF label.
///
/// [`RobotsPort::is_allowed`] is FAIL-OPEN: if the robots.txt fetch itself
/// fails (network error, non-2xx, timeout), the verdict is
/// [`RobotsDecision::Allowed`] — matching the production crawl behavior.
pub async fn enforce_robots_policy(
    url: &url::Url,
    robots: Option<&dyn RobotsPort>,
    ignore_robots: bool,
) -> Result<()> {
    if ignore_robots {
        return Ok(());
    }
    let Some(fetcher) = robots else {
        return Ok(());
    };

    let domain = url.host_str().unwrap_or("unknown");
    // #1301 + #1329: the typed verdict distinguishes a genuine robots-rules
    // denial from a policy refusal WITHOUT re-validating the URL — the
    // literal-IP guard runs exactly once, inside the fetcher. Re-running
    // `reject_forbidden_literal_url` here would duplicate the guard call
    // site this port shape exists to eliminate.
    match fetcher.is_allowed(url.as_str(), domain).await {
        RobotsDecision::Allowed => {},
        RobotsDecision::RulesDenied => {
            tracing::warn!(url = %url, domain = %domain, "robots_txt_denied");
            return Err(ScraperError::waf_blocked(url.to_string(), "robots.txt"));
        },
        RobotsDecision::PolicyRefused(forbidden) => {
            // The guard's real cause, verbatim — never a phantom WAF label.
            return Err(ScraperError::Network(Box::new(forbidden)));
        },
    }
    Ok(())
}

/// Instrumented span of the scrape use case (#1610, OBS-P2-4).
///
/// `#[instrument]` can only see arguments, so what the caller actually needs
/// — did this page produce content, and if not, WHICH class of failure — is
/// declared `Empty` here and recorded on the span just before it closes.
/// Without it the span closed as a bare "the fetch took N ms" and every
/// answer to "did this page work" had to be reassembled from neighbouring
/// events. The wrapper exists so the recording happens INSIDE the span
/// without rewriting the many `?` early returns of [`scrape_page`] into
/// explicit matches: control flow and errors are untouched.
#[instrument(
    name = "scrape_with_config",
    skip(client, config, downloader, inspector, engine, correlation),
    fields(
        url = %url,
        correlation_id = %correlation,
        trace_id = %correlation.trace_id(),
        has_downloads = config.has_downloads(),
        outcome = tracing::field::Empty,
        error_class = tracing::field::Empty,
        results = tracing::field::Empty
    )
)]
async fn scrape_with_config_inner(
    client: &dyn HttpClientPort,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    inspector: Option<&dyn DomInspectorPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
    correlation: CorrelationId,
) -> Result<ScrapeOutcome> {
    let scraped = scrape_page(
        client,
        url,
        config,
        downloader,
        inspector,
        engine,
        correlation,
    )
    .await;

    let span = tracing::Span::current();
    match &scraped {
        Ok(outcome) => {
            span.record("outcome", "ok");
            span.record("results", outcome.results.len() as u64);
        },
        Err(e) => {
            span.record("outcome", "error");
            // `display` wraps the Display impl as a tracing Value, so the
            // JSONL spells the class exactly as the error events do.
            span.record("error_class", tracing::field::display(e.classify()));
        },
    }

    scraped
}

/// Body of the scrape use case — the span lives in
/// [`scrape_with_config_inner`], which wraps it to record the outcome.
use page_content::{
    build_scraped_content, clean_html_for_scrape, detect_waf, extract_page_extractions, fetch_html,
    log_html_size,
};

async fn scrape_page(
    client: &dyn HttpClientPort,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    inspector: Option<&dyn DomInspectorPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
    correlation: CorrelationId,
) -> Result<ScrapeOutcome> {
    let mut results = Vec::new();

    info!("🌐 Fetching: {}", url);

    let response = fetch_html(client, url, &correlation).await?;
    // Crash-injection: response body fully received, nothing processed.
    crate::cli::crash_points::hit(crate::cli::crash_points::MID_FETCH);

    let html: &str = &response.body;
    log_html_size(html, url);

    detect_waf(html, &response, config, url, &correlation)?;

    // M7 FIX: Log selector feedback when --selector is active
    if config.selector != "body" {
        info!(
            target: "scraper",
            selector = %config.selector,
            "Aplicando selector CSS manual"
        );
    }

    // Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
    // Readability. This helps legible find the main content without being
    // confused by navigation elements, JavaScript bundles, and CSS.
    let cleaned_html = clean_html_for_scrape(html, config);
    // Crash-injection: fetched + cleaned, extraction not yet run.
    crate::cli::crash_points::hit(crate::cli::crash_points::POST_FETCH_PRE_EXTRACT);

    // Apply CSS selector extraction if a non-default selector is configured.
    let extract_result = extract_with_selector(&cleaned_html, &config.selector, url, inspector);

    // Adaptive selector repair: delegate to the canonical shared helper (#442),
    // threading this use case's inspector through for diagnostics.
    // Returns (ExtractResult, Option<CascadeTrace>) for structural scoring (#792).
    #[cfg(feature = "adaptive-selectors")]
    let (extract_result, adaptive_trace) = adaptive_selector_repair(
        extract_result,
        engine,
        &config.selector,
        url,
        url.host_str(),
        inspector,
    )
    .await;

    // Compute structural quality hint if adaptive repair was attempted (#792).
    #[cfg(feature = "adaptive-selectors")]
    let quality_hint = adaptive_trace.and_then(|trace| {
        crate::application::structural_score::compute_quality_hint(&trace, &extract_result)
    });
    #[cfg(not(feature = "adaptive-selectors"))]
    let quality_hint = None;

    let extraction_html = extract_result.as_html().to_owned();

    // #962: single DOM parse per page. Every shared-document operation
    // (title, Readability, author, assets) runs synchronously inside
    // [`extract_page_extractions`]: `scraper::Html` is neither `Send` nor
    // `Sync`, so confining all DOM access to a non-async function guarantees
    // this scrape future never captures the document across an `.await`.
    let page = extract_page_extractions(html, &extract_result, url, config);

    // Try Readability first, fallback to plain text extraction
    let content = build_scraped_content(
        page.content,
        page.author,
        html,
        &extraction_html,
        &page.asset_urls,
        url,
        config,
        downloader,
        &page.original_title,
        &correlation,
        quality_hint,
    )
    .await?;
    results.push(content);

    info!(
        "✅ Extracted: {} ({} chars, {} assets)",
        results
            .first()
            .map(|r| r.title.as_str())
            .unwrap_or("unknown"),
        results.first().map(|r| r.content.len()).unwrap_or(0),
        results.first().map(|r| r.assets.len()).unwrap_or(0)
    );

    Ok(ScrapeOutcome {
        results,
        extract_result,
    })
}

#[cfg(test)]
#[path = "scraper_service_tests.rs"]
mod tests;
