use super::arrange::*;
use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Install the real WAF inspector on first test access (#996). Idempotent.
fn ensure_waf_inspector() {
    use crate::domain::waf::set_waf_inspector;
    use crate::infrastructure::http::waf_engine::WafInspector;
    use std::sync::{Arc, OnceLock};
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        set_waf_inspector(Arc::new(WafInspector) as Arc<dyn crate::domain::waf::WafInspectorPort>);
    });
}

#[tokio::test]
async fn test_200_cloudflare_challenge_returns_waf_error() {
    ensure_waf_inspector();
    let mock_server = MockServer::start().await;

    let challenge_body = r#"<html><head><title>Just a moment...</title></head>
    <body><div id="challenge-running">Checking your browser...</div></body></html>"#;
    mount_body(&mock_server, "/", 200, challenge_body).await;

    let client = retry_client(1);

    let result = get_route(&client, &mock_server, "/").await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), HttpError::WafChallenge(_)));
}

#[tokio::test]
async fn test_200_recaptcha_challenge_returns_waf_error() {
    ensure_waf_inspector();
    let mock_server = MockServer::start().await;

    let challenge_body =
        r#"<html><body><div class="g-recaptcha" data-sitekey="abc"></div></body></html>"#;
    mount_body(&mock_server, "/", 200, challenge_body).await;

    let client = retry_client(1);

    let result = get_route(&client, &mock_server, "/").await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), HttpError::WafChallenge(_)));
}

#[tokio::test]
async fn test_200_normal_page_returns_body() {
    let mock_server = MockServer::start().await;

    let normal_body =
        "<html><body><article><h1>Real Content</h1><p>Normal page.</p></article></body></html>";
    mount_body(&mock_server, "/", 200, normal_body).await;

    let client = default_client();

    let result = get_route(&client, &mock_server, "/").await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), normal_body);
}

// ========================================================================
// TASK-10 — context-aware inspect() in the 2xx + 5xx branches (REQ-WAF-05/07)
// ========================================================================

/// Approved behavior change: a 503 Cloudflare challenge is classified as a
/// WAF block (WafChallenge) instead of dying as a generic ServerError(503)
/// after exhausting retries. `.expect(1)` proves the block returns
/// immediately — no retry loop, no wasted requests.
#[tokio::test]
async fn test_503_cloudflare_challenge_returns_waf_error_not_server_error() {
    ensure_waf_inspector();
    let mock_server = MockServer::start().await;

    let challenge_body = r#"<html><head><title>Just a moment...</title></head>
    <body><div id="challenge-running">Checking your browser...</div></body></html>"#;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(challenge_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = retry_client(3);

    let result = get_route(&client, &mock_server, "/").await;

    let err = result.expect_err("503 challenge must be a WAF block, not Ok");
    assert!(
        matches!(err, HttpError::WafChallenge(_)),
        "expected WafChallenge, got {err:?}"
    );
    mock_server.verify().await;
}

/// A genuine 5xx error with no WAF signature still surfaces as ServerError
/// (triangulation: the 5xx inspection only diverts actual challenges).
///
/// RES-01: the response carries a `cf-ray` header — present on EVERY
/// Cloudflare edge response, including genuine transient origin failures —
/// which must NOT turn a plain 503 into a WAF block.
#[tokio::test]
async fn test_503_plain_error_still_returns_server_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("content-type", "text/plain")
                .insert_header("cf-ray", "abc123-IAD")
                .set_body_string("service temporarily unavailable"),
        )
        .mount(&mock_server)
        .await;

    let client = retry_client(1);

    let result = get_route(&client, &mock_server, "/").await;

    assert!(
        matches!(result.unwrap_err(), HttpError::ServerError(503)),
        "plain 503 with a cf-ray trace header must stay a ServerError"
    );
}

