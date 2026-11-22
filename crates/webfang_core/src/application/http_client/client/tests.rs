use super::*;
use crate::domain::http_config::HttpClientConfig;

#[test]
fn test_http_client_creation_default() {
    let config = HttpClientConfig::default();
    let result = HttpClient::new(config);
    assert!(result.is_ok());
}

#[test]
fn test_http_client_has_user_agents() {
    let config = HttpClientConfig::default();

    assert!(
        config.max_retries > 0,
        "HttpClientConfig should have positive max_retries default"
    );
    assert!(
        config.backoff_base_ms > 0,
        "HttpClientConfig should have positive backoff_base_ms default"
    );

    let _client = HttpClient::new(config).unwrap();
}

/// An unparseable URL is rejected by `get`'s entry `Url::parse`, which
/// reports `HttpError::Request("Invalid URL: ...")`. The sibling
/// `test_url_validation_invalid_scheme` reaches the *same* variant through
/// a different arm (the http/https scheme allow-list), so the message is
/// what tells the two failure causes apart — asserting the variant alone
/// would let the two tests cover each other.
#[tokio::test]
async fn test_http_client_get_invalid_url() {
    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let result = client.get("not-a-valid-url").await;
    let err = result.expect_err("an unparseable URL must not reach the network");
    assert!(
        matches!(&err, HttpError::Request(msg) if msg.contains("Invalid URL")),
        "expected an unparseable-URL Request error, got: {err:?}"
    );
}

#[tokio::test]
async fn test_url_validation_invalid_scheme() {
    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    // `ftp://example.com` parses fine, so it is rejected one arm later, by
    // the http/https allow-list — a different message, same variant.
    let result = client.get("ftp://example.com").await;
    let err = result.expect_err("a non-http(s) scheme must not reach the network");
    assert!(
        matches!(&err, HttpError::Request(msg) if msg.contains("http or https")),
        "expected a scheme-rejection Request error, got: {err:?}"
    );
}

/// Construction smoke test: a non-default `tls_emulation` preset builds a
/// client successfully. `wreq::Client` does not expose its applied profile,
/// so the meaningful profile-mapping assertions live at the config level
/// (`http_config::profile_from_name` and `scrape_flow::build_http_client_config`).
#[test]
fn test_http_client_with_custom_tls_emulation() {
    let config = HttpClientConfig {
        tls_emulation: wreq_util::Profile::Chrome131,
        ..Default::default()
    };
    let result = HttpClient::new(config);
    assert!(result.is_ok());
}

#[test]
fn test_http_client_with_rate_limiting() {
    let config = HttpClientConfig {
        rate_limit_rpm: Some(60),
        ..Default::default()
    };
    let result = HttpClient::new(config);
    assert!(result.is_ok());
}

#[tokio::test]
#[ignore = "requires network - run with cargo test --ignored"]
async fn test_http_client_get_example_com() {
    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let result = client.get("https://example.com").await;
    assert!(result.is_ok());

    let body = result.unwrap();
    assert!(!body.is_empty());
}
