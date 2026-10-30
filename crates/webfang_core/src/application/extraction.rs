//! Content extraction — CSS selector extraction and Readability entry point.
//!
//! Hosts the pure CSS-selector extraction logic and the thin
//! [`scrape_with_readability`] convenience wrapper over the orchestrating
//! `scrape_with_config` use case.

use crate::application::diagnostic::build_diagnostic;
use crate::application::http_client::HttpClientPort;
use crate::application::scraper_service::scrape_with_config;
use crate::domain::config::ScraperConfig;
use crate::domain::html_cleaner::clean_html;
use crate::domain::html_extraction::{author_extractor, dom_pruner, fallback, readability};
use crate::domain::{
    CorrelationId, DomInspectorPort, ExtractResult, ScrapedContent, SelectorErrorKind, ValidUrl,
};
use crate::error::{Result, ScraperError};
use crate::infrastructure::observability::log_scrape_error;
use std::collections::HashSet;
use tracing::{debug, warn};

use crate::application::adaptive_engine::AdaptiveSelectorEngine;

/// Extract HTML content using a CSS selector.
///
/// When `selector` is not "body", parses the HTML and extracts all elements
/// matching the selector. Returns the outer HTML of matched elements wrapped
/// in a `<div>` for Readability processing. If no elements match or the
/// selector is invalid, returns [`ExtractResult::Fallback`] with the full
/// HTML and an optional diagnostic (when an inspector is provided).
///
/// # Arguments
/// * `html` - The HTML document to extract from
/// * `selector` - CSS selector string (use `"body"` to skip extraction)
/// * `inspector` - Optional DOM inspector for diagnostics on failure paths
pub fn extract_with_selector(
    html: &str,
    selector: &str,
    inspector: Option<&dyn DomInspectorPort>,
) -> ExtractResult {
    if selector == "body" {
        return ExtractResult::Matched(html.to_owned());
    }

    // Early check: empty or whitespace-only HTML. `scraper::Html::parse_document("")`
    // creates 3 implicit elements (html, head, body), so without this check the
    // selector matching would fall through to ZeroMatches instead of
    // EmptyDocument — leaving SelectorErrorKind::EmptyDocument as dead code.
    if html.trim().is_empty() {
        let document = scraper::Html::parse_document(html);
        return fallback(
            html,
            inspector,
            &document,
            SelectorErrorKind::EmptyDocument,
            selector,
            "HTML document is empty or whitespace-only, falling back with EmptyDocument diagnostic",
        );
    }

    let document = scraper::Html::parse_document(html);
    let sel = match scraper::Selector::parse(selector) {
        Ok(s) => s,
        Err(e) => {
            return fallback(
                html,
                inspector,
                &document,
                // The diagnostic keeps the raw crate error for debugging;
                // the user-facing WARN must not leak library jargon (#761).
                SelectorErrorKind::InvalidSelector(e.to_string()),
                selector,
                &invalid_selector_message(selector),
            );
        },
    };

    let matched: Vec<String> = document.select(&sel).map(|el| el.html()).collect();

    if matched.is_empty() {
        return fallback(
            html,
            inspector,
            &document,
            SelectorErrorKind::ZeroMatches,
            selector,
            &format!("CSS selector '{selector}' matched 0 elements, falling back to full HTML"),
        );
    }

    debug!(
        "CSS selector '{}' matched {} elements",
        selector,
        matched.len()
    );

    ExtractResult::Matched(format!(
        "<div id=\"selector-extracted\">{}</div>",
        matched.join("\n")
    ))
}