/// RES-01 (real-world case): a genuine transient 503 behind Cloudflare — a
/// generic HTML error body plus the ubiquitous `cf-ray` edge header — must
/// surface as ServerError after retries, NOT as a WAF block. `cf-ray` rides
/// on 100% of Cloudflare traffic and carries zero challenge evidence, so it
/// must not correlate with the 503 to fabricate a false positive.
#[tokio::test]
async fn test_503_cf_ray_generic_error_still_returns_server_error() {
    let mock_server = MockServer::start().await;

    let error_body = "<html><body><h1>503 Service Unavailable</h1>\
        <p>The server is temporarily unable to service your request.</p></body></html>";

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("cf-ray", "abc123-IAD")
                .set_body_string(error_body),
        )
        .mount(&mock_server)
        .await;

    let client = retry_client(2);

    let result = get_route(&client, &mock_server, "/").await;

    let err = result.expect_err("generic 503 with cf-ray must not be Ok");
    assert!(
        matches!(err, HttpError::ServerError(503)),
        "expected ServerError(503), got {err:?}"
    );
}

/// RES-01 guard: purging the ubiquitous trace headers must NOT weaken the
/// correlated-mitigation case. A 503 carrying `cf-mitigated` (an active
/// Cloudflare mitigation signal, retained in the registry) is still a WAF
/// block — the Fingerprint header correlates with the 503 status. `.expect(1)`
/// proves the block returns immediately with no retry.
#[tokio::test]
async fn test_503_cf_mitigated_still_returns_waf_error() {
    ensure_waf_inspector();
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("content-type", "text/html; charset=utf-8")
                .insert_header("cf-mitigated", "challenge")
                .set_body_string("<html><body>temporarily unavailable</body></html>"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = retry_client(3);

    let result = get_route(&client, &mock_server, "/").await;

    let err = result.expect_err("cf-mitigated 503 must be a WAF block, not Ok");
    assert!(
        matches!(err, HttpError::WafChallenge(_)),
        "expected WafChallenge, got {err:?}"
    );
    mock_server.verify().await;
}

/// FIX B (body-vs-header granularity): a genuine transient 503 whose BODY
/// merely mentions a vendor (bare "cloudflare", T2 Body) is ubiquitous
/// diagnostic noise — NOT a WAF block. It must surface as `ServerError(503)`
/// after retries, not as an instant `WafChallenge`. A control HEADER such as
/// `cf-mitigated` would still block (see
/// `test_503_cf_mitigated_still_returns_waf_error`).
#[tokio::test]
async fn test_503_bare_body_vendor_mention_retries_then_server_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string("<html><body>served by cloudflare</body></html>"),
        )
        .mount(&mock_server)
        .await;

    let client = retry_client(2);

    let result = get_route(&client, &mock_server, "/").await;

    assert!(
        matches!(result.unwrap_err(), HttpError::ServerError(503)),
        "bare body vendor mention at 503 must stay a ServerError (retries), not a WAF block"
    );
}

/// REQ-WAF-02/04/05 at the client boundary: a 200 `application/json` body
/// carrying `akamai_hash` (the tls.peet.ws false positive from issue #346)
/// passes — the content-type gate skips scanning JSON entirely.
#[tokio::test]
async fn test_200_json_akamai_hash_passes() {
    let mock_server = MockServer::start().await;

    let json_body = r#"{"tls":{"peetprint_hash":"abc123","akamai_hash":"def456"}}"#;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(json_body),
        )
        .mount(&mock_server)
        .await;

    let client = default_client();

    let result = get_route(&client, &mock_server, "/").await;

    assert!(result.is_ok(), "JSON akamai_hash must pass, got {result:?}");
    assert_eq!(result.unwrap(), json_body);
}

/// A classified 200 challenge returns immediately: `.expect(1)` proves the
/// old fallback ladder (~4 requests / ~5s of UA rotation) is gone.
#[tokio::test]
async fn test_200_classified_block_skips_fallback_ladder() {
    ensure_waf_inspector();
    let mock_server = MockServer::start().await;

    let challenge_body =
        r#"<html><body><div class="g-recaptcha" data-sitekey="abc"></div></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(challenge_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = default_client();

    let result = get_route(&client, &mock_server, "/").await;

    assert!(matches!(result.unwrap_err(), HttpError::WafChallenge(_)));
    mock_server.verify().await;
}
