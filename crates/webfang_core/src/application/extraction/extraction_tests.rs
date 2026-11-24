use super::*;
use crate::domain::config::ScraperConfig;
use crate::domain::{CorrelationId, ExtractResult};
use crate::error::ScraperError;

/// Base URL for the selector cases below.
fn base() -> url::Url {
    url::Url::parse("https://example.com/").expect("static base URL is valid")
}

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
    let result = extract_with_selector(html, "body", &base(), None);
    assert!(result.is_matched());
    assert_eq!(
        result.as_html(),
        html,
        "body selector must return HTML verbatim"
    );
}

#[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB: drops parsed Selector (#487, #764)
#[test]
fn test_matching_selector_returns_the_fragment_unwrapped() {
    let html = "<html><body><div class=\"main\">Hello</div><aside>noise</aside></body></html>";
    let result = extract_with_selector(html, "div.main", &base(), None);
    assert!(result.is_matched());
    let extracted = result.as_html();
    assert_eq!(
        extracted, "<div class=\"main\">Hello</div>",
        "the matched element is the content: no Readability wrapper, no siblings"
    );
    assert!(
        selector_supplied_fragment("div.main", &result),
        "an explicit selector that matched must skip Readability"
    );
    assert!(
        !selector_supplied_fragment("body", &result),
        "the body default never skips Readability, even when Matched"
    );
}

#[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB: drops parsed Selector (#487, #764)
#[test]
fn test_zero_matches_falls_back_without_inspector() {
    let html = "<html><body><p>content</p></body></html>";
    let result = extract_with_selector(html, "div.does-not-exist", &base(), None);
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
    let result = extract_with_selector(html, ">>>not-a-selector", &base(), None);
    assert!(!result.is_matched());
    assert_eq!(result.as_html(), html);
}

/// #761: the user-facing WARN must not leak `selectors` crate jargon
/// ("Please report this to the developer", variant debug names).
#[test]
fn test_invalid_selector_message_is_clean() {
    let msg = super::selector::invalid_selector_message("[[[invalid");
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
    let result = extract_with_selector("   ", "div.main", &base(), None);
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
/// the marker English reason, never Ok near-empty.
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
                reason.contains("insufficient content"),
                "English reason must state insufficient content: {reason}"
            );
            assert!(
                reason.contains("JavaScript rendering"),
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
/// NOT fire there (its English text would be a duplicate signal).
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
                reason.contains("poor fallback content"),
                "fallback branch error must come from MIN_FALLBACK_CONTENT, got: {reason}"
            );
            assert!(
                !reason.contains("insufficient content"),
                "the 50-char guard must NOT double-fire on the fallback branch: {reason}"
            );
        },
        other => panic!("expected ExtractionFailed, got: {other}"),
    }
}
