#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Integration test: proxy posture — env proxies are ignored by design (#2500).
//!
//! WebFang's egress is direct-only. The validating SSRF resolver must resolve
//! the real destination; routing through an ambient `HTTP_PROXY` would resolve
//! the proxy instead, silently reject RFC1918 corporate proxies at the guard,
//! and delegate destination IP checks to the proxy. These tests pin that every
//! fetch path ignores the proxy environment variables.

#[path = "common/cli_harness.rs"]
mod common;

use common::BehavioralTest;
use url::Url;
use webfang_core::HttpClientConfig;
use webfang_core::infrastructure::downloader::wreq_downloader::WreqDownloader;
use webfang_core::infrastructure::downloader::Downloader;
use webfang_test_utils::EnvGuard;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Proxy env vars that must never influence a WebFang fetch. `HTTP_PROXY` is
/// set by the tests themselves; the rest are scrubbed so an ambient value on
/// the developer machine cannot flip the outcome either way.
const SCRUBBED_PROXY_VARS: &[&str] = &[
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
];

async fn mount_ok(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("<html><body>ok</body></html>", "text/html; charset=utf-8"),
        )
        .mount(server)
        .await;
}

/// The scrape client ignores `HTTP_PROXY`: the fetch reaches the target
/// directly and the proxy receives nothing. RED while clients inherit wreq's
/// default `auto_sys_proxy` (the request is observable at the proxy instead).
#[tokio::test]
#[cfg(not(miri))]
async fn scrape_client_ignores_env_proxy() {
    let target = MockServer::start().await;
    let proxy = MockServer::start().await;
    mount_ok(&target).await;

    let mut env = EnvGuard::with(&[("HTTP_PROXY", proxy.uri().as_str())]);
    for var in SCRUBBED_PROXY_VARS {
        env.remove(var);
    }

    let downloader: Box<dyn Downloader> = Box::new(
        WreqDownloader::new(
            10,
            5,
            HttpClientConfig::default().tls_emulation,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            50_000_000,
        )
        .expect("downloader builds"),
    );
    let url = Url::parse(&target.uri()).expect("wiremock URI is valid");

    let page = downloader
        .fetch(&url)
        .await
        .expect("direct fetch from wiremock should succeed");
    assert_eq!(page.status, 200, "fetch must reach the target, not the proxy");

    let proxy_requests = proxy.received_requests().await.unwrap_or_default();
    assert!(
        proxy_requests.is_empty(),
        "env proxy must be ignored, but the proxy observed {} request(s)",
        proxy_requests.len()
    );
}

/// Same contract at the CLI boundary: a scrape with `HTTP_PROXY` in the
/// environment succeeds through a direct dial and the proxy observes nothing.
#[tokio::test]
#[cfg(not(miri))]
async fn cli_scrape_ignores_env_proxy() {
    let harness = BehavioralTest::new().await;
    let proxy = MockServer::start().await;
    mount_ok(&harness.server).await;

    let mut cmd = harness.scraper_cmd();
    cmd.env("HTTP_PROXY", proxy.uri());
    for var in SCRUBBED_PROXY_VARS {
        cmd.env_remove(var);
    }

    cmd.assert().success();
    assert!(
        !harness.find_files("md").is_empty(),
        "scrape must produce markdown output"
    );

    let proxy_requests = proxy.received_requests().await.unwrap_or_default();
    assert!(
        proxy_requests.is_empty(),
        "env proxy must be ignored, but the proxy observed {} request(s)",
        proxy_requests.len()
    );
}
