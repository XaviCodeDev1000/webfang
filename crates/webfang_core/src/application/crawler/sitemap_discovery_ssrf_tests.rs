use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── #1382: sitemap discovery SSRF entry guard (pre-socket) ──
//
// `build_discovery_client` installs SSRF layers 3 (validating resolver)
// and 4 (redirect policy) but the entry layer (`reject_forbidden_literal_url`)
// was never called on this path, so a loopback *seed* opened real sockets
// (measured: 18 requests with every guard armed, #1376/#1382). These pins
// restore the AGENTS.md guard-chain order: entry validation BEFORE the
// discovery client touches the network.
//
// Every pin below needs layer 1 ARMED, which makes them *observers* of the
// process-global environment rather than mutators of it. Taking no lock is
// not neutrality here: `ENV_LOCK` serializes only the threads that take it,
// so a sibling test inside an `entry_guard_off()` window would leave these
// asserting against a DISARMED guard — the RFC1918 literal would pass
// through and the downloader would dial it, failing with `Http { Connect }`
// where the contract says `CrawlError::InvalidUrl`. That is #1788, and it
// reproduces under libtest (one process, N threads — what the `Coverage`
// job runs, no `--nextest`) rather than under nextest (one process per
// test, so the window never coincides). Hence `EnvGuard::entry_guard_on()`:
// it excludes the disarm window for its whole lifetime and mutates nothing.

/// A loopback seed is rejected at entry, pre-socket: the run fails with
/// the typed `CrawlError::InvalidUrl` carrying the Spanish SSRF message,
/// and the mock — the only listener for the seed's host — receives zero
/// requests of any kind (no robots.txt GET, no probes, nothing).
#[tokio::test]
async fn sitemap_discovery_rejects_loopback_seed_pre_socket() {
    // Layer 1 is the SUBJECT: it must be armed, and observing that
    // requires excluding every sibling's disarm window (#1788).
    let _entry_on = webfang_test_utils::EnvGuard::entry_guard_on();
    let mock = MockServer::start().await;
    // Tripwire: any request that escapes the guard lands here and fails
    // the zero-request assertion.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
        .mount(&mock)
        .await;

    let base_url = mock.uri();
    let seed = Url::parse(&base_url).expect("mock URL must parse");
    let config = CrawlerConfig::new(seed);

    let err = crawl_with_sitemap_resolved(&base_url, None, &config, &CorrelationId::new())
        .await
        .expect_err("loopback seed must be rejected at the entry guard");

    match err {
        CrawlError::InvalidUrl(msg) => assert!(
            msg.contains("SSRF detectado"),
            "rejection must carry the Spanish SSRF message, got: {msg}"
        ),
        other => panic!("expected CrawlError::InvalidUrl, got: {other:?}"),
    }
    let seen = mock.received_requests().await.unwrap_or_default();
    assert!(
        seen.is_empty(),
        "rejection is pre-socket: the mock must observe zero requests, got {seen:?}"
    );
}

/// An explicit sitemap URL whose host is a forbidden literal is rejected
/// at entry even when the seed host itself is fetchable text: the guard
/// applies to every URL this path would fetch, not only the seed. The
/// seed is never fetched by sitemap discovery, so a hostname seed keeps
/// this test focused on the sitemap target alone.
#[tokio::test]
async fn sitemap_discovery_rejects_forbidden_literal_sitemap_url() {
    let _entry_on = webfang_test_utils::EnvGuard::entry_guard_on();
    // Hostname seed: sitemap discovery never fetches the seed itself, and
    // the entry guard lets hostnames through (that is layer-3 territory).
    let seed = Url::parse("https://example.com").expect("hostname seed");
    let config = CrawlerConfig::new(seed);
    // Forbidden literal, RFC1918. Nothing listens there and nothing may
    // ever dial it — the assertion is the typed rejection; the address is
    // unroutable in the test environment, so a followed fetch could never
    // succeed anyway.
    let sitemap_url =
        ValidUrl::parse("http://192.168.1.5:59999/sitemap.xml").expect("literal parses");

    let err = crawl_with_sitemap_resolved(
        "https://example.com",
        Some(&sitemap_url),
        &config,
        &CorrelationId::new(),
    )
    .await
    .expect_err("forbidden-literal sitemap URL must be rejected at entry");

    match err {
        CrawlError::InvalidUrl(msg) => assert!(
            msg.contains("SSRF detectado") && msg.contains("192.168.1.5"),
            "rejection must name the offending literal in Spanish, got: {msg}"
        ),
        other => panic!("expected CrawlError::InvalidUrl, got: {other:?}"),
    }
}

