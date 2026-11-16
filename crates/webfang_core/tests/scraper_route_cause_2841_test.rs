#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #2841 — the classified transport cause reaches the scraper route's
//! boundary, observed end to end with injected and ephemeral failures, NO
//! network.
//!
//! Three observation levels:
//!
//! - INJECTED (S12): port doubles returning the exact `HttpError` shapes the
//!   one classification seam produces — `ConnectTimeout`, `ReadTimeout` and
//!   `TlsHandshake` each walk verdict → `HttpError` → `ScraperError` and are
//!   read back with `cause_in_chain` and `classify`. These satisfy S12; no
//!   `wreq::Error` can be fabricated, which is exactly why the seam exists.
//! - TRACE (S9): the fetch stage logs through `log_scrape_error_with_cause`,
//!   so the machine-readable `cause` field the trace contract promises
//!   (#2784) is emitted on this route — it used to be omitted entirely.
//! - REAL DIAL (S10): a wiremock body that never arrives against a 1-second
//!   request timeout produces a genuine `wreq` timeout, classified
//!   `ReadTimeout` at the boundary. The https-against-plaintext handshake is
//!   deliberately INFORMATIVE: what a plaintext port yields in the connector
//!   chain varies, so only "the failure is classified, never flattened into
//!   `Request`" is pinned there.

#[path = "common/mod.rs"]
mod common;

use std::pin::Pin;
use std::time::Duration;

use common::trace_capture::capture;
use webfang_core::application::create_http_client_with_config;
use webfang_core::application::scraper_service::scrape_with_config;
use webfang_core::domain::error::cause::{cause_in_chain, ClassifiedNetworkError, NetworkCause};
use webfang_core::domain::http_config::HttpClientConfig;
use webfang_core::domain::http_error::{HttpError, HttpResult};
use webfang_core::domain::{CorrelationId, HttpClientPort, HttpResponse, ScraperConfig};
use webfang_core::error::ScraperError;

// =========================================================================
// Injected failures (S12)
// =========================================================================

/// A client whose every request fails with the error a factory produces —
/// the failure is injected, so nothing is dialed.
struct FailingClient<F>(F);

impl<F: Fn() -> HttpError + Send + Sync> HttpClientPort for FailingClient<F> {
    fn get(
        &self,
        _url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = HttpResult<HttpResponse>> + Send + '_>> {
        Box::pin(async move { Err((self.0)()) })
    }
}

/// The `Timeout` shape the seam produces for a read deadline: verdict
/// `Timeout { cause: ReadTimeout }`, cause carried as the source.
fn read_timeout_error() -> HttpError {
    HttpError::timeout_with_cause(Box::new(ClassifiedNetworkError::new(
        NetworkCause::ReadTimeout,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "read deadline",
        )),
    )))
}

/// The connect-deadline twin: `ConnectTimeout` rides the same variant.
fn connect_timeout_error() -> HttpError {
    HttpError::timeout_with_cause(Box::new(ClassifiedNetworkError::new(
        NetworkCause::ConnectTimeout,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "connect deadline",
        )),
    )))
}

/// The `Connection` shape the seam produces for a handshake failure in the
/// CONNECT phase (the connector covers DNS + TCP + TLS, so a TLS failure
/// answers `is_connect`): message names the cause, cause carried as source.
fn tls_handshake_error() -> HttpError {
    HttpError::Connection {
        message: "falló el handshake TLS: tls alert".to_string(),
        source: Some(Box::new(ClassifiedNetworkError::new(
            NetworkCause::TlsHandshake,
            Box::new(std::io::Error::other("tls alert")),
        ))),
    }
}

/// The one scrape call every test drives: the injected failure must fail the
/// scrape and return the boundary error.
async fn scrape_failing_with(client: &dyn HttpClientPort) -> ScraperError {
    let url = url::Url::parse("https://example.test/page").expect("a valid URL");
    scrape_with_config(
        client,
        &url,
        &ScraperConfig::default(),
        None,
        None,
        None,
        None,
        true,
        &CorrelationId::new(),
    )
    .await
    .expect_err("the injected failure must fail the scrape")
}

/// S12: `ReadTimeout` reaches the boundary with its classified cause.
#[tokio::test]
async fn a_read_timeout_reaches_the_boundary_with_its_cause() {
    let err = scrape_failing_with(&FailingClient(read_timeout_error)).await;
    assert_eq!(
        cause_in_chain(&err),
        Some(NetworkCause::ReadTimeout),
        "the read timeout cause must reach the boundary: {err}"
    );
    assert_eq!(
        err.classify(),
        webfang_core::error::ErrorClass::TransientRetriable,
        "the boundary's TimedOut row (rows 21/22) — the exit code (69) is unchanged"
    );
}

