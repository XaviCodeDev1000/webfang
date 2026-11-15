use super::tests_common::*;
use super::*;
use crate::mcp_server::handlers::test_support::{self, result_text};
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
/// PI-13 (#1601): `discover_urls` must apply the same SSRF/discovery
/// filter as `crawl_site` — external-domain and forbidden-literal-IP
/// links extracted from the page never reach the MCP response.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
#[serial]
async fn discover_urls_drops_external_and_literal_links() {
    // Lift both guards for this test only (wiremock binds 127.0.0.1):
    // the MCP entry validator and the shared core literal-IP entry
    // guard (F-06 + F-32, #1217).
    let _guard = webfang_test_utils::EnvGuard::ssrf_hatches_off();
    let (handler, _tmp) = test_handler().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<html><body>
                <a href="/internal-page">Internal</a>
                <a href="https://other.com/x">External</a>
                <a href="http://10.0.0.5/x">Literal</a>
            </body></html>"#,
        ))
        .mount(&server)
        .await;

    let res = handler
        .discover_urls(Parameters(DiscoverUrlsParams {
            url: vu(&format!("{}/", server.uri())),
        }))
        .await
        .expect("discover_urls returns Ok");

    let raw = result_text(&res);
    let payload = crate::mcp_server::provenance::payload_of(&raw)
        .expect("discover_urls response must carry the UNTRUSTED envelope");
    assert!(
        payload.contains("/internal-page"),
        "seed-host link must survive the filter: {payload}"
    );
    assert!(
        !payload.contains("other.com") && !payload.contains("10.0.0.5"),
        "external-domain and forbidden-literal links must be dropped: {payload}"
    );
}

/// PI-13 (#1601): `discover_sitemap` must apply the same SSRF/discovery
/// filter as `crawl_site` — external `<loc>` entries never reach the
/// MCP response. The engine's sitemap discovery is host-agnostic
/// (`CrawlerConfig::new` ships empty include patterns), so this handler
/// gate is the one that enforces the contract.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
#[serial]
async fn discover_sitemap_drops_external_and_literal_loc_entries() {
    // Same both-hatch lift as the discover_urls filter test above
    // (wiremock loopback literal, no robots chain here).
    let _guard = webfang_test_utils::EnvGuard::ssrf_hatches_off();
    let (handler, _tmp) = test_handler().await;
    let server = MockServer::start().await;
    let base = server.uri();
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
<url><loc>{base}/internal-page</loc></url>
<url><loc>https://other.com/x</loc></url>
<url><loc>http://10.0.0.5/x</loc></url>
</urlset>"#
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&server)
        .await;

    let res = handler
        .discover_sitemap(Parameters(DiscoverUrlsParams {
            url: vu(&format!("{base}/")),
        }))
        .await
        .expect("discover_sitemap returns Ok");

    let raw = result_text(&res);
    let payload = crate::mcp_server::provenance::payload_of(&raw)
        .expect("discover_sitemap response must carry the UNTRUSTED envelope");
    assert!(
        payload.contains("/internal-page"),
        "seed-host <loc> must survive the filter: {payload}"
    );
    assert!(
        !payload.contains("other.com") && !payload.contains("10.0.0.5"),
        "external-domain and forbidden-literal <loc> entries must be dropped: {payload}"
    );
}

/// `discover_urls` with a robots-disallowed URL returns a `robots.txt`
/// error and never issues the page fetch.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
#[serial]
async fn discover_urls_robots_disallowed_returns_error_and_zero_page_hits() {
    // Shared robots harness (test_support): both SSRF hatches off, a
    // robots-fetcher handler, and wiremock serving the canonical
    // `/robots.txt` (`Disallow: /private`). See the fixture docs for
    // why both hatches (F-06 + F-32, #1217, #1301) and the drop-restore
    // (#1126).
    let (handler, _tmp, _guard, server) =
        test_support::test_handler_with_robots_and_no_ssrf().await;

    let res = handler
        .discover_urls(Parameters(DiscoverUrlsParams {
            url: vu(&format!("{}/private/list", server.uri())),
        }))
        .await
        .expect("handler returns Ok with an honest error");

    assert_robots_error(&res);
    assert_eq!(
        page_hits(&server).await,
        0,
        "the robots gate must short-circuit before the page fetch"
    );
}

/// `detect_spa` with a robots-disallowed URL returns a `robots.txt` error
/// and never issues the page fetch.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
#[serial]
async fn detect_spa_robots_disallowed_returns_error_and_zero_page_hits() {
    // Shared robots harness (test_support): both SSRF hatches off, a
    // robots-fetcher handler, and wiremock serving the canonical
    // `/robots.txt` (`Disallow: /private`). See the fixture docs for
    // why both hatches (F-06 + F-32, #1217, #1301) and the drop-restore
    // (#1126).
    let (handler, _tmp, _guard, server) =
        test_support::test_handler_with_robots_and_no_ssrf().await;

    let res = handler
        .detect_spa(Parameters(DetectSpaParams {
            url: vu(&format!("{}/private/app", server.uri())),
        }))
        .await
        .expect("handler returns Ok with an honest error");

    assert_robots_error(&res);
    assert_eq!(
        page_hits(&server).await,
        0,
        "the robots gate must short-circuit before the page fetch"
    );
}
