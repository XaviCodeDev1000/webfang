//! Per-page extraction products and the `ScrapedContent` assembly.
//!
//! #962: one DOM parse per page — every shared-document operation (title,
//! Readability, author, assets) runs synchronously inside
//! `extract_page_extractions`, so the async scrape pipeline only ever holds
//! owned, `Send` values (`scraper::Html` is neither `Send` nor `Sync`).

use super::types::MAX_INSTRUMENTED_BODY_SIZE;
use crate::application::asset_download::{download_asset_urls, extract_asset_urls};
use crate::application::error_mapping::scraper_error_from_http;
use crate::application::http_client::HttpClientPort;
use crate::domain::config::ScraperConfig;
use crate::domain::html_cleaner::clean_html;
use crate::domain::html_extraction::{author_extractor, fallback, readability};
use crate::domain::http_port::HttpResponse;
use crate::domain::waf::{waf_inspector, InspectionContext};
use crate::domain::{CorrelationId, ScrapedContent, ValidUrl};
use crate::error::{Result, ScraperError};
use crate::infrastructure::observability::{log_scrape_error, log_scrape_error_with_cause};
use tracing::{debug, warn};

/// Extract the `<title>` from the original DOM before any transformation.
///
/// 'title' is a compile-time-constant selector; `Selector::parse` cannot fail.
pub fn extract_original_title(document: &scraper::Html) -> String {
    #[allow(clippy::expect_used)]
    let selector = scraper::Selector::parse("title")
        // LCOV_EXCL_LINE defensive: compile-time-selector — 'title' is a constant valid selector
        .expect("invariant: 'title' is a valid CSS selector — this cannot fail");
    document
        .select(&selector)
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default()
}

/// Every shared-document extraction product for one fetched page (#962).
///
/// Produced synchronously by [`extract_page_extractions`]; the async scrape
/// pipeline consumes only these owned, `Send` values — never the document.
pub(super) struct PageExtractions {
    /// `<title>` of the original DOM (H1 FIX), empty when absent.
    pub(super) original_title: String,
    /// What the extraction stage produced: the user's fragment or Readability.
    pub(super) content: PageContent,
    /// Author resolved against the shared DOM and the readability byline.
    pub(super) author: Option<String>,
    /// Deduplicated asset URLs extracted from the shared DOM — validated
    /// `ValidUrl` values: the port downstream only accepts the typestate
    /// (#1117).
    pub(super) asset_urls: Vec<crate::domain::ValidUrl>,
}

/// What the extraction stage produced for one page (#2534).
///
/// An explicit selector that matched means Readability never runs: the
/// fragment is already the content the user asked for.
pub(super) enum PageContent {
    /// `--selector` matched — the fragment IS the content.
    Selector(String),
    /// No explicit selector, or it fell back — Readability over the page.
    Readability(crate::error::Result<readability::Article>),
}

/// Parse `html` once (#962) and run every document-dependent stage over the
/// single parsed DOM: original-title extraction, Readability, author
/// extraction and asset URL extraction.
///
/// Deliberately non-async: [`scraper::Html`] contains interior mutability
/// (neither `Send` nor `Sync`), so all DOM access is confined to this
/// synchronous function and dies with its scope — the enclosing scrape future
/// only ever holds the owned [`PageExtractions`] values.
pub(super) fn extract_page_extractions(
    html: &str,
    extract_result: &crate::domain::dom_inspector::ExtractResult,
    url: &url::Url,
    config: &ScraperConfig,
) -> PageExtractions {
    let document = scraper::Html::parse_document(html);
    tracing::debug!(url = %url, "parsed page document once for all extraction stages");

    // H1 FIX: Extract title from original DOM BEFORE any transformation.
    // This preserves the <title> tag even when --selector filters it out.
    let original_title = extract_original_title(&document);

    // #2534: an explicit selector already chose the content, so Readability is
    // skipped entirely for it. Without a selector this is unchanged.
    let content = if crate::application::extraction::selector_supplied_fragment(
        &config.selector,
        extract_result,
    ) {
        PageContent::Selector(extract_result.as_html().to_owned())
    } else {
        PageContent::Readability(readability::parse(
            extract_result.as_html(),
            Some(url.as_str()),
        ))
    };

    let author = match &content {
        PageContent::Readability(Ok(article)) => {
            author_extractor::extract_author_from_document(&document, article.byline.as_deref())
        },
        PageContent::Readability(Err(_)) => None,
        // No byline to fall back to: metadata only.
        PageContent::Selector(_) => author_extractor::extract_author_from_document(&document, None),
    };

    let asset_urls = extract_asset_urls(&document, url, config);

    PageExtractions {
        original_title,
        content,
        author,
        asset_urls,
    }
}

/// Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
/// Readability, logging the reduction.
/// Applies DOM pre-pruning (#791) when enabled.
pub(super) fn clean_html_for_scrape(html: &str, config: &ScraperConfig) -> String {
    let pruned_html = crate::application::extraction::prune_dom_if_enabled(html, config);
    let cleaned_html = clean_html(&pruned_html);

    debug!(
        "🧹 Cleaned HTML: {} → {} bytes ({}:0.2% reduction)",
        html.len(),
        cleaned_html.len(),
        ((html.len() - cleaned_html.len()) as f64 / html.len() as f64 * 100.0).round()
    );
    cleaned_html
}

/// Build a [`ScrapedContent`] from the fetched HTML, trying Readability first
/// and falling back to plain-text extraction.
///
/// #962: every shared-document operation (Readability parse, author
/// extraction, asset URL extraction) completes synchronously in
/// [`scrape_page`] — `scraper::Html` is neither `Send` nor `Sync`, and
/// async-fn parameters are conservatively live across `.await` points, so
/// this future only receives owned / `Send` data.
/// Prefer the title from the original DOM, fall back to what the extraction
/// produced. The empty check lives here once instead of being repeated at every
/// call site — three copies of the same `if` is what pushed this module over the
/// duplication ratchet.
pub(super) fn preferred_title<'a>(original_title: &'a str, fallback: &'a str) -> &'a str {
    if original_title.is_empty() {
        fallback
    } else {
        original_title
    }
}

