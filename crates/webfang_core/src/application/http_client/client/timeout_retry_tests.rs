//! The timeout-retry tests of the hardened client (F-08 parity), moved
//! out of the grandfathered-size `client.rs`: a pure move, no behaviour
//! change. The module carries its own `#[cfg(not(miri))]` gate.

//! F-08/F-09 retry-parity contract tests for the hardened Stack B client
//! (RC-1 slice 5): request timeouts are RETRIED within the retry budget and
//! recovery is served — mirroring Stack A's `fetch_inner` tests
//! (`wreq_downloader.rs`: `timeout_is_retried_and_recovery_is_served`).
//! Builder/scheme errors were already terminal pre-dial (`get` validates
//! the scheme before any request), so no test can dial them.

use super::arrange::*;
use super::*;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Timeout that a 1500 ms delayed wiremock response reliably trips.
const TIMEOUT_SECS: u64 = 1;
/// First hit trips the timeout, later hits answer instantly.
const TRIP_DELAY_MS: u64 = 1500;

fn parity_config(max_retries: u32) -> HttpClientConfig {
    HttpClientConfig {
        timeout_secs: TIMEOUT_SECS,
        // Short backoff keeps the retry path exercised but the test fast.
        ..retry_config(max_retries)
    }
}

async fn count_requests(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("received requests are recorded")
        .len()
}

/// F-08 (initial send): a request that times out is RETRIED and recovery
/// is served. Exactly one re-request after the timeout — the parity twin
/// of Stack A's `timeout_is_retried_and_recovery_is_served`.
#[tokio::test]
async fn timeout_is_retried_and_recovery_is_served() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("late")
                .set_delay(Duration::from_millis(TRIP_DELAY_MS)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_body_string("fast"))
        .mount(&server)
        .await;

    let client = HttpClient::new(parity_config(1)).expect("client builds");
    let body = client
        .get(&format!("{}/slow", server.uri()))
        .await
        .expect("retry after initial-send timeout must recover");
    assert_eq!(body, "fast");
    assert_eq!(
        count_requests(&server).await,
        2,
        "initial-send timeout retry must produce exactly one re-request"
    );
}

/// F-08 (inside the 429 retry loop): a timeout during `retry_with_backoff`
/// consumes the attempt and the loop continues — it does not surface
/// early. 429 → timeout → fast recovery: exactly 3 outbound requests.
#[tokio::test]
async fn timeout_inside_429_retry_loop_is_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("late")
                .set_delay(Duration::from_millis(TRIP_DELAY_MS)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_body_string("fast"))
        .mount(&server)
        .await;

    let client = HttpClient::new(parity_config(2)).expect("client builds");
    let body = client
        .get(&format!("{}/slow", server.uri()))
        .await
        .expect("timeout inside the 429 retry loop must not abort the budget");
    assert_eq!(body, "fast");
    assert_eq!(
        count_requests(&server).await,
        3,
        "429 + timeout + recovery = exactly three outbound requests"
    );
}

/// F-08 (exhaustion): when every attempt times out, `HttpError::Timeout`
/// still surfaces — after the FULL retry budget, not after the first hit.
#[tokio::test]
async fn timeout_exhaustion_returns_timeout_after_full_budget() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("late")
                .set_delay(Duration::from_millis(TRIP_DELAY_MS)),
        )
        .mount(&server)
        .await;

    let client = HttpClient::new(parity_config(1)).expect("client builds");
    let result = client.get(&format!("{}/slow", server.uri())).await;
    assert!(
        matches!(result, Err(HttpError::Timeout { .. })),
        "exhausted timeouts must surface as Timeout, got: {result:?}"
    );
    assert_eq!(
        count_requests(&server).await,
        2,
        "initial attempt + max_retries(1) = exactly two attempts"
    );
}
