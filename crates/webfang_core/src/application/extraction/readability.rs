//! Readability-based content extraction and the thin convenience wrappers
//! (`scrape_with_readability`, `extract_content`) plus the adaptive-selector
//! repair hook and the DOM pruner toggle.

use super::selector::{extract_with_selector, selector_supplied_fragment};
use crate::application::adaptive_engine::AdaptiveSelectorEngine;
use crate::application::http_client::HttpClientPort;
use crate::application::scraper_service::scrape_with_config;
use crate::domain::config::ScraperConfig;
use crate::domain::html_cleaner::clean_html;
use crate::domain::html_extraction::{
    author_extractor, date_extractor, dom_pruner, fallback, readability,
};
use crate::domain::{CorrelationId, ScrapedContent, ValidUrl};
#[cfg(feature = "adaptive-selectors")]
use crate::domain::{DomInspectorPort, ExtractResult};
use crate::error::{Result, ScraperError};
use crate::infrastructure::observability::log_scrape_error;
use tracing::{debug, warn};

/// Scrape a URL using Readability algorithm for clean content extraction
///
/// This is the 2026 best practice approach — uses the same algorithm as
/// Firefox Reader View to extract only meaningful content.
///
/// # Examples
///
/// ```no_run
/// use webfang_core::application::{create_http_client, scrape_with_readability};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let client = create_http_client()?;
/// let url = url::Url::parse("https://example.com")?;
/// let results = scrape_with_readability(&client, &url).await?;
/// # Ok(())
/// # }
/// ```
pub async fn scrape_with_readability(
    client: &dyn HttpClientPort,
    url: &url::Url,
) -> Result<Vec<ScrapedContent>> {
    // Standalone convenience entry: this call IS the operation, so it mints
    // its own run-root identity (#501). No robots fetcher is available at
    // this standalone entry point (#697): enforcement is the caller's
    // concern (the MCP/CLI paths wire their own robots handling).
    let root_correlation = CorrelationId::new();
    let outcome = scrape_with_config(
        client,
        url,
        &ScraperConfig::default(),
        None,
        None,
        None,
        None,
        false,
        &root_correlation,
    )
    .await?;
    Ok(outcome.results)
}

/// Canonical adaptive selector repair (Tier 1 lexical cascade).
///
/// Single home for the "extraction fell back → ask the adaptive engine for a
/// repaired selector → re-extract" use case, shared by [`extract_content`] and
/// `scrape_with_config` (issue #442 — previously duplicated in both). When the
/// engine yields a selector that matches, the repaired [`ExtractResult`] is
/// returned; otherwise the original fallback is preserved unchanged.
///
/// Returns a tuple of (ExtractResult, Option<CascadeTrace>) where the trace
/// is populated when the adaptive engine attempted a repair. The trace
/// enables structural scoring for honest error hints (#792).
///
/// `inspector` is threaded through to [`extract_with_selector`] for diagnostics
/// (`scrape_with_config` passes its inspector; [`extract_content`] passes
/// `None`).
/// Canonical adaptive selector repair (Tier 1 lexical cascade).
///
/// Single home for the "extraction fell back → ask the adaptive engine for a
/// repaired selector → re-extract" use case, shared by [`extract_content`] and
/// `scrape_with_config` (issue #442 — previously duplicated in both). When the
/// engine yields a selector that matches, the repaired [`ExtractResult`] is
/// returned; otherwise the original fallback is preserved unchanged.
///
/// Returns a tuple of (ExtractResult, Option<CascadeTrace>) where the trace
/// is populated when the adaptive engine attempted a repair. The trace
/// enables structural scoring for honest error hints (#792).
///
/// `inspector` is threaded through to [`extract_with_selector`] for diagnostics
/// (`scrape_with_config` passes its inspector; [`extract_content`] passes
/// `None`).
#[cfg(feature = "adaptive-selectors")]
pub(crate) async fn adaptive_selector_repair(
    extract_result: ExtractResult,
    engine: Option<&AdaptiveSelectorEngine>,
    selector: &str,
    base_url: &url::Url,
    host: Option<&str>,
    inspector: Option<&dyn DomInspectorPort>,
) -> (
    ExtractResult,
    Option<crate::application::adaptive_engine::CascadeTrace>,
) {
    if let ExtractResult::Fallback { html, diagnostic } = extract_result {
        if let Some(engine) = engine {
            match engine
                .select_sync_aware(
                    html.clone(),
                    selector.to_owned(),
                    host.map(|s| s.to_owned()),
                )
                .await
            {
                Ok(outcome) => {
                    let trace = outcome.trace.clone();
                    let repaired = extract_with_selector(
                        &html,
                        &outcome.suggestion.selector,
                        base_url,
                        inspector,
                    );
                    if repaired.is_matched() {
                        tracing::info!(
                            repaired_selector = %outcome.suggestion.selector,
                            method = ?outcome.status,
                            "adaptive_repair_resolved"
                        );
                        (repaired, trace)
                    } else {
                        (ExtractResult::Fallback { html, diagnostic }, trace)
                    }
                },
                Err(_) => (ExtractResult::Fallback { html, diagnostic }, None),
            }
        } else {
            (ExtractResult::Fallback { html, diagnostic }, None)
        }
    } else {
        (extract_result, None)
    }
}