/// Serialize the elements matched by `selector` into one fragment.
///
/// Matches nested inside another match are dropped so each element is emitted
/// once, and `href`/`src` are resolved against `base_url` — the two jobs
/// Readability used to do implicitly for this path. Blocks are separated by a
/// blank line, which is what makes `--selector h3` yield two paragraphs instead
/// of two concatenated headings.
///
/// Returns `None` when the selector matches nothing.
// Wired into the `--selector` path by the next commit of #2534; until then the
// only callers are the tables below.
#[allow(dead_code)]
fn serialize_matches(
    document: &scraper::Html,
    selector: &scraper::Selector,
    base_url: &url::Url,
) -> Option<String> {
    let matches: Vec<scraper::ElementRef<'_>> = document.select(selector).collect();
    if matches.is_empty() {
        return None;
    }

    // `ancestors()` starts at the parent, so a match never discards itself.
    let selected: HashSet<_> = matches.iter().map(|element| element.id()).collect();
    let blocks: Vec<String> = matches
        .iter()
        .filter(|element| {
            !element
                .ancestors()
                .any(|ancestor| selected.contains(&ancestor.id()))
        })
        .map(scraper::ElementRef::html)
        .collect();

    let fragment = blocks.join("\n\n");
    // `htmd::convert` parses with `parse_document`, so a row or cell fragment
    // loses its tags at conversion time: the HTML5 parser drops `<tr>`/`<td>`
    // outside a table context and the cells collapse into bare text. Restoring
    // the synthetic ancestor here is the only place it can be restored — the
    // converter cannot recover context the fragment never had.
    let fragment = if matches
        .first()
        .is_some_and(|element| needs_table_context(element.value().name()))
    {
        format!("<table><tbody>{fragment}</tbody></table>")
    } else {
        fragment
    };

    Some(absolutize_urls(&fragment, base_url))
}

/// Whether a matched tag only means something inside a `<table>`.
///
/// `caption` is deliberately absent: it belongs outside `<tbody>`, so the
/// synthetic wrapper below would be wrong for it.
fn needs_table_context(tag: &str) -> bool {
    matches!(tag, "tr" | "td" | "th" | "thead" | "tbody" | "tfoot")
}

/// Resolve `href`/`src` attributes in `fragment` against `base_url`.
///
/// Fragment-only links are left alone: rewriting `#top` into an absolute URL
/// changes what the link means. Values `Url::join` cannot resolve keep their
/// original text rather than losing the link, and a rewriter failure returns
/// the fragment untouched — the fragment is never dropped on the floor.
fn absolutize_urls(fragment: &str, base_url: &url::Url) -> String {
    use lol_html::{element, RewriteStrSettings};

    let settings = RewriteStrSettings {
        element_content_handlers: vec![
            element!("a[href]", |el| {
                resolve_attr(el, "href", base_url);
                Ok(())
            }),
            element!("img[src]", |el| {
                resolve_attr(el, "src", base_url);
                Ok(())
            }),
            element!("source[src]", |el| {
                resolve_attr(el, "src", base_url);
                Ok(())
            }),
            element!("video[poster]", |el| {
                resolve_attr(el, "poster", base_url);
                Ok(())
            }),
        ],
        ..RewriteStrSettings::default()
    };

    lol_html::rewrite_str(fragment, settings).unwrap_or_else(|_| fragment.to_owned())
}

/// Rewrite one attribute in place when it resolves against `base_url`.
fn resolve_attr(el: &mut lol_html::html_content::Element<'_, '_>, attr: &str, base_url: &url::Url) {
    let Some(raw) = el.get_attribute(attr) else {
        return;
    };
    if raw.starts_with('#') {
        return;
    }
    if let Ok(resolved) = base_url.join(&raw) {
        let _ = el.set_attribute(attr, resolved.as_str());
    }
}

/// Emit the fallback warning and build a [`ExtractResult::Fallback`] carrying
/// the full HTML and a diagnostic for the given [`SelectorErrorKind`], when an
/// inspector is available.
/// Clean user-facing message for an invalid CSS selector (#761).
///
/// The `selectors` crate's error text leaks library jargon
/// (`NoQualifiedNameInAttributeSelector(...)`, "Please report this to the
/// developer") into production stderr. The raw error is preserved in the
/// [`SelectorErrorKind::InvalidSelector`] diagnostic for debugging; only this
/// sanitized message reaches the user.
fn invalid_selector_message(selector: &str) -> String {
    format!("selector CSS inválido: '{selector}' — se usó el HTML completo como fallback")
}