/// S12: `ConnectTimeout` reaches the boundary with its classified cause.
#[tokio::test]
async fn a_connect_timeout_reaches_the_boundary_with_its_cause() {
    let err = scrape_failing_with(&FailingClient(connect_timeout_error)).await;
    assert_eq!(
        cause_in_chain(&err),
        Some(NetworkCause::ConnectTimeout),
        "the connect timeout cause must reach the boundary: {err}"
    );
    assert_eq!(
        err.classify(),
        webfang_core::error::ErrorClass::TransientRetriable,
        "the boundary's TimedOut row — the class does not move with the cause"
    );
}

/// S12: `TlsHandshake` reaches the boundary with its classified cause.
#[tokio::test]
async fn a_tls_handshake_reaches_the_boundary_with_its_cause() {
    let err = scrape_failing_with(&FailingClient(tls_handshake_error)).await;
    assert_eq!(
        cause_in_chain(&err),
        Some(NetworkCause::TlsHandshake),
        "the TLS handshake cause must reach the boundary: {err}"
    );
    assert_eq!(
        err.classify(),
        webfang_core::error::ErrorClass::TransientRetriable,
        "a handshake failure is a transport failure, not a bug"
    );
}

// =========================================================================
// The fetch-stage trace event (S9)
// =========================================================================

/// S9: the fetch stage logs the classified cause — the field the trace
/// contract promises and this route used to omit entirely.
#[test]
fn the_fetch_stage_trace_event_names_the_cause() {
    let events = capture(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("the test runtime builds");
        runtime.block_on(async {
            let err = scrape_failing_with(&FailingClient(read_timeout_error)).await;
            assert_eq!(cause_in_chain(&err), Some(NetworkCause::ReadTimeout));
        });
    });

    let fetch_event = events
        .iter()
        .find(|event| event.field("stage") == Some("fetch"))
        .expect("the fetch stage must log an error event");
    assert_eq!(
        fetch_event.field("cause"),
        Some("read_timeout"),
        "the fetch-stage event must carry the machine-readable cause: {fetch_event:?}"
    );
    let message = &fetch_event.message;
    assert_eq!(message, "HTTP request failed");
}

// =========================================================================
// A real dial (S10) — wiremock, no external network
// =========================================================================

/// S10/S1: a wiremock body that never arrives against a 1-second request
/// timeout — a genuine `wreq` timeout, classified `ReadTimeout` at the
/// boundary. `max_retries: 0` so the exhaustion is the first deadline.
#[tokio::test]
async fn a_real_dial_read_timeout_reaches_the_boundary_with_its_cause() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("late")
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;

    let config = HttpClientConfig {
        timeout_secs: 1,
        connect_timeout_secs: 1,
        max_retries: 0,
        ..HttpClientConfig::default()
    };
    let client = create_http_client_with_config(&config).expect("the client builds");
    let url = url::Url::parse(&format!("{}/slow", server.uri())).expect("a valid URL");

    let err = scrape_with_config(
        &client,
        &url,
        &ScraperConfig::default(),
        None,
        None,
        None,
        None,
        true,
        &CorrelationId::new(),
    )
    .await
    .expect_err("the deadline must fail the scrape");

    assert_eq!(
        cause_in_chain(&err),
        Some(NetworkCause::ReadTimeout),
        "the real dial's read timeout must reach the boundary classified: {err}"
    );
}

/// S10, INFORMATIVE: an `https://` URL against a plaintext wiremock port
/// fails in the TLS handshake. What the connector chain yields there varies
/// (different platforms and `wreq` versions report different alerts), so the
/// exact cause is NOT pinned — the criterion is satisfied by the injected
/// TLS test above. What IS pinned: the failure is classified (some cause in
/// the chain), never flattened into an unclassified `Request`.
#[tokio::test]
async fn an_https_handshake_against_a_plaintext_port_stays_classified() {
    let server = wiremock::MockServer::start().await;
    let url = url::Url::parse(&format!(
        "https://{}/page",
        server.uri().replace("http://", "")
    ))
    .expect("a valid URL");
    let client =
        create_http_client_with_config(&HttpClientConfig::default()).expect("the client builds");

    let err = scrape_with_config(
        &client,
        &url,
        &ScraperConfig::default(),
        None,
        None,
        None,
        None,
        true,
        &CorrelationId::new(),
    )
    .await
    .expect_err("the handshake against a plaintext port must fail");

    assert!(
        cause_in_chain(&err).is_some(),
        "the handshake failure must stay classified, not flattened: {err}"
    );
}