/// Content-extraction pipeline: clean → selector → adaptive → readability/fallback.
///
/// Recibe HTML ya fetchado y validado (post-WAF). No conoce el transporte.
///
/// Correlation identity (#501): the per-page identity is a REQUIRED input —
/// callers own it (e.g. `scrape_single_url` declares it on its trace
/// span) and inject it here so the exported content shares the same identity
/// as the page's `span_fields` in the `--trace-file` JSONL. Standalone
/// callers mint their own root at entry. Identity enters through the type
/// system or not at all — there is no ad-hoc fallback.
///
/// # Errors
///
/// Returns [`ScraperError::ExtractionFailed`] when fallback content is below
/// `MIN_FALLBACK_CONTENT` bytes.
/// DOM pre-pruning (#791): remove invisible/empty elements BEFORE
/// clean_html. ORDER MATTERS: html_cleaner strips non-preserved attributes
/// — including `style` — so the pruner must see raw HTML while the
/// display:none / visibility:hidden signals are still intact.
/// Content-extraction pipeline: clean → selector → adaptive → readability/fallback.
///
/// Recibe HTML ya fetchado y validado (post-WAF). No conoce el transporte.
///
/// Correlation identity (#501): the per-page identity is a REQUIRED input —
/// callers own it (e.g. `scrape_single_url` declares it on its trace
/// span) and inject it here so the exported content shares the same identity
/// as the page's `span_fields` in the `--trace-file` JSONL. Standalone
/// callers mint their own root at entry. Identity enters through the type
/// system or not at all — there is no ad-hoc fallback.
///
/// # Errors
///
/// Returns [`ScraperError::ExtractionFailed`] when fallback content is below
/// `MIN_FALLBACK_CONTENT` bytes.
/// DOM pre-pruning (#791): remove invisible/empty elements BEFORE
/// clean_html. ORDER MATTERS: html_cleaner strips non-preserved attributes
/// — including `style` — so the pruner must see raw HTML while the
/// display:none / visibility:hidden signals are still intact.
pub(crate) fn prune_dom_if_enabled(html: &str, config: &ScraperConfig) -> String {
    if config.dom_preprune {
        let (pruned, reduction_ratio) = dom_pruner::prune_dom(html);
        debug!(
            original = html.len(),
            pruned = pruned.len(),
            reduction_pct = (reduction_ratio * 100.0).round(),
            "DOM pre-pruned invisible/empty elements"
        );
        pruned
    } else {
        html.to_string()
    }
}

