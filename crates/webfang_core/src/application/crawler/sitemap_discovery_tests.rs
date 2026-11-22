use super::*;
use crate::domain::error::WafDetectionKind;
use crate::infrastructure::observability::test_log_capture::LogBuffer;
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

fn test_client() -> wreq::Client {
    wreq::Client::new()
}

#[cfg_attr(
    miri,
    ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
)]
#[tokio::test]
async fn discovers_via_robots_absolute_directive() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("User-agent: *\nSitemap: https://example.com/sitemap.xml\n"),
        )
        .mount(&mock)
        .await;

    let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
    assert_eq!(result.unwrap(), "https://example.com/sitemap.xml");
}

#[cfg_attr(
    miri,
    ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
)]
#[tokio::test]
async fn discovers_via_robots_relative_directive() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nSitemap: /sitemap.xml\n"),
        )
        .mount(&mock)
        .await;

    let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
    assert_eq!(result.unwrap(), format!("{}/sitemap.xml", mock.uri()));
}

#[cfg_attr(
    miri,
    ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
)]
#[tokio::test]
async fn discovers_via_fallback_location() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
        .mount(&mock)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
    assert_eq!(result.unwrap(), format!("{}/sitemap.xml", mock.uri()));
}

#[cfg_attr(
    miri,
    ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
)]
#[tokio::test]
async fn discovers_via_subpath_sitemap() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/docs/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let base = format!("{}/docs", mock.uri());
    let result = discover_sitemap_url(&base, &test_client()).await;
    assert_eq!(result.unwrap(), format!("{}/docs/sitemap.xml", mock.uri()));
}

// ── Issue #879 (Option A): WAF inspection over sitemap-chain bodies ──

/// Cloudflare Turnstile widget marker — Challenge-tier (T1), blocks even
/// in degraded mode per REQ-WAF-05/09.
const CHALLENGE_HTML: &str =
    r#"<html><body>Just a moment...</body><div id="cf-turnstile" data-sitekey="abc"></div></html>"#;

/// A WAF challenge served where robots.txt should be must abort discovery
/// with the typed `CrawlError::WafChallenge` (carrying host context and
/// evidence chain) and emit a structured trace event — not silently fall
/// through to `SitemapNotFound` (issue #879).
#[cfg_attr(
    miri,
    ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
)]
#[test]
fn robots_txt_waf_challenge_yields_typed_error_and_trace_event() {
    ensure_waf_inspector();
    let buf = LogBuffer::run(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let mock = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/robots.txt"))
                .respond_with(ResponseTemplate::new(200).set_body_string(CHALLENGE_HTML))
                .mount(&mock)
                .await;

            let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;

            match result {
                Err(CrawlError::WafChallenge {
                    provider,
                    kind,
                    url,
                }) => {
                    assert_eq!(kind, WafDetectionKind::BodySignature);
                    assert!(
                        provider.contains("Cloudflare"),
                        "evidence chain expected in provider, got: {provider}"
                    );
                    assert_eq!(url, format!("{}/robots.txt", mock.uri()));
                },
                other => panic!("expected CrawlError::WafChallenge, got: {other:?}"),
            }
        });
    });

    let out = buf.text();
    assert!(
        out.contains("challenge"),
        "WAF trace event expected on the discovery path, got: {out}"
    );
}

/// A benign robots.txt that merely mentions a WAF vendor at status 200 is
/// Fingerprint-tier evidence, which never blocks without a correlated WAF
/// status (REQ-WAF-09): discovery must proceed to fallbacks as before.
#[cfg_attr(
    miri,
    ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
)]
#[tokio::test]
async fn benign_robots_txt_mentioning_vendor_does_not_trigger_waf_error() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("User-agent: *\n# site hosted behind cloudflare\n"),
        )
        .mount(&mock)
        .await;

    let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
    assert!(
        !matches!(result, Err(CrawlError::WafChallenge { .. })),
        "benign vendor mention must not raise WafChallenge, got: {result:?}"
    );
}

#[cfg_attr(
    miri,
    ignore = "test_client() builds a real wreq::Client (btls-sys TLS_method FFI at build time); unsupported by Miri (exhaustive survey for #1563)"
)]
#[tokio::test]
async fn errors_when_no_sitemap_found() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    Mock::given(method("HEAD"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;

    let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
    assert!(result.is_err());
}
