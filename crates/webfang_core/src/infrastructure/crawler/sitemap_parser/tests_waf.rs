use super::*;
use crate::infrastructure::observability::test_log_capture::LogBuffer;
use wiremock::{MockServer, ResponseTemplate};
// One-layer entry disarmer for loopback mocks (#1382/#1369), canonical in
// `webfang_test_utils::EnvGuard::entry_guard_off` since #1396: wiremock binds
// 127.0.0.1, which the parser's entry guard now rejects pre-socket in
// production posture. These tests exercise WAF inspection, not the guard
// (pinned in `sitemap_discovery` tests + `sitemap_ssrf_e2e_test`).
use webfang_test_utils::EnvGuard;

// ── Issue #879 (Option A): WAF inspection over sitemap bodies ──

/// Cloudflare Turnstile widget marker — Challenge-tier (T1), blocks even
/// in degraded mode per REQ-WAF-05/09.
const CHALLENGE_HTML: &str =
    r#"<html><body>Just a moment...</body><div id="cf-turnstile" data-sitekey="abc"></div></html>"#;

/// A WAF challenge page served where sitemap XML should be must surface the
/// typed `SitemapError::WafChallenge` with host context and evidence chain,
/// plus a structured trace event — not a generic XML parse failure (#879).
#[test]
#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
fn parse_from_url_waf_challenge_body_returns_typed_error_and_trace_event() {
    let _entry_off = EnvGuard::entry_guard_off();
    let buf = LogBuffer::run(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let mock = MockServer::start().await;
            let result = test_support::serve_and_parse(
                &mock,
                "/sitemap.xml",
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(CHALLENGE_HTML),
                "/sitemap.xml",
            )
            .await;

            match result {
                Err(SitemapError::WafChallenge { url, provider }) => {
                    assert_eq!(url, format!("{}/sitemap.xml", mock.uri()));
                    assert!(
                        provider.contains("Cloudflare"),
                        "evidence chain expected in provider, got: {provider}"
                    );
                },
                other => panic!("expected SitemapError::WafChallenge, got: {other:?}"),
            }
        });
    });

    let out = buf.text();
    assert!(
        out.contains("challenge"),
        "WAF trace event expected on the parser path, got: {out}"
    );
}

/// A challenge served by ONE child of a sitemap index must hard-abort the
/// whole index with `WafChallenge` (HybridRouter parity) instead of being
/// swallowed into `AllChildrenFailed` (#879).
#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
#[tokio::test]
async fn index_child_waf_challenge_propagates_typed_error() {
    let _entry_off = EnvGuard::entry_guard_off();
    let mock = MockServer::start().await;
    let result = test_support::serve_many_and_parse(
        &mock,
        vec![
            (
                "/sitemap.xml",
                ResponseTemplate::new(200).set_body_string(format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\
             <sitemap><loc>{}/child.xml</loc></sitemap>\
             </sitemapindex>",
                    mock.uri()
                )),
            ),
            (
                "/child.xml",
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(CHALLENGE_HTML),
            ),
        ],
        "/sitemap.xml",
    )
    .await;

    match result {
        Err(SitemapError::WafChallenge { url, .. }) => {
            assert_eq!(url, format!("{}/child.xml", mock.uri()));
        },
        other => panic!("expected SitemapError::WafChallenge from index child, got: {other:?}"),
    }
}

/// A benign sitemap body that merely mentions a WAF vendor at status 200 is
/// Fingerprint-tier evidence and must NOT raise `WafChallenge` (REQ-WAF-09).
#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
#[tokio::test]
async fn benign_sitemap_body_mentioning_vendor_is_not_a_challenge() {
    let _entry_off = EnvGuard::entry_guard_off();
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <!-- served behind cloudflare -->
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        <url><loc>https://example.com/page1</loc></url>
    </urlset>"#;
    let mock = MockServer::start().await;
    let result = test_support::serve_and_parse(
        &mock,
        "/sitemap.xml",
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/xml")
            .set_body_string(xml),
        "/sitemap.xml",
    )
    .await;
    assert!(
        !matches!(result, Err(SitemapError::WafChallenge { .. })),
        "benign vendor mention must not raise WafChallenge, got: {result:?}"
    );
    assert!(
        result.is_ok(),
        "valid XML must still parse, got: {result:?}"
    );
}
