use super::{HttpClient, HttpError};
use crate::domain::http_config::HttpClientConfig;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Fast retry preset: short backoff keeps the retry path exercised but
/// the test fast. Mirrors the `parity_config` shape minus its timeout.
pub(super) fn retry_config(max_retries: u32) -> HttpClientConfig {
    HttpClientConfig {
        max_retries,
        backoff_base_ms: 10,
        backoff_max_ms: 50,
        ..Default::default()
    }
}

/// Client with default config for status/body tests that never retry.
pub(super) fn default_client() -> HttpClient {
    HttpClient::new(HttpClientConfig::default()).expect("client builds")
}

/// Client with the fast retry preset.
pub(super) fn retry_client(max_retries: u32) -> HttpClient {
    HttpClient::new(retry_config(max_retries)).expect("client builds")
}

/// Mount a `GET {route}` mock answering `status` with an empty body.
pub(super) async fn mount_status(server: &MockServer, route: &str, status: u16) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

/// Mount a `GET {route}` mock answering `status` with `body`.
pub(super) async fn mount_body(server: &MockServer, route: &str, status: u16, body: &str) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

/// GET `route` against `server` through `client`.
pub(super) async fn get_route(
    client: &HttpClient,
    server: &MockServer,
    route: &str,
) -> Result<String, HttpError> {
    client.get(&format!("{}{route}", server.uri())).await
}
