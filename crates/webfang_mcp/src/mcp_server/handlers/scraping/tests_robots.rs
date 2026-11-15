use super::tests_common::*;
use super::*;
use crate::mcp_server::handlers::test_support::{self, result_text};
use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};
// ── #749: robots.txt must be respected before any page fetch ──

/// `scrape_url` with a robots-disallowed URL returns an `isError:true`
/// result reporting `robots.txt` and never issues the page fetch.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
#[serial]
async fn scrape_url_robots_disallowed_returns_error_and_zero_page_hits() {
    // Shared robots harness (test_support): both SSRF hatches off, a
    // robots-fetcher handler, and wiremock serving the canonical
    // `/robots.txt` (`Disallow: /private`). See the fixture docs for
    // why both hatches (F-06 + F-32, #1217, #1301) and the drop-restore
    // (#1126).
    let (handler, _tmp, _guard, server) =
        test_support::test_handler_with_robots_and_no_ssrf().await;

    let res = handler
        .scrape_url(Parameters(ScrapeUrlParams {
            url: vu(&format!("{}/private/page", server.uri())),
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

/// Positive path: a robots-allowed URL passes the gate through to a real
/// scrape (fail-open positive: allowed = no interference), and the
/// extracted content reaches the caller.
#[cfg_attr(miri, ignore)] // real network stack via wreq — unsupported by Miri
#[tokio::test]
#[serial]
async fn scrape_url_robots_allowed_still_scrapes_page() {
    // Shared robots harness (test_support): both SSRF hatches off, a
    // robots-fetcher handler, and wiremock serving the canonical
    // `/robots.txt` (`Disallow: /private`). The positive path needs the
    // page route too, so it is mounted here — the fixture mounts only
    // the robots route.
    let (handler, _tmp, _guard, server) =
        test_support::test_handler_with_robots_and_no_ssrf().await;
    Mock::given(method("GET"))
        .and(path("/public/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&server)
        .await;

    let res = handler
        .scrape_url(Parameters(ScrapeUrlParams {
            url: vu(&format!("{}/public/page", server.uri())),
        }))
        .await
        .expect("handler returns Ok");

    let json = serde_json::to_value(&res).expect("CallToolResult serializes");
    assert_ne!(
        json.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "robots-allowed URL must not be rejected: {}",
        result_text(&res)
    );
    assert_eq!(page_hits(&server).await, 1, "the page must be fetched");
    assert!(
        result_text(&res).contains("Public Page"),
        "allowed scrape must return the extracted article title, got: {}",
        result_text(&res)
    );
}
