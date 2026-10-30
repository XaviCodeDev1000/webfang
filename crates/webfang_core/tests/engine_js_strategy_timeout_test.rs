#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Engine-level regression test for issue #280.
//!
//! `Engine::with_js_strategy()` previously built its `WreqDownloader` with a
//! hardcoded 30s timeout, ignoring `CrawlerConfig::timeout_secs`. This test
//! drives the public `crawl_site_with_options` API — the only entry point that
//! applies `with_js_strategy` — against a slow endpoint and pins the property:
//! the crawl exhausts both attempts at the configured deadline (observable as
//! exactly 2 requests and zero crawled pages) instead of hanging on the
//! hardcoded default. The in-process hang-guard only bounds a hang; the
//! discriminators are the request count and the outcome (#2533 class: a
//! wall-clock assert measured the runner, not the product).
//!
//! No CLI path reaches `with_js_strategy` today (CLI crawl mode goes through
//! `scrape_flow`; batch/MCP use `crawl_site`, which leaves the fetch router
//! unset), so this API-level test is the only behavioral coverage of the fix.
//!
//! # What this test does NOT cover
//!
//! The static fallback path honours `CrawlerConfig::timeout_secs` too —
//! `fetch_url` passes it straight to the request builder — so every
//! discriminator here (2 requests, zero pages) is branch-neutral: the test
//! passes identically on both `ProductionPageFetcher` branches and **cannot
//! detect a router→fallback flip**. Verified while writing #1024: with
//! `timeout_secs(3600)` the crawl times out the same either way.
//!
//! Branch observability — that the injected `DownloaderFactory` actually routes
//! through the downloader and that the fallback fabricates `status: 200` and drops
//! cookies — is pinned by `ProductionPageFetcher`'s tests in
//! `src/application/crawler/ports.rs` (#1024), not here.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use url::Url;
use webfang_core::application::{crawl_site_with_options, EngineOptions};
use webfang_core::domain::{CorrelationId, CrawlerConfig, JsStrategy};
use webfang_core::infrastructure::downloader::fetch_router::DefaultDownloaderFactory;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// `crawl_site_with_options` with `JsStrategy::Static` must honor
/// `CrawlerConfig::timeout_secs` on EVERY attempt: a 30s-delayed response
/// with a 2s configured timeout must exhaust both attempts at the configured
/// deadline — exactly 2 requests, zero crawled pages — and never hang on the
/// hardcoded default.
///
/// FIX-1 (#1231 F-08) made request timeouts retriable, so a fetch against a
/// slow peer is bounded by `max_retries × timeout + backoff`, not by a
/// single request. The test pins the retry budget explicitly (1 retry,
/// 10ms backoff) and asserts exactly 2 attempts — which is also the
/// discriminator against the #280 regression: a hardcoded 30s timeout
/// would yield ONE request consuming ~30s per attempt.
///
/// The hang-guard is a bound, not an assertion: 30 s is ~7x the healthy path
/// (~4.05 s = 2 × 2 s + 10 ms backoff) so a loaded runner stretches it instead
/// of failing it (#2533 class), and still below the #280 buggy regime, where
/// the two 30 s attempts complete only at ~60 s — the guard fires mid-run and
/// a regression still fails, with one request recorded.
#[tokio::test]
async fn engine_js_strategy_respects_config_timeout() {
    // Entry-guard allowance (F-06 + F-32, #1217): the engine fetches a
    // wiremock loopback literal through the production router.
    let _guard = webfang_test_utils::EnvGuard::with(&[(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    )]);
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;

    let seed: Url = format!("{}/slow", server.uri())
        .parse()
        .expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(1)
        .timeout_secs(2)
        .delay_ms(1)
        .ignore_robots(true)
        .build();

    let options = EngineOptions {
        js_strategy: JsStrategy::Static,
        ignore_robots: true,
        // Without the factory, `with_js_strategy` builds no downloader and
        // `ProductionPageFetcher` falls back to the static `fetch_url`, so the
        // configured timeout under test would never reach the wire.
        downloader_factory: Some(Arc::new(DefaultDownloaderFactory)),
        // F-08: timeouts retry. Pin the budget so exactly 2 attempts happen —
        // the request-count discriminator — independent of the hang-guard's
        // width.
        max_retries: 1,
        backoff_base_ms: 10,
        backoff_max_ms: 20,
        ..Default::default()
    };

    let result = timeout(
        Duration::from_secs(30),
        crawl_site_with_options(config, options, &CorrelationId::new()),
    )
    .await
    .expect("crawl must not hang — with_js_strategy must honor config.timeout_secs");

    // The engine must have attempted the seed fetch — and retried the
    // timeout exactly once (max_retries=1): 2 requests is the discriminator
    // against both the #280 hardcoded-30s regression (1 request) and a
    // timeout that is NOT retried (1 request, F-08 regression).
    let requests = server.received_requests().await.expect("requests recorded");
    assert_eq!(
        requests.len(),
        2,
        "2s config timeout must cap each of the 2 attempts, got {} requests",
        requests.len()
    );

    match result {
        Ok(crawl) => {
            assert_eq!(
                crawl.total_pages, 0,
                "timed-out seed must not produce crawled pages"
            );
            assert!(
                crawl.errors >= 1,
                "timed-out seed must be counted as an error"
            );
        },
        Err(e) => panic!("crawl should complete with errors, not fail outright: {e}"),
    }
}
