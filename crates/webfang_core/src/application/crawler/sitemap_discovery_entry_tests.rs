use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Issue #1162/#1190: rejection end-to-end via discovery ──
// These replace the isolated-enum `issue_1162_*` tests (which passed
// while production still failed late): each drives a real discovery
// entry, so a regression reintroduces the late failure — not a unit
// miss (systemic-issue-triage: trust the reproduction over the
// mechanism).

/// `Some("not-a-url")` is rejected by the legacy entry BEFORE any
/// client build or fetch (Spanish, typed) — no mock needed because
/// the network is never touched.
#[tokio::test]
async fn issue_1162_invalid_url_rejected_before_fetch() {
    let seed = Url::parse("https://example.com").expect("valid seed");
    let config = CrawlerConfig::new(seed);
    let err = crawl_with_sitemap(
        "https://example.com",
        Some("not-a-url"),
        &config,
        &CorrelationId::new(),
    )
    .await
    .expect_err("invalid sitemap URL must fail at the boundary");
    match err {
        CrawlError::InvalidUrl(msg) => assert!(
            msg.contains("sitemap") && msg.contains("inválida"),
            "rejection must name the sitemap URL in Spanish, got: {msg}"
        ),
        other => panic!("expected InvalidUrl, got: {other:?}"),
    }
}

/// Non-http(s) schemes are rejected at the same edge (`ValidUrl`
/// hardening applies at the boundary, not just URL shape).
#[tokio::test]
async fn issue_1162_non_http_sitemap_scheme_rejected() {
    let seed = Url::parse("https://example.com").expect("valid seed");
    let config = CrawlerConfig::new(seed);
    let err = crawl_with_sitemap(
        "https://example.com",
        Some("ftp://example.com/sitemap.xml"),
        &config,
        &CorrelationId::new(),
    )
    .await
    .expect_err("non-http(s) sitemap URL must fail at the boundary");
    assert!(
        matches!(err, CrawlError::InvalidUrl(_)),
        "expected InvalidUrl, got: {err:?}"
    );
}

/// An explicit VALID URL flows through the boundary untouched: served
/// by wiremock, the resolved entry returns exactly its `<loc>` entries.
#[tokio::test]
async fn issue_1162_explicit_valid_url_used_verbatim() {
    // Loopback mock seed: the entry guard (#1382) now cuts literal-IP
    // targets exactly as production should — these tests exercise the
    // verbatim-URL contract, not the guard, so the entry hatch is the
    // documented one-layer disarmer (same posture as the scrape-flow
    // robots tests, #1369).
    let _entry_off = webfang_test_utils::EnvGuard::entry_guard_off();
    let mock = MockServer::start().await;
    let page1 = format!("{}/page1", mock.uri());
    let page2 = format!("{}/page2", mock.uri());
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{page1}</loc></url>
    <url><loc>{page2}</loc></url>
</urlset>"#
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(xml)
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let base_url = mock.uri();
    let seed = Url::parse(&base_url).expect("valid mock URL");
    let config = CrawlerConfig::new(seed);
    let sitemap_url = format!("{base_url}/sitemap.xml");
    let explicit = ValidUrl::parse(&sitemap_url).expect("explicit test URL is valid");

    let urls =
        crawl_with_sitemap_resolved(&base_url, Some(&explicit), &config, &CorrelationId::new())
            .await
            .expect("explicit sitemap must parse");

    assert_eq!(urls.len(), 2, "expected both <loc> entries");
    let found: Vec<String> = urls.into_iter().map(|d| d.url.to_string()).collect();
    assert!(found.contains(&page1), "should contain page1");
    assert!(found.contains(&page2), "should contain page2");
}

/// No explicit URL auto-discovers through the resolved entry
/// (robots.txt `Sitemap:` directive → sitemap fetch → URLs).
#[tokio::test]
async fn issue_1162_auto_discovery_without_url() {
    // Loopback mock seed — same entry-hatch posture as the verbatim test
    // above (#1382/#1369): the contract under test is the discovery
    // chain, not the guard.
    let _entry_off = webfang_test_utils::EnvGuard::entry_guard_off();
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("User-agent: *\nDisallow:\nSitemap: /sitemap.xml\n"),
        )
        .mount(&mock)
        .await;
    let page1 = format!("{}/page1", mock.uri());
    let page2 = format!("{}/page2", mock.uri());
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{page1}</loc></url>
    <url><loc>{page2}</loc></url>
</urlset>"#
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(xml)
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let base_url = mock.uri();
    let seed = Url::parse(&base_url).expect("valid mock URL");
    let config = CrawlerConfig::new(seed);

    let urls = crawl_with_sitemap_resolved(&base_url, None, &config, &CorrelationId::new())
        .await
        .expect("auto-discovery should succeed");

    assert_eq!(urls.len(), 2, "expected both auto-discovered URLs");
}