/// Extract scraped content from raw HTML: DOM pre-pruning (#791) →
/// boilerplate clean → CSS selector → Readability, with plain-text fallback.
/// Extract scraped content from raw HTML: DOM pre-pruning (#791) →
/// boilerplate clean → CSS selector → Readability, with plain-text fallback.
#[allow(clippy::too_many_lines)]
pub async fn extract_content(
    html: &str,
    url: &url::Url,
    config: &ScraperConfig,
    asset_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
    correlation_id: &CorrelationId,
) -> Result<ScrapedContent> {
    // DOM pre-pruning (#791): invisible/empty elements removed before
    // clean_html strips the `style` attributes that identify them.
    let pruned_html = prune_dom_if_enabled(html, config);

    // Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
    // Readability. This helps legible find the main content without being
    // confused by navigation elements, JavaScript bundles, and CSS.
    let cleaned_html = clean_html(&pruned_html);

    // Apply CSS selector extraction if a non-default selector is configured.
    let extract_result = extract_with_selector(&cleaned_html, &config.selector, url, None);
    // Adaptive selector repair (Tier 1 lexical): delegate to the canonical
    // shared helper (#442). Returns (ExtractResult, Option<CascadeTrace>).
    // The trace enables structural scoring for honest error hints (#792).
    #[cfg(feature = "adaptive-selectors")]
    let (extract_result, adaptive_trace) = adaptive_selector_repair(
        extract_result,
        engine,
        &config.selector,
        url,
        url.host_str(),
        None,
    )
    .await;

    // Compute structural quality hint if adaptive repair was attempted (#792).
    #[cfg(feature = "adaptive-selectors")]
    let quality_hint = adaptive_trace.and_then(|trace| {
        crate::application::structural_score::compute_quality_hint(&trace, &extract_result)
    });
    #[cfg(not(feature = "adaptive-selectors"))]
    let quality_hint = None;

    // Crash-injection: inside extraction — cleaned HTML has been through the
    // CSS selector step; content assembly not yet complete.
    crate::cli::crash_points::hit(crate::cli::crash_points::MID_EXTRACTION);

    // #2534: an explicit selector already chose the content. Readability is a
    // heuristic for FINDING main content in a full document; feeding it the
    // user's own choice re-decides it and drags the output back onto legible's
    // heuristics (which is why every new selector needed a new snapshot).
    if selector_supplied_fragment(&config.selector, &extract_result) {
        let fragment = extract_result.as_html().to_owned();
        let assets = crate::application::asset_download::download_assets_if_enabled(
            html,
            url,
            config,
            asset_downloader,
        )
        .await?;
        let document = scraper::Html::parse_document(html);
        let original_title = crate::application::scraper_service::extract_original_title(&document);
        // Read from the RAW document: clean_html strips <script>, so JSON-LD
        // never survives the cleaned HTML.
        let date = date_extractor::extract_date_from_document(&document);
        // No byline: the fragment is not an article, so the author comes from
        // the document metadata (JSON-LD, meta tags, microdata).
        let author = author_extractor::extract_author(html, None);

        return Ok(ScrapedContent {
            title: crate::application::resolve_title(&original_title, url),
            // Derived from the fragment so JSON/JSONL keep block boundaries.
            content: fallback::extract_text(&fragment),
            url: ValidUrl::try_from_url(url.clone())?,
            // The user asked for elements, not an article: no excerpt.
            excerpt: None,
            author,
            // legible supplied published_time and this path skips legible, so
            // the date comes from document metadata instead (#2539).
            date,
            html: Some(fragment),
            assets,
            correlation_id: Some(correlation_id.clone()),
            quality_hint,
        });
    }

    let extraction_html = extract_result.as_html().to_owned();

    // Try Readability first, fallback to plain text extraction
    match readability::parse(&extraction_html, Some(url.as_str())) {
        Ok(article) => {
            let assets = crate::application::asset_download::download_assets_if_enabled(
                html,
                url,
                config,
                asset_downloader,
            )
            .await?;

            // Shared minimum-content guard (#706): on the SUCCESS branch there
            // was no content-size check before, so JS-shell pages returned Ok
            // near-empty. The fallback branch keeps MIN_FALLBACK_CONTENT=100 as
            // its SOLE authority — no second guard there (no double-error).
            crate::application::spa_detection::validate_min_content(
                url.as_str(),
                &article.text_content,
                html,
                correlation_id,
            )?;

            let author = author_extractor::extract_author(html, article.byline.as_deref());

            Ok(ScrapedContent {
                title: crate::application::resolve_title(&article.title, url),
                content: article.text_content,
                url: ValidUrl::try_from_url(url.clone())?,
                excerpt: article.excerpt.as_deref().map(|e| {
                    crate::domain::excerpt_repair::repair_empty_byline(e, author.as_deref())
                }),
                author,
                // From the RAW document: clean_html strips <script> and every
                // metadata attribute, so legible's `published_time` is always
                // None here and this cascade is the only source (#2563).
                date: date_extractor::extract_date(html),
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                html: Some(article.content),
                assets,
                correlation_id: Some(correlation_id.clone()),
                quality_hint,
            })
        },
        Err(e) => {
            warn!(
                url = %url,
                error = %e,
                "readability extraction failed; using text-extraction fallback"
            );
            let fallback_content = fallback::extract_text(&extraction_html);

            // Check if fallback produced poor content (likely extraction failure)
            const MIN_FALLBACK_CONTENT: usize = 100;
            if fallback_content.len() < MIN_FALLBACK_CONTENT {
                let msg = format!(
                    "poor fallback content: {} bytes (minimum {} bytes). Readability: {}",
                    fallback_content.len(),
                    MIN_FALLBACK_CONTENT,
                    e
                );
                log_scrape_error(
                    &msg,
                    url.as_str(),
                    "extract",
                    Some(correlation_id),
                    "content extraction failed",
                );
                return Err(ScraperError::ExtractionFailed {
                    url: url.to_string(),
                    reason: msg,
                });
            }

            let assets = crate::application::asset_download::download_assets_if_enabled(
                html,
                url,
                config,
                asset_downloader,
            )
            .await?;

            Ok(ScrapedContent {
                // #2552: `resolve_title` is the single owner of the empty-title
                // fallback; pre-substituting a bare host here made this branch
                // disagree with the other two.
                title: crate::application::resolve_title("", url),
                content: fallback_content,
                url: ValidUrl::try_from_url(url.clone())?,
                excerpt: None,
                author: None,
                date: None,
                html: Some(html.to_owned()),
                assets,
                correlation_id: Some(correlation_id.clone()),
                quality_hint,
            })
        },
    }
}
