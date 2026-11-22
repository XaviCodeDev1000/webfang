use super::*;

// Install the real WAF inspector on first test access (#996). The static
// is idempotent, so a no-op if `Container::new` already initialized it.
use std::sync::{Arc, OnceLock};
fn ensure_waf_inspector() {
    use crate::domain::waf::set_waf_inspector;
    use crate::infrastructure::http::waf_engine::WafInspector;
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        set_waf_inspector(Arc::new(WafInspector) as Arc<dyn crate::domain::waf::WafInspectorPort>);
    });
}

// ========================================================================
// scrape_single_url — Downloader-injection unit tests (#303)
// ========================================================================

use crate::domain::downloader_port::FetchedPage;
use futures::future::BoxFuture;
use std::collections::HashMap;

struct StubDownloader {
    result: std::sync::Mutex<Option<Result<FetchedPage, DownloadError>>>,
}

impl StubDownloader {
    fn returning(result: Result<FetchedPage, DownloadError>) -> Self {
        Self {
            result: std::sync::Mutex::new(Some(result)),
        }
    }
}

impl Downloader for StubDownloader {
    fn fetch<'a>(&'a self, _url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        let result = self
            .result
            .lock()
            .expect("stub lock poisoned")
            .take()
            .expect("fetch called more than once on StubDownloader");
        Box::pin(async move { result })
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        0
    }
}

fn stub_page(html: &str, status: u16, headers: Vec<(&str, &str)>) -> FetchedPage {
    FetchedPage {
        url: Url::parse("https://example.com").expect("valid test URL"),
        html: html.to_owned(),
        status,
        headers: headers
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect::<HashMap<_, _>>(),
        cookies: Vec::new(),
    }
}

#[tokio::test]
async fn scrape_single_binary_content_type_returns_early() {
    let page = stub_page(
        "%PDF-1.4 fake bytes",
        200,
        vec![("content-type", "application/pdf")],
    );
    let dl = StubDownloader::returning(Ok(page));
    let url = Url::parse("https://example.com/doc.pdf").expect("valid URL");
    let config = ScraperConfig::new();

    let corr = CorrelationId::new();
    let result = scrape_single_url(&dl, &url, &config, None, None, None, &corr, &corr)
        .await
        .expect("binary detection should succeed");

    assert!(
        result.content.contains("[Binary content:"),
        "expected binary marker, got: {}",
        result.content
    );
    assert_eq!(result.title, "example.com");
}

#[tokio::test]
async fn scrape_single_waf_body_signature_returns_waf_blocked() {
    ensure_waf_inspector();
    let waf_html = r#"<html><body><div id="cf-browser-verification">Checking your browser before accessing the site.</div></body></html>"#;
    let page = stub_page(waf_html, 200, vec![("content-type", "text/html")]);
    let dl = StubDownloader::returning(Ok(page));
    let url = Url::parse("https://example.com").expect("valid URL");
    let config = ScraperConfig::new();

    let corr = CorrelationId::new();
    let err = scrape_single_url(&dl, &url, &config, None, None, None, &corr, &corr)
        .await
        .expect_err("WAF body should trigger WafBlocked");

    assert!(
        matches!(err, ScraperError::WafBlocked { .. }),
        "expected WafBlocked, got: {err:?}"
    );
}

#[tokio::test]
#[cfg(not(miri))] // lol_html → servo_arc triggers Tree Borrows UB (upstream, experimental model)
async fn scrape_single_normal_html_extracts_title_and_content() {
    let html = r#"<html><head><title>Test Article</title></head>
<body><article><h1>Heading</h1>
<p>This is a substantial paragraph with enough text for the readability
algorithm to identify it as the main content of the page and extract it
properly into the ScrapedContent output structure.</p>
<p>A second paragraph ensures the extractor has multiple text blocks to
work with when computing the document readability score.</p>
</article></body></html>"#;
    let page = stub_page(html, 200, vec![("content-type", "text/html")]);
    let dl = StubDownloader::returning(Ok(page));
    let url = Url::parse("https://example.com/article").expect("valid URL");
    let config = ScraperConfig::new();

    let corr = CorrelationId::new();
    let result = scrape_single_url(&dl, &url, &config, None, None, None, &corr, &corr)
        .await
        .expect("normal HTML should scrape successfully");

    assert!(
        !result.title.is_empty(),
        "title should be extracted from HTML"
    );
    assert!(
        !result.content.is_empty(),
        "content should be extracted from HTML"
    );
}

#[tokio::test]
async fn scrape_single_download_waf_challenge_maps_to_waf_blocked() {
    let dl = StubDownloader::returning(Err(DownloadError::WafChallenge("Cloudflare".to_owned())));
    let url = Url::parse("https://example.com").expect("valid URL");
    let config = ScraperConfig::new();

    let corr = CorrelationId::new();
    let err = scrape_single_url(&dl, &url, &config, None, None, None, &corr, &corr)
        .await
        .expect_err("WafChallenge download error should propagate");

    match err {
        ScraperError::WafBlocked { url, provider } => {
            assert_eq!(provider, "Cloudflare");
            assert!(url.contains("example.com"));
        },
        other => panic!("expected WafBlocked, got: {other:?}"),
    }
}

