use super::arrange::*;
use super::*;
use crate::domain::http_config::HttpClientConfig;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_403_returns_error() {
    let mock_server = MockServer::start().await;
    mount_status(&mock_server, "/", 403).await;

    let client = retry_client(1);

    let result = get_route(&client, &mock_server, "/").await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), HttpError::Forbidden));
}

#[tokio::test]
async fn test_429_returns_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
        .mount(&mock_server)
        .await;

    let client = retry_client(1);

    let start = std::time::Instant::now();
    let result = get_route(&client, &mock_server, "/").await;
    let elapsed = start.elapsed();

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), HttpError::RateLimited(_)));
    assert!(elapsed.as_millis() >= 10);
}

#[tokio::test]
async fn test_500_returns_error() {
    let mock_server = MockServer::start().await;
    mount_status(&mock_server, "/", 500).await;

    let client = retry_client(1);

    let result = get_route(&client, &mock_server, "/").await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), HttpError::ServerError(500)));
}

#[tokio::test]
async fn test_500_exhausts_retries() {
    let mock_server = MockServer::start().await;
    mount_status(&mock_server, "/", 500).await;

    let client = retry_client(2);

    let result = get_route(&client, &mock_server, "/").await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), HttpError::ServerError(500)));
}

#[tokio::test]
async fn test_404_returns_client_error() {
    let mock_server = MockServer::start().await;
    mount_status(&mock_server, "/notfound", 404).await;

    let client = default_client();

    let result = get_route(&client, &mock_server, "/notfound").await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), HttpError::ClientError(404)));
}

#[tokio::test]
async fn test_200_returns_body() {
    let mock_server = MockServer::start().await;

    let expected_body = "<html><body>Hello World</body></html>";
    mount_body(&mock_server, "/", 200, expected_body).await;

    let client = default_client();

    let result = get_route(&client, &mock_server, "/").await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), expected_body);
}

/// `user_agent: Some(..)` in `HttpClientConfig` must reach the wire as a
/// `user-agent` header.
///
/// `wreq::Client` does not expose its applied profile or default headers,
/// so we verify at the wire: the mock only answers 200 when the request
/// carries the exact `user-agent` we configured, and `.expect(1)` plus
/// `server.verify()` prove the header matched exactly once. Real network
/// I/O to localhost, hence not(miri).
#[tokio::test]
#[cfg(not(miri))]
async fn test_custom_user_agent_reaches_wire() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("user-agent", "custom-ua-test"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;

    let config = HttpClientConfig {
        user_agent: Some("custom-ua-test".to_string()),
        ..Default::default()
    };
    let client = create_http_client_with_config(&config).unwrap();

    let response = client.get(server.uri()).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);

    server.verify().await;
}