fn fallback(
    html: &str,
    inspector: Option<&dyn DomInspectorPort>,
    document: &scraper::Html,
    kind: SelectorErrorKind,
    selector: &str,
    message: &str,
) -> ExtractResult {
    warn!("{}", message);
    ExtractResult::Fallback {
        html: html.to_owned(),
        diagnostic: build_diagnostic(inspector, document, kind, selector),
    }
}

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
#[cfg(feature = "adaptive-selectors")]
pub(crate) async fn adaptive_selector_repair(
    extract_result: ExtractResult,
    engine: Option<&AdaptiveSelectorEngine>,
    selector: &str,
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
                    let repaired =
                        extract_with_selector(&html, &outcome.suggestion.selector, inspector);
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

/// Pipeline de extracción de contenido: clean → selector → adaptive → readability/fallback.
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
    let extract_result = extract_with_selector(&cleaned_html, &config.selector, None);
    // Adaptive selector repair (Tier 1 lexical): delegate to the canonical
    // shared helper (#442). Returns (ExtractResult, Option<CascadeTrace>).
    // The trace enables structural scoring for honest error hints (#792).
    #[cfg(feature = "adaptive-selectors")]
    let (extract_result, adaptive_trace) = adaptive_selector_repair(
        extract_result,
        engine,
        &config.selector,
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
                date: article.published_time,
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
                    "contenido pobre del fallback: {} bytes (mín {} bytes). Readability: {}",
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
                title: url
                    .host_str()
                    .ok_or_else(|| ScraperError::invalid_url(format!("URL missing host: {url}")))?
                    .to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // Miri gating policy for this module (#764, #487): 6 tests below trip the
    // known servo_arc 0.4.3 ArcUnion Tree Borrows UB via two entry points:
    // - valid-selector tests drop a `scraper::Selector` built in
    //   `extract_with_selector` (same defect family as dom_inspector, #763);
    // - `scrape_with_readability`/`extract_content` tests reach `clean_html`
    //   (html_cleaner.rs:112) → `lol_html::rewrite_str`, whose drop glue ends
    //   in the same `servo_arc::ArcUnion` drop (recurrence of #498, same
    //   class as the discovery.rs gates in #486).
    // The 3 ungated tests avoid it: body passthrough (no Selector), and the
    // invalid-selector / whitespace-only fallbacks (parse failure happens
    // before `selectors::SelectorList` is constructed, so nothing drops).
    // Regular (non-Miri) runs keep full coverage.

    // --- extract_with_selector: pure, no network (scraper::Html from a string) ---

    #[test]
    fn test_body_selector_is_passthrough() {
        let html = "<html><body><p>keep me</p></body></html>";
        let result = extract_with_selector(html, "body", None);
        assert!(result.is_matched());
        assert_eq!(
            result.as_html(),
            html,
            "body selector must return HTML verbatim"
        );
    }

    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB: drops parsed Selector (#487, #764)
    #[test]
    fn test_matching_selector_wraps_elements() {
        let html = "<html><body><div class=\"main\">Hello</div><aside>noise</aside></body></html>";
        let result = extract_with_selector(html, "div.main", None);
        assert!(result.is_matched());
        let extracted = result.as_html();
        assert!(
            extracted.starts_with("<div id=\"selector-extracted\">"),
            "matched output must be wrapped for Readability, got: {extracted}"
        );
        assert!(extracted.contains("Hello"));
        assert!(
            !extracted.contains("noise"),
            "non-matching elements must be excluded"
        );
    }

    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB: drops parsed Selector (#487, #764)
    #[test]
    fn test_zero_matches_falls_back_without_inspector() {
        let html = "<html><body><p>content</p></body></html>";
        let result = extract_with_selector(html, "div.does-not-exist", None);
        assert!(!result.is_matched());
        assert_eq!(result.as_html(), html, "fallback must carry the full HTML");
        match result {
            ExtractResult::Fallback { diagnostic, .. } => {
                assert!(diagnostic.is_none(), "no inspector means no diagnostic");
            },
            ExtractResult::Matched(_) => panic!("expected Fallback"),
        }
    }

    #[test]
    fn test_invalid_selector_falls_back() {
        let html = "<html><body><p>content</p></body></html>";
        let result = extract_with_selector(html, ">>>not-a-selector", None);
        assert!(!result.is_matched());
        assert_eq!(result.as_html(), html);
    }

    /// #761: the user-facing WARN must not leak `selectors` crate jargon
    /// ("Please report this to the developer", variant debug names).
    #[test]
    fn test_invalid_selector_message_is_clean() {
        let msg = super::invalid_selector_message("[[[invalid");
        assert!(
            msg.contains("'[[[invalid'"),
            "must name the offending selector"
        );
        assert!(
            !msg.contains("Please report this to the developer"),
            "must not leak crate jargon: {msg}"
        );
        assert!(
            !msg.contains("NoQualifiedNameInAttributeSelector"),
            "must not leak variant debug names: {msg}"
        );
    }

    #[test]
    fn test_empty_html_falls_back() {
        let result = extract_with_selector("   ", "div.main", None);
        assert!(!result.is_matched(), "whitespace-only HTML must fall back");
    }

    // --- scrape_with_readability: ephemeral mock HTTP client, no real network ---

    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB in ArcUnion drop via lol_html (#487, #764)
    #[tokio::test]
    async fn test_scrape_with_readability_produces_single_result() {
        use crate::domain::http_port::HttpResponse;
        use crate::test_fixtures::MockHttpClient;
        use std::collections::HashMap;

        let url = url::Url::parse("https://example.com").unwrap();
        let article = "<html><head><title>Test Article</title></head><body><article>\
             <h1>Test Article</h1>\
             <p>This is a reasonably long paragraph of article content that should survive \
             readability extraction without any problems at all, providing plenty of text.</p>\
             </article></body></html>";
        let mock = MockHttpClient::new().with_response(
            url.as_str(),
            Ok(HttpResponse {
                status: 200,
                body: article.to_string(),
                headers: HashMap::new(),
            }),
        );

        let results = scrape_with_readability(&mock, &url)
            .await
            .expect("a 200 article response must scrape successfully");

        assert_eq!(results.len(), 1, "one URL must yield exactly one result");
        assert_eq!(
            results[0].url.as_str(),
            url.as_str(),
            "URL must be preserved"
        );
        assert!(
            !results[0].title.is_empty(),
            "title must resolve to non-empty"
        );
    }

    // --- extract_content: the shared minimum-content guard (#706) ---

    /// CLI funnel success branch (#684): readability succeeds but yields
    /// sub-threshold text on a JS-shell page → typed `ExtractionFailed` with
    /// the marker Spanish reason, never Ok near-empty.
    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB in ArcUnion drop via lol_html (#487, #764)
    #[tokio::test]
    async fn test_extract_content_success_branch_below_threshold_errors() {
        let html = "<html><head><title>App</title></head><body>\
             <article><h1>Status</h1><p>Initializing…</p></article>\
             <div id=\"root\"></div>\
             </body></html>";
        let url = url::Url::parse("https://spa.example.com/app").unwrap();
        let corr = CorrelationId::new();

        let result = extract_content(html, &url, &ScraperConfig::default(), None, None, &corr)
            .await
            .expect_err("sub-threshold success-branch content must fail honestly");

        match &result {
            ScraperError::ExtractionFailed {
                url: failed_url,
                reason,
            } => {
                assert_eq!(failed_url, url.as_str(), "url must be preserved");
                assert!(
                    reason.contains("contenido insuficiente"),
                    "Spanish reason must state insufficient content: {reason}"
                );
                assert!(
                    reason.contains("renderizado de JavaScript"),
                    "marker-bearing shell must report the JS cause: {reason}"
                );
            },
            other => panic!("expected ExtractionFailed, got: {other}"),
        }
    }

    /// XC-2 legit fixture: a server-rendered page with ≥50 chars of extractable
    /// text MUST still succeed — the guard never fires above the threshold.
    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB in ArcUnion drop via lol_html (#487, #764)
    #[tokio::test]
    async fn test_extract_content_legit_page_above_threshold_succeeds() {
        let html = "<html><head><title>Docs</title></head><body><article>\
             <h1>Guide</h1>\
             <p>This is a substantially long paragraph of server-rendered \
             article content, easily over the fifty character extraction \
             threshold, so the guard must let it through untouched.</p>\
             </article></body></html>";
        let url = url::Url::parse("https://example.com/guide").unwrap();
        let corr = CorrelationId::new();

        let result = extract_content(html, &url, &ScraperConfig::default(), None, None, &corr)
            .await
            .expect("a legit page must keep scraping successfully");

        assert!(
            result.content.chars().count() >= crate::application::spa_detection::MIN_CONTENT_CHARS,
            "legit pages must yield substantial content"
        );
    }

    /// CE-3 no double-gate: when readability FAILS, `MIN_FALLBACK_CONTENT=100`
    /// stays the SOLE authority on the fallback branch — the 50-char guard must
    /// NOT fire there (its Spanish text would be a duplicate signal).
    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB in ArcUnion drop via lol_html (#487, #764)
    #[tokio::test]
    async fn test_extract_content_poor_fallback_keeps_min_fallback_authority() {
        let html = "<html><body><a href=\"/x\"></a></body></html>";
        let url = url::Url::parse("https://example.com/empty").unwrap();
        let corr = CorrelationId::new();

        let result = extract_content(html, &url, &ScraperConfig::default(), None, None, &corr)
            .await
            .expect_err("a content-less page must fail");

        match &result {
            ScraperError::ExtractionFailed { reason, .. } => {
                assert!(
                    reason.contains("contenido pobre del fallback"),
                    "fallback branch error must come from MIN_FALLBACK_CONTENT, got: {reason}"
                );
                assert!(
                    !reason.contains("contenido insuficiente"),
                    "the 50-char guard must NOT double-fire on the fallback branch: {reason}"
                );
            },
            other => panic!("expected ExtractionFailed, got: {other}"),
        }
    }
}

/// #2534: the `--selector` fragment must be a function of the selector, not of
/// Readability. These tables pin the serialization contract (HTML) and the
/// Markdown the user actually receives (through the same converter the CLI
/// writes with, `file_saver.rs`).
#[cfg(test)]
mod selector_serialization {
    use super::*;
    use crate::domain::ports::MarkdownConverter;
    use crate::infrastructure::converter::htmd_converter::HtmdConverter;

    const BASE: &str = "https://example.com/docs/page.html";

    /// Pure serialization: what the fragment looks like before any converter.
    struct HtmlCase {
        name: &'static str,
        html: &'static str,
        selector: &'static str,
        /// `None` means the selector matched nothing.
        expected: Option<&'static str>,
    }

    #[test]
    fn selector_fragment_html_table() {
        let cases = [
            HtmlCase {
                name: "nested_div_emitted_once",
                html: r#"<div class="outer"><div class="inner">x</div></div>"#,
                selector: "div",
                expected: Some(r#"<div class="outer"><div class="inner">x</div></div>"#),
            },
            HtmlCase {
                name: "relative_href_and_img_src",
                html: r#"<a href="/root">a</a><img src="img.png">"#,
                selector: "a, img",
                expected: Some(
                    "<a href=\"https://example.com/root\">a</a>\n\n\
                     <img src=\"https://example.com/docs/img.png\">",
                ),
            },
            HtmlCase {
                name: "unresolvable_schemes_untouched",
                html: r##"<a href="mailto:a@b.co">m</a><a href="#top">t</a><a href="javascript:void(0)">j</a>"##,
                selector: "a",
                expected: Some(
                    "<a href=\"mailto:a@b.co\">m</a>\n\n\
                     <a href=\"#top\">t</a>\n\n\
                     <a href=\"javascript:void(0)\">j</a>",
                ),
            },
            HtmlCase {
                name: "two_siblings_joined_by_blank_line",
                html: "<p>one</p><p>two</p>",
                selector: "p",
                expected: Some("<p>one</p>\n\n<p>two</p>"),
            },
            HtmlCase {
                name: "no_match_is_none",
                html: "<p>x</p>",
                selector: "blockquote",
                expected: None,
            },
        ];

        let base = url::Url::parse(BASE).expect("BASE is a valid absolute URL");
        let mut failures = Vec::new();
        for case in cases {
            let document = scraper::Html::parse_document(case.html);
            let selector = scraper::Selector::parse(case.selector)
                .expect("every table selector is a valid CSS selector");
            let actual = serialize_matches(&document, &selector, &base);
            if actual.as_deref() != case.expected {
                failures.push(format!(
                    "case `{}`\n  expected: {:?}\n  actual:   {:?}",
                    case.name, case.expected, actual
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} selector case(s) diverged:\n{}",
            failures.len(),
            failures.join("\n---\n")
        );
    }

    /// What the user receives in the `.md`: fragment → the production converter.
    struct MdCase {
        name: &'static str,
        html: &'static str,
        selector: &'static str,
        expected: &'static str,
    }

    #[test]
    fn selector_fragment_markdown_table() {
        let cases = [
            MdCase {
                name: "h3",
                html: "<h3>Section</h3>",
                selector: "h3",
                expected: "### Section",
            },
            MdCase {
                name: "table",
                html: "<table><thead><tr><th>Name</th><th>Value</th></tr></thead>\
                       <tbody><tr><td>Foo</td><td>42</td></tr></tbody></table>",
                selector: "table",
                // A real GFM table: the whole point of bypassing Readability.
                expected: "| Name | Value |\n| ---- | ----- |\n| Foo  | 42    |",
            },
            MdCase {
                name: "characterize_li_orphan_bullet",
                html: "<li>one</li><li>two</li>",
                selector: "li",
                // Characterization: htmd falls back to a bullet when the parent
                // is not <ol>, so an orphan <li> still renders as a list item.
                expected: "*   one\n*   two",
            },
            MdCase {
                name: "characterize_tr_inside_table",
                html: "<table><tbody><tr><td>a</td><td>b</td></tr></tbody></table>",
                selector: "tr",
                // Characterization: the synthetic <table><tbody> keeps the cell
                // boundary as a paragraph break, but htmd still emits no table
                // (it needs a <thead> to build the header row).
                expected: "a\n\nb",
            },
            MdCase {
                name: "characterize_td_inside_table",
                html: "<table><tbody><tr><td>a</td></tr></tbody></table>",
                selector: "td",
                // Characterization: a lone cell has no row to belong to.
                expected: "a",
            },
            MdCase {
                name: "characterize_strong_inline_separator",
                html: "<strong>a</strong><strong>b</strong>",
                selector: "strong",
                // Characterization: HTML collapses the blank line between two
                // inline matches into a space, so both end up in one paragraph.
                expected: "**a** **b**",
            },
        ];

        let base = url::Url::parse(BASE).expect("BASE is a valid absolute URL");
        let converter = HtmdConverter;
        let mut failures = Vec::new();
        for case in cases {
            let document = scraper::Html::parse_document(case.html);
            let selector = scraper::Selector::parse(case.selector)
                .expect("every table selector is a valid CSS selector");
            let Some(fragment) = serialize_matches(&document, &selector, &base) else {
                failures.push(format!(
                    "case `{}` matched nothing — fixture is wrong, not the code",
                    case.name
                ));
                continue;
            };
            let markdown = converter.convert(&fragment);
            if markdown.trim() != case.expected {
                failures.push(format!(
                    "case `{}`\n  fragment: {fragment}\n  markdown:\n{markdown}",
                    case.name
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} markdown case(s) diverged:\n{}",
            failures.len(),
            failures.join("\n---\n")
        );
    }
}
