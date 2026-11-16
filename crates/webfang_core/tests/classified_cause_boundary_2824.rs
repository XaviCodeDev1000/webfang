#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #2824: the classified transport cause reaches the application boundary.
//!
//! Two application paths, two injected failures, no network:
//!
//! - BATCH: the batch reports one aggregate error for a run whose pages all
//!   failed. The aggregate used to carry NO cause (`source: None`), so the
//!   operator read "crawl completed with N transient error(s)" where the
//!   transport had already classified "the server refused the connection".
//!   The failure is injected with a loopback port nobody listens on: the
//!   connect is refused, deterministically, without leaving the host.
//! - SCRAPER: the scrape path reports the classified cause it was given.
//!
//! The cause is read with `cause_in_chain` — the same walk the trace and
//! `CrawlErrorCategory` use — so a consumer that never learned the
//! `io::Error` payload quirk still reaches the typed cause.

use std::net::TcpListener;
use std::pin::Pin;

use webfang_core::application::batch::{BatchJob, BatchProcessor};
use webfang_core::application::scraper_service::scrape_with_config;
use webfang_core::domain::error::cause::{cause_in_chain, ClassifiedNetworkError, NetworkCause};
use webfang_core::domain::http_error::{HttpError, HttpResult};
use webfang_core::domain::{
    CorrelationId, CrawlerConfig, HttpClientPort, HttpResponse, ScraperConfig,
};

/// The SSRF guard refuses literal loopback IPs — the production default. The
/// batch's crawl of an injected failure needs the dial to happen, so both
/// hatches are cleared for the length of the test (the same pair
/// `engine_options_1369` uses for its wiremock loopbacks).
const SSRF_BYPASS: [(&str, &str); 2] = [
    ("WEBFANG_DISABLE_SSRF_ENTRY_GUARD", "1"),
    ("WEBFANG_DISABLE_SSRF_RESOLVER", "1"),
];

/// A loopback port with nothing listening on it.
///
/// The listener is bound to learn a free port and dropped immediately: the
/// connect that follows is refused by the kernel, with no server and no
/// packet leaving the host.
fn closed_loopback_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free loopback port");
    let port = listener
        .local_addr()
        .expect("the bound loopback address")
        .port();
    drop(listener);
    port
}

/// The batch's aggregate error carries the cause the transport classified.
#[tokio::test]
async fn the_batch_error_carries_the_classified_cause() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);

    let port = closed_loopback_port();
    let url = format!("http://127.0.0.1:{port}/");
    let seed = url::Url::parse(&url).expect("the loopback URL must parse");

    let processor = BatchProcessor::new(1).expect("a valid concurrency limit");
    let job = BatchJob::new(
        "classified-cause".to_string(),
        vec![url],
        CrawlerConfig::builder(seed)
            .max_depth(0)
            .max_pages(1)
            .ignore_robots(true)
            .timeout_secs(5)
            .build(),
    );

    let result = processor
        .process_batch(job)
        .await
        .expect("the batch dispatch itself must succeed");
    assert_eq!(result.failed, 1, "the refused URL must be a failure");

    let (_, error) = result.errors.first().expect("the failure must be reported");
    assert_eq!(
        cause_in_chain(error),
        Some(NetworkCause::ConnectRefused),
        "the batch error must carry the classified cause: {error}"
    );
}

/// An HTTP client whose every request fails with a classified connection
/// error — the failure is injected, so nothing is dialed.
struct RefusedClient;

impl HttpClientPort for RefusedClient {
    fn get(
        &self,
        _url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = HttpResult<HttpResponse>> + Send + '_>> {
        Box::pin(async move {
            Err(HttpError::Connection {
                message: "el servidor rechazó la conexión: connection refused".to_string(),
                source: Some(Box::new(ClassifiedNetworkError::new(
                    NetworkCause::ConnectRefused,
                    Box::new(std::io::Error::other("connection refused")),
                ))),
            })
        })
    }
}

/// The scrape path reports the classified cause it was given.
#[tokio::test]
async fn the_scrape_error_carries_the_classified_cause() {
    let url = url::Url::parse("https://example.test/page").expect("a valid URL");
    let config = ScraperConfig::default();
    let correlation = CorrelationId::new();

    let error = scrape_with_config(
        &RefusedClient,
        &url,
        &config,
        None,
        None,
        None,
        None,
        true,
        &correlation,
    )
    .await
    .expect_err("the refused fetch must fail the scrape");

    assert_eq!(
        cause_in_chain(&error),
        Some(NetworkCause::ConnectRefused),
        "the scrape error must carry the classified cause: {error}"
    );
}