#[tokio::test]
async fn scrape_single_non_2xx_status_returns_http_error() {
    let page = stub_page("not found", 404, vec![("content-type", "text/html")]);
    let dl = StubDownloader::returning(Ok(page));
    let url = Url::parse("https://example.com/missing").expect("valid URL");
    let config = ScraperConfig::new();

    let corr = CorrelationId::new();
    let err = scrape_single_url(&dl, &url, &config, None, None, None, &corr, &corr)
        .await
        .expect_err("404 status should produce an error");

    assert!(
        matches!(err, ScraperError::Http { status: 404, .. }),
        "expected Http(404), got: {err:?}"
    );
}

// ─── extract_content direct tests (no Downloader needed) ───
// All gated: extract_content → html_cleaner → lol_html → servo_arc
// triggers Tree Borrows UB in servo_arc Arc::drop (upstream, experimental model).

#[tokio::test]
#[cfg(not(miri))]
async fn extract_content_readability_extracts_article() {
    let html = r#"<html><head><title>Test Page</title></head><body>
            <article><h1>Hello</h1><p>This is a long enough paragraph of content that readability should be able to extract properly from the DOM structure.</p>
            <p>Second paragraph with more content to ensure readability has enough material to work with for extraction.</p></article>
        </body></html>"#;
    let url = Url::parse("https://example.com/article").unwrap();
    let config = ScraperConfig::default();

    let corr = CorrelationId::new();
    let result = extract_content(html, &url, &config, None, None, &corr).await;

    assert!(result.is_ok());
    let content = result.unwrap();
    assert!(!content.content.is_empty());
    assert_eq!(content.url.as_str(), "https://example.com/article");
}

#[tokio::test]
#[cfg(not(miri))]
async fn extract_content_fallback_short_content_returns_error() {
    let html = "<html><body><div></div></body></html>";
    let url = Url::parse("https://example.com/tiny").unwrap();
    let config = ScraperConfig::default();

    let corr = CorrelationId::new();
    let result = extract_content(html, &url, &config, None, None, &corr).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, ScraperError::ExtractionFailed { .. }),
        "Expected ExtractionFailed, got: {err:?}"
    );
}

#[tokio::test]
#[cfg(not(miri))]
async fn extract_content_with_selector_extracts_matching_content() {
    let html = r#"<html><body>
            <nav>Navigation stuff</nav>
            <div class="main-content"><p>This is the main content that should be extracted by the CSS selector we configured.</p></div>
            <footer>Footer stuff</footer>
        </body></html>"#;
    let url = Url::parse("https://example.com/page").unwrap();
    let config = ScraperConfig {
        selector: ".main-content".to_string(),
        ..Default::default()
    };

    let corr = CorrelationId::new();
    let result = extract_content(html, &url, &config, None, None, &corr).await;

    assert!(result.is_ok());
    let content = result.unwrap();
    assert!(content.content.contains("main content"));
}

#[tokio::test]
#[cfg(not(miri))]
async fn extract_content_populates_correlation_id_natively() {
    // Fase 1 (issue #356): correlation_id must be populated natively,
    // WITHOUT requiring the `otel` feature. Every scraped page must be
    // correlatable with its own logs/traces out of the box.
    let html = r#"<html><head><title>Test Page</title></head><body>
            <article><h1>Hello</h1><p>This is a long enough paragraph of content that readability should be able to extract properly from the DOM structure.</p>
            <p>Second paragraph with more content to ensure readability has enough material to work with for extraction.</p></article>
        </body></html>"#;
    let url = Url::parse("https://example.com/article").unwrap();
    let config = ScraperConfig::default();

    let corr = CorrelationId::new();
    let result = extract_content(html, &url, &config, None, None, &corr).await;

    assert!(result.is_ok());
    let content = result.unwrap();
    assert!(
        content.correlation_id.is_some(),
        "correlation_id must be populated natively (without the `otel` feature)"
    );
    // Must be a valid W3C traceparent: 00-{trace_id}-{span_id}-01
    let corr = content.correlation_id.unwrap();
    assert!(corr.to_traceparent().starts_with("00-"));
}

#[tokio::test]
#[cfg(not(miri))]
async fn extract_content_reuses_injected_correlation_id() {
    // Issue #501: callers own the page identity (e.g. the scrape span in
    // `scrape_single_url`) and inject it so the exported content
    // is correlatable with the trace's `span_fields` — exact identity,
    // not a freshly generated one. The parameter is required: identity
    // enters through the type system or not at all.
    let html = r#"<html><head><title>Test Page</title></head><body>
            <article><h1>Hello</h1><p>This is a long enough paragraph of content that readability should be able to extract properly from the DOM structure.</p>
            <p>Second paragraph with more content to ensure readability has enough material to work with for extraction.</p></article>
        </body></html>"#;
    let url = Url::parse("https://example.com/article").unwrap();
    let config = ScraperConfig::default();
    let injected = CorrelationId::new_with_ids(uuid::Uuid::now_v7(), 0x00AB);

    let content = extract_content(html, &url, &config, None, None, &injected)
        .await
        .expect("extraction with an injected correlation ID must succeed");

    assert_eq!(
        content.correlation_id.as_ref(),
        Some(&injected),
        "extract_content must reuse the injected correlation ID exactly"
    );
}
