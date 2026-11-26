use super::tests_common::*;
use super::*;
use crate::mcp_server::handlers::test_support::result_text;
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// #1116: invalid URL is unrepresentable at the `McpUrl` boundary.
#[test]
fn crawl_site_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<CrawlSiteParams>(serde_json::json!({
        "url": "not a url"
    }));
    assert!(
        res.is_err(),
        "invalid URL must fail to deserialize: {res:?}"
    );
}

/// #1116: invalid URL is unrepresentable at the `McpUrl` boundary.
#[test]
fn crawl_with_sitemap_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<CrawlWithSitemapParams>(serde_json::json!({
        "url": "not a url"
    }));
    assert!(
        res.is_err(),
        "invalid URL must fail to deserialize: {res:?}"
    );
}

/// REQ-01: the post-hoc filter keeps seed-host-internal URLs and drops
/// external-domain and forbidden-literal-IP URLs, reporting the exclusion
/// count. Empty input produces empty output with zero exclusions.
#[test]
fn filter_ssrf_safe_keeps_internal_drops_external_literal() {
    let seed = url::Url::parse("https://example.com/").expect("valid seed");
    let discovered = vec![
        webfang_core::domain::DiscoveredUrl::html(
            url::Url::parse("https://example.com/a").expect("valid url"),
            1,
            seed.clone(),
        ),
        webfang_core::domain::DiscoveredUrl::html(
            url::Url::parse("https://other.com/x").expect("valid url"),
            1,
            seed.clone(),
        ),
        webfang_core::domain::DiscoveredUrl::html(
            url::Url::parse("http://10.0.0.5/x").expect("valid url"),
            1,
            seed.clone(),
        ),
    ];

    let mut out = Vec::new();
    let excluded = filter_ssrf_safe(&discovered, &seed, &mut out);
    assert_eq!(
        out,
        vec!["https://example.com/a".to_string()],
        "only the seed-host URL survives"
    );
    assert_eq!(excluded, 2, "external + forbidden literal are excluded");

    let mut empty_out = Vec::new();
    assert_eq!(
        filter_ssrf_safe(&[], &seed, &mut empty_out),
        0,
        "empty input excludes nothing"
    );
    assert!(
        empty_out.is_empty(),
        "empty input keeps the output empty (established as the filtered case above returns entries)"
    );
}

/// REQ-02: an internal `sitemap_url` is rejected at entry (guard ON) with
/// the SSRF message, before any sitemap fetch. The seed uses a public
/// literal IP so its own validation resolves locally (no DNS dependency).
#[cfg_attr(miri, ignore = "btls TLS_method FFI via Container::new")]
#[tokio::test]
#[serial]
async fn crawl_with_sitemap_rejects_internal_sitemap_url() {
    // The escape hatch must be unset so the guard is active for this
    // test; EnvGuard restores the original on drop, so the removal can
    // no longer leak into sibling tests in a shared process (#1126).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
    ]);

    let (handler, _tmp) = test_handler().await;
    let res = handler
        .crawl_with_sitemap(Parameters(CrawlWithSitemapParams {
            url: vu("http://8.8.8.8/"),
            sitemap_url: Some(vu("http://127.0.0.1/")),
            max_depth: None,
            max_pages: None,
        }))
        .await;
    assert_ssrf_rejected(res);
}

/// Phase 2 (#1429, RED): `crawl_with_sitemap` is session-owned — after
/// the call, `session_results` holds THIS sitemap run's records so a
/// subsequent export serves the sitemap run, not a previous run's.
/// Wiremock sitemap fixture: two pages listed, all served with
/// article HTML. Fails today (discovery-only: nothing stored).
#[cfg_attr(miri, ignore)] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
#[tokio::test]
#[serial]
async fn crawl_with_sitemap_stores_sitemap_run_in_session_results() {
    // Lift both guards for this test only (wiremock binds 127.0.0.1):
    // the MCP entry validator and the shared core literal-IP entry
    // guard (F-06 + F-32, #1217). EnvGuard restores the originals on
    // drop, so the "1"s cannot leak into siblings (#1126).
    let _guard = webfang_test_utils::EnvGuard::ssrf_hatches_off();
    let (handler, _tmp) = test_handler().await;
    let server = MockServer::start().await;
    let base = server.uri();
    let sitemap = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n<url><loc>{base}/a</loc></url>\n<url><loc>{base}/b</loc></url>\n</urlset>"
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/xml")
                .set_body_string(sitemap),
        )
        .mount(&server)
        .await;
    for page in ["/", "/a", "/b"] {
        Mock::given(method("GET"))
            .and(path(page))
            .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
            .mount(&server)
            .await;
    }

    let res = handler
        .crawl_with_sitemap(Parameters(CrawlWithSitemapParams {
            url: vu(&base),
            sitemap_url: Some(vu(&format!("{base}/sitemap.xml"))),
            max_depth: None,
            max_pages: None,
        }))
        .await
        .expect("handler returns Ok");

    // Wire shape unchanged: the bare JSON string array carries the
    // discovered sitemap pages.
    let raw = result_text(&res);
    // #1600: the discovered-URL list is untrusted content — unwrap the
    // provenance envelope before parsing the bare JSON array.
    let text = crate::mcp_server::provenance::payload_of(&raw)
        .expect("crawl_with_sitemap response must carry the UNTRUSTED envelope");
    let urls: Vec<String> =
        serde_json::from_str(text).expect("response must stay a bare JSON string array");
    assert!(
        urls.iter().any(|u| u.ends_with("/a")) && urls.iter().any(|u| u.ends_with("/b")),
        "response array must carry the sitemap pages, got: {urls:?}"
    );

    // Session ownership: the sitemap run is stored for the export tools.
    let stored = handler
        .state
        .session_results
        .lock()
        .expect("session_results lock")
        .clone();
    assert!(
        !stored.is_empty(),
        "session_results must hold the sitemap run after crawl_with_sitemap"
    );
    let stored_urls: Vec<&str> = stored.iter().map(|c| c.url.as_str()).collect();
    assert!(
        stored_urls.iter().any(|u| u.ends_with("/a"))
            && stored_urls.iter().any(|u| u.ends_with("/b")),
        "stored run must cover the sitemap pages, got: {stored_urls:?}"
    );
}

/// #1116: invalid URL is unrepresentable at the `McpUrl` boundary.
#[test]
fn discover_sitemap_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<DiscoverUrlsParams>(serde_json::json!({
        "url": "not a url"
    }));
    assert!(
        res.is_err(),
        "invalid URL must fail to deserialize: {res:?}"
    );
}