/// #2534: the selector fragment is the content. No Readability, no excerpt
/// (the user asked for elements, not an article) and no published time
/// (legible was its only source — tracked as a follow-up).
///
/// No minimum-content guard either: the guard exists to catch JS shells that
/// rendered nothing, and an explicit selector is not a heuristic about whether
/// the page rendered. A short heading is a legitimate answer to `--selector h3`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn selector_scraped_content(
    fragment: String,
    author: Option<String>,
    asset_urls: &[crate::domain::ValidUrl],
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    html: &str,
    original_title: &str,
    correlation: &CorrelationId,
    quality_hint: Option<crate::domain::extraction_quality::ExtractionQualityHint>,
) -> Result<ScrapedContent> {
    let assets = download_asset_urls(asset_urls, config, downloader).await?;

    // #2552: no host pre-emption — `resolve_title` owns the fallback, so a
    // title-less page reads `Document: [host]` on every route.
    Ok(ScrapedContent {
        title: crate::application::resolve_title(original_title, url),
        content: fallback::extract_text(&fragment),
        url: ValidUrl::try_from_url(url.clone())?,
        excerpt: None,
        author,
        // legible supplied published_time and this path skips legible;
        // the date comes from the RAW document metadata (#2539).
        date: crate::domain::html_extraction::date_extractor::extract_date(html),
        html: Some(fragment),
        assets,
        correlation_id: Some(correlation.clone()),
        quality_hint: quality_hint.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_scraped_content(
    content: PageContent,
    author: Option<String>,
    html: &str,
    extraction_html: &str,
    asset_urls: &[crate::domain::ValidUrl],
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    original_title: &str,
    correlation: &CorrelationId,
    quality_hint: Option<crate::domain::extraction_quality::ExtractionQualityHint>,
) -> Result<ScrapedContent> {
    let article = match content {
        PageContent::Selector(fragment) => {
            return selector_scraped_content(
                fragment,
                author,
                asset_urls,
                url,
                config,
                downloader,
                html,
                original_title,
                correlation,
                quality_hint,
            )
            .await;
        },
        PageContent::Readability(article) => article,
    };

    match article {
        Ok(article) => {
            let assets = download_asset_urls(asset_urls, config, downloader).await?;

            // Shared minimum-content guard (#706): fail honestly on JS-shell
            // or near-empty extraction instead of returning Ok near-empty.
            crate::application::spa_detection::validate_min_content(
                url.as_str(),
                &article.text_content,
                html,
                correlation,
            )?;

            Ok(ScrapedContent {
                // H1 FIX: Use title from original DOM, falling back to Readability's title
                title: crate::application::resolve_title(
                    preferred_title(original_title, &article.title),
                    url,
                ),
                content: article.text_content,
                url: ValidUrl::try_from_url(url.clone())?,
                excerpt: article.excerpt.as_deref().map(|e| {
                    crate::domain::excerpt_repair::repair_empty_byline(e, author.as_deref())
                }),
                author,
                // legible never sees the metadata — clean_html strips the JSON-LD
                // blocks and the attributes off every <meta> — so the date has to
                // come from the raw document (#2563).
                date: crate::domain::html_extraction::date_extractor::extract_date(html),
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                // This is what downstream Markdown converters receive.
                html: Some(article.content),
                assets,
                correlation_id: Some(correlation.clone()),
                quality_hint: quality_hint.clone(),
            })
        },
        Err(e) => {
            warn!(
                url = %url,
                error = %e,
                "readability extraction failed; using text-extraction fallback"
            );
            // H2 FIX: Apply clean_html to fallback content to prevent JS/CSS leakage
            let raw_fallback = fallback::extract_text(extraction_html);
            let fallback_content = clean_html(&raw_fallback);
            let assets = download_asset_urls(asset_urls, config, downloader).await?;

            // Shared minimum-content guard (#706): the fallback branch has NO
            // content-size check today (unlike extract_content's
            // MIN_FALLBACK_CONTENT), so the guard is this branch's sole
            // authority — fail honestly on sub-threshold content.
            crate::application::spa_detection::validate_min_content(
                url.as_str(),
                &fallback_content,
                html,
                correlation,
            )?;

            Ok(ScrapedContent {
                // H1 FIX: the document title, or `resolve_title`'s fallback —
                // never a pre-substituted host (#2552).
                title: crate::application::resolve_title(original_title, url),
                content: fallback_content,
                url: ValidUrl::try_from_url(url.clone())?,
                excerpt: None,
                author: None,
                date: None,
                html: Some(html.to_string()),
                assets,
                correlation_id: Some(correlation.clone()),
                quality_hint: quality_hint.clone(),
            })
        },
    }
}

/// Fetch the page HTML, logging failures and rejecting non-2xx responses.
pub(super) async fn fetch_html(
    client: &dyn HttpClientPort,
    url: &url::Url,
    correlation: &CorrelationId,
) -> Result<HttpResponse> {
    let response = match client.get(url.as_str()).await {
        Ok(resp) => resp,
        Err(e) => {
            log_scrape_error_with_cause(
                &e,
                url.as_str(),
                "fetch",
                Some(correlation),
                "HTTP request failed",
            );
            return Err(scraper_error_from_http(e, url.as_str()));
        },
    };

    if !(200..300).contains(&response.status) {
        return Err(ScraperError::http(response.status, url.as_str()));
    }

    Ok(response)
}

/// Log the HTML size and record it on the current span, skipping detailed
/// instrumentation for bodies larger than 1MB.
pub(super) fn log_html_size(html: &str, url: &url::Url) {
    let html_size = html.len();
    let html_truncated = html_size > MAX_INSTRUMENTED_BODY_SIZE;
    if html_truncated {
        tracing::debug!(
            html_size_bytes = html_size,
            html_size_skipped = true,
            "HTML body exceeds 1MB, skipping detailed instrumentation"
        );
    } else {
        tracing::debug!("📄 Downloaded {} bytes from {}", html.len(), url);
    }

    // Add span field for html size (truncated)
    let span = tracing::Span::current();
    span.record("html_size_bytes", html_size.min(MAX_INSTRUMENTED_BODY_SIZE));
    span.record("html_size_skipped", html_truncated);
}

/// Detect WAF/CAPTCHA challenges disguised as HTTP 200 (REQ-WAF-05).
///
/// Context-aware inspection: status + content-type + headers drive the tiered
/// verdict, and a block carries the full Spanish evidence chain (REQ-WAF-08)
/// instead of a bare first-hit provider. `config.ignore_waf` short-circuits to
/// a clean verdict (REQ-WAF-07).
pub(super) fn detect_waf(
    html: &str,
    response: &HttpResponse,
    config: &ScraperConfig,
    url: &url::Url,
    correlation: &CorrelationId,
) -> Result<()> {
    let ctx = InspectionContext::from_lowercase_headers(
        response.status,
        &response.headers,
        config.ignore_waf,
    );
    let verdict = waf_inspector().inspect(html, &ctx);
    if verdict.is_blocked {
        let chain = verdict.evidence_chain();
        log_scrape_error(
            &chain,
            url.as_str(),
            "fetch",
            Some(correlation),
            "WAF challenge detected",
        );
        return Err(ScraperError::waf_blocked(url.to_string(), chain));
    }
    Ok(())
}
