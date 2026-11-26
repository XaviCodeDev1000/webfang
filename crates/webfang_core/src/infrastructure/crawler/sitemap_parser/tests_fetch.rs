use super::*;
use webfang_test_utils::EnvGuard;

/// Bug #9 regression: a 404 response must yield SitemapError::HttpError
/// ("server returned 404"), NOT SitemapError::InvalidContentType. Status
/// MUST be checked BEFORE content-type (issue #590).
#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
#[tokio::test]
async fn test_sitemap_404_yields_http_error_not_content_type() {
    let _entry_off = EnvGuard::entry_guard_off();
    use wiremock::{MockServer, ResponseTemplate};

    let mock = MockServer::start().await;
    let result = test_support::serve_and_parse(
        &mock,
        "/sitemap.xml",
        ResponseTemplate::new(404).set_body_string("Not Found"),
        "/sitemap.xml",
    )
    .await;

    match result {
        Err(SitemapError::HttpError { message, .. }) => {
            assert!(
                message.contains("404"),
                "error must reference 404, got: {message}"
            );
        },
        other => panic!("expected SitemapError::HttpError with 404, got: {other:?}"),
    }
}

// Gap B: parse_with_depth — depth=0 returns MaxDepthExceeded without HTTP
#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
#[tokio::test]
async fn test_parse_from_url_depth_zero_returns_error() {
    let config = SitemapConfig::builder().max_depth(0).build();
    let parser = SitemapParser::with_config(config).unwrap();
    let result = parser
        .parse_from_url("https://example.com/sitemap.xml", &CorrelationId::new())
        .await;
    assert!(matches!(result, Err(SitemapError::MaxDepthExceeded)));
}

#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
#[tokio::test]
#[ignore = "requires network — hits real DNS for invalid-host-xyz-12345.com"]
async fn test_parse_from_url_depth_one_attempts_fetch() {
    let config = SitemapConfig::builder().max_depth(1).build();
    let parser = SitemapParser::with_config(config).unwrap();
    // depth=1 means it tries the HTTP fetch — with an invalid host it should fail
    let result = parser
        .parse_from_url(
            "https://invalid-host-xyz-12345.com/sitemap.xml",
            &CorrelationId::new(),
        )
        .await;
    assert!(result.is_err());
}
