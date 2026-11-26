use super::tests_common::*;
use super::*;
use serial_test::serial;

/// WEBFANG_MCP_DISABLE_SSRF is a process-global variable. The Coverage job
/// runs raw `cargo test` (single process, parallel threads — nextest
/// isolates per process, which hides the race), so every test that reads
/// or mutates the escape hatch is `#[serial]` to keep the guard's state
/// consistent (pattern: metrics.rs).
#[cfg_attr(miri, ignore = "btls TLS_method FFI via Container::new")]
#[tokio::test]
#[serial]
async fn scrape_url_rejects_loopback() {
    // The escape hatch must be unset so the guard is active for this
    // test; EnvGuard restores the original on drop, so the removal can
    // no longer leak into sibling tests in a shared process (#1126).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
    ]);
    // SSRF protection must block requests to internal/loopback addresses
    // before any fetch happens (Bug #673).
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .scrape_url(Parameters(ScrapeUrlParams {
            url: vu("http://127.0.0.1/"),
        }))
        .await;
    assert_ssrf_rejected(res);
}

/// #1116: an invalid URL can no longer be constructed as `ScrapeUrlParams`
/// at all — `McpUrl` rejects it during deserialization, which rmcp maps
/// to JSON-RPC -32602. This replaces the old "build the param with a bad
/// URL, call the handler, expect Err" test: the invalid state is now
/// unrepresentable, so the rejection is proven at the type boundary.
#[test]
fn scrape_url_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<ScrapeUrlParams>(serde_json::json!({
        "url": "not a url"
    }));
    assert!(
        res.is_err(),
        "invalid URL must fail to deserialize: {res:?}"
    );
}

#[cfg_attr(miri, ignore = "btls TLS_method FFI via Container::new")]
#[tokio::test]
#[serial]
async fn scrape_with_options_rejects_loopback() {
    // The escape hatch must be unset so the guard is active for this
    // test; EnvGuard restores the original on drop, so the removal can
    // no longer leak into sibling tests in a shared process (#1126).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
    ]);
    // SSRF protection must block internal/loopback addresses (Bug #673).
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .scrape_with_options(Parameters(ScrapeWithOptionsParams {
            url: vu("http://127.0.0.1/"),
            max_pages: Some(1),
            download_images: Some(false),
            download_documents: Some(false),
            selector: None,
            ignore_robots: None,
        }))
        .await;
    assert_ssrf_rejected(res);
}

/// #1116: `::bad::` cannot be named as an `McpUrl` — deserialization
/// rejects it (see `scrape_url_invalid_url_is_unrepresentable`).
#[test]
fn scrape_with_options_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<ScrapeWithOptionsParams>(serde_json::json!({
        "url": "::bad::"
    }));
    assert!(
        res.is_err(),
        "invalid URL must fail to deserialize: {res:?}"
    );
}

#[cfg_attr(miri, ignore = "btls TLS_method FFI via Container::new")]
#[tokio::test]
#[serial]
async fn scrape_batch_rejects_loopback() {
    // The escape hatch must be unset so the guard is active for this
    // test; EnvGuard restores the original on drop, so the removal can
    // no longer leak into sibling tests in a shared process (#1126).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
    ]);
    // SSRF protection must block internal/loopback addresses in a batch (Bug #673).
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .scrape_batch(Parameters(ScrapeBatchParams {
            urls: vec![vu("http://127.0.0.1/")],
            concurrency: Some(2),
            ignore_robots: None,
            single_page: None,
            delay_ms: None,
        }))
        .await;
    assert_ssrf_rejected(res);
}

/// #1116: a batch element that is not a valid URL cannot be constructed
/// — the whole `ScrapeBatchParams` fails to deserialize.
#[test]
fn scrape_batch_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<ScrapeBatchParams>(serde_json::json!({
        "urls": ["not a url"]
    }));
    assert!(
        res.is_err(),
        "invalid batch URL must fail to deserialize: {res:?}"
    );
}

#[test]
fn zero_limits_are_rejected() {
    // Bug #597: concurrency:0 deadlocks buffer_unordered(0).
    // Bug #598: max_pages:0 panics mpsc::channel(0).
    // Both must be rejected up front with invalid_params.
    let batch = ScrapeBatchParams {
        urls: vec![vu("https://example.com")],
        concurrency: Some(0),
        ignore_robots: None,
        single_page: None,
        delay_ms: None,
    }
    .validate();
    assert!(batch.is_err(), "concurrency 0 must be rejected: {batch:?}");

    let crawl = CrawlSiteParams {
        url: vu("https://example.com"),
        max_depth: None,
        max_pages: Some(0),
        js_strategy: None,
        session_pool: None,
        checkpoint_dir: None,
    }
    .validate();
    assert!(crawl.is_err(), "max_pages 0 must be rejected: {crawl:?}");
}

#[cfg_attr(miri, ignore = "btls TLS_method FFI via Container::new")]
#[tokio::test]
#[serial]
async fn discover_urls_rejects_loopback() {
    // The escape hatch must be unset so the guard is active for this
    // test; EnvGuard restores the original on drop, so the removal can
    // no longer leak into sibling tests in a shared process (#1126).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
    ]);
    // SSRF protection must block internal/loopback addresses (Bug #673).
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .discover_urls(Parameters(DiscoverUrlsParams {
            url: vu("http://127.0.0.1/"),
        }))
        .await;
    assert_ssrf_rejected(res);
}

/// #1116: invalid URL is unrepresentable at the `McpUrl` boundary.
#[test]
fn discover_urls_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<DiscoverUrlsParams>(serde_json::json!({
        "url": "not a url"
    }));
    assert!(
        res.is_err(),
        "invalid URL must fail to deserialize: {res:?}"
    );
}

#[cfg_attr(miri, ignore = "btls TLS_method FFI via Container::new")]
#[tokio::test]
#[serial]
async fn detect_spa_rejects_loopback() {
    // The escape hatch must be unset so the guard is active for this
    // test; EnvGuard restores the original on drop, so the removal can
    // no longer leak into sibling tests in a shared process (#1126).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
    ]);
    // SSRF protection must block internal/loopback addresses (Bug #673).
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .detect_spa(Parameters(DetectSpaParams {
            url: vu("http://127.0.0.1/"),
        }))
        .await;
    assert_ssrf_rejected(res);
}

/// #1116: invalid URL is unrepresentable at the `McpUrl` boundary.
#[test]
fn detect_spa_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<DetectSpaParams>(serde_json::json!({
        "url": "not a url"
    }));
    assert!(
        res.is_err(),
        "invalid URL must fail to deserialize: {res:?}"
    );
}