/// A robots.txt `Sitemap:` directive pointing at a forbidden literal is
/// rejected before the sitemap fetch: robots.txt itself is fetched (one
/// request, the discovery chain's legitimate first hop), but the pointed
/// sitemap URL is never dialed. The seed uses the `localhost` hostname —
/// the entry layer correctly lets hostnames through (#1217 layer
/// separation) — with the resolver hatch set so the loopback answer is
/// dialable by the harness. Layer 1 (the subject under test) stays ARMED:
/// the mock's one-request journal plus the typed rejection pin that the
/// directive target was cut before its socket existed.
#[tokio::test]
async fn sitemap_discovery_rejects_robots_directive_pointing_at_literal() {
    // Resolver-only hatch: `localhost` resolves to 127.0.0.1, a forbidden
    // answer for layer 3. Layer 1 (literal guard) stays armed — it is the
    // layer under test. ONE guard, not two: `ENV_LOCK` is not reentrant,
    // so an armed-guard observer stacked on an `EnvGuard::with` mutator
    // would self-deadlock (#1788, #1224). The combined constructor keeps a
    // single lock holder and leaves the hatch names in this one module.
    let _entry_on = webfang_test_utils::EnvGuard::entry_guard_on_resolver_off();
    // `localhost` + the mock's port: the same wiremock listener, spelled
    // as a hostname so the entry layer is not the thing that fires on the
    // seed itself.
    let mock = MockServer::start().await;
    let base_url = format!("http://localhost:{}/", mock.address().port());

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("User-agent: *\nSitemap: http://10.9.8.7:9/sitemap.xml\n"),
        )
        .mount(&mock)
        .await;

    let seed = Url::parse(&base_url).expect("localhost seed must parse");
    let config = CrawlerConfig::new(seed);

    let err = crawl_with_sitemap_resolved(&base_url, None, &config, &CorrelationId::new())
        .await
        .expect_err("robots directive at a forbidden literal must be rejected");

    match err {
        CrawlError::InvalidUrl(msg) => assert!(
            msg.contains("SSRF detectado") && msg.contains("10.9.8.7"),
            "rejection must name the directive's forbidden literal, got: {msg}"
        ),
        other => panic!("expected CrawlError::InvalidUrl, got: {other:?}"),
    }
    let seen = mock.received_requests().await.unwrap_or_default();
    assert_eq!(
        seen.len(),
        1,
        "only the robots.txt fetch may reach the mock (the legitimate \
             first hop); the directive target must never be dialed, got {seen:?}"
    );
}

/// A sitemap index whose `<sitemap>` child points at a forbidden literal
/// is stopped by the parser's per-URL entry guard: the child URL is
/// never dialed (pre-socket) and the typed SSRF rejection surfaces
/// instead of a fetch error. The seed uses the `localhost` hostname with
/// the resolver-only hatch so the index itself is fetchable while the
/// child literal — an unroutable RFC1918 address — is cut by the armed
/// entry layer before its socket exists.
#[tokio::test]
async fn sitemap_discovery_rejects_index_child_pointing_at_literal() {
    // Same single-holder constraint as the robots-directive pin above: the
    // entry guard must stay ARMED (it is the layer under test) and the
    // resolver hatch must be lifted for the loopback index, so both facts
    // are established by ONE non-reentrant-lock holder (#1788).
    let _entry_on = webfang_test_utils::EnvGuard::entry_guard_on_resolver_off();
    let mock = MockServer::start().await;
    let base_url = format!("http://localhost:{}/", mock.address().port());

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nSitemap: /sitemap.xml\n"),
        )
        .mount(&mock)
        .await;
    let index_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap><loc>http://192.168.7.7:9/child-sitemap.xml</loc></sitemap>
</sitemapindex>"#;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_xml)
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let seed = Url::parse(&base_url).expect("localhost seed must parse");
    let config = CrawlerConfig::new(seed);

    let err = crawl_with_sitemap_resolved(&base_url, None, &config, &CorrelationId::new())
        .await
        .expect_err("index child at a forbidden literal must be rejected");

    match err {
        CrawlError::InvalidUrl(msg) => assert!(
            msg.contains("SSRF detectado") && msg.contains("192.168.7.7"),
            "rejection must name the child's forbidden literal, got: {msg}"
        ),
        other => panic!("expected CrawlError::InvalidUrl, got: {other:?}"),
    }
}
