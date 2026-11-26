#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! S11 (T11) — L3 output classification and escalation-context regressions.
//!
//! The hybrid router used to return the L3 `Ok(page)` unclassified and fed
//! `detect_spa` a degraded `InspectionContext` (no status, headers or cookies).
//! These tests exercise the public ports only: the router's `Downloader`
//! implementation and `WafInspector::inspect` with a caller-built context.
//! Fixtures are in-memory and deterministic — no network, clock or randomness.

use std::collections::HashMap;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;
use url::Url;
use webfang_core::domain::waf::InspectionContext;
use webfang_core::infrastructure::downloader::hybrid_router::HybridRouter;
use webfang_core::infrastructure::downloader::spa_detector::{detect_spa, SpaSignal};
use webfang_core::infrastructure::downloader::{DownloadError, Downloader, FetchedPage};
use webfang_core::infrastructure::http::waf_engine::WafInspector;

/// In-memory downloader layer. Each layer returns its own canned page so the
/// three escalation stages can be composed without spawning a browser or
/// touching the network.
struct PageDownloader {
    html: String,
    status: u16,
    headers: HashMap<String, String>,
}

impl PageDownloader {
    fn spa() -> Self {
        Self {
            html: r#"<!DOCTYPE html><html><body><div id="root"></div></body></html>"#.into(),
            status: 200,
            headers: HashMap::new(),
        }
    }

    /// An L3 page that is a genuine Cloudflare challenge (T1 marker), served
    /// with a real HTML content-type and a WAF-correlated status.
    fn l3_challenge() -> Self {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/html".to_string());
        Self {
            html: r#"<!DOCTYPE html><html><body><div id="challenge-running">Checking your browser...</div></body></html>"#
                .into(),
            status: 503,
            headers,
        }
    }
}

impl Downloader for PageDownloader {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        let page = FetchedPage {
            url: url.clone(),
            html: self.html.clone(),
            status: self.status,
            headers: self.headers.clone(),
            cookies: Vec::new(),
        };
        Box::pin(async move { Ok(page) })
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        1
    }
}

#[tokio::test]
async fn l3_challenge_aborts() {
    // L1 and L2 are SPA shells, so escalation reaches L3. The L3 page carries
    // a T1 challenge that must abort instead of being returned as a success.
    let router = HybridRouter::new(
        PageDownloader::spa(),
        PageDownloader::spa(),
        PageDownloader::l3_challenge(),
        false,
        CancellationToken::new(),
    );
    let url: Url = "https://l3-waf.example.com".parse().unwrap();
    let err = router.fetch(&url).await.unwrap_err();
    assert!(
        matches!(err, DownloadError::WafChallenge(_)),
        "L3 challenge must abort the escalation, got: {err}"
    );
}

#[test]
fn large_legitimate_200_with_control_header_does_not_block() {
    // A >100KB high-entropy body is ordinary for a legitimate page. A control
    // header is T2 evidence; at status 200 it must not be promoted to a T1
    // "Obfuscated WAF" verdict on its own.
    let body: String = (0u8..=255).map(char::from).cycle().take(104_000).collect();
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "text/html".to_string());
    headers.insert("cf-mitigated".to_string(), "challenge".to_string());
    let ctx = InspectionContext {
        status: Some(200),
        content_type: Some("text/html".to_string()),
        headers,
        ignore_waf: false,
    };
    let verdict = WafInspector::inspect(&body, &ctx);
    assert!(
        !verdict.is_blocked,
        "large legitimate 200 with a control header must not block, got: {verdict:?}"
    );
}

#[test]
fn five_xx_with_vendor_marker_blocks() {
    // `_abck` is a vendor-specific Akamai marker (T2, Exempt). On a WAF-
    // correlated 5xx it is a real mitigation signal, not bare vendor prose,
    // so it must not be carved out by the 5xx body-noise rule.
    let body = r#"<html><body><input type="hidden" name="_abck" value="abc"></body></html>"#;
    let ctx = InspectionContext {
        status: Some(503),
        content_type: Some("text/html".to_string()),
        headers: HashMap::new(),
        ignore_waf: false,
    };
    let verdict = WafInspector::inspect(body, &ctx);
    assert!(
        verdict.is_blocked,
        "503 with a vendor marker in the body must block, got: {verdict:?}"
    );
}

#[test]
fn bare_vendor_five_xx_stays_unblocked() {
    // PRESERVE: the FIX B carve-out for a bare vendor word in prose at 5xx
    // still holds — only specific vendor markers are retained.
    let ctx = InspectionContext {
        status: Some(503),
        content_type: Some("text/html".to_string()),
        headers: HashMap::new(),
        ignore_waf: false,
    };
    let verdict = WafInspector::inspect("served by cloudflare", &ctx);
    assert!(
        !verdict.is_blocked,
        "bare body vendor mention at 503 must stay unblocked, got: {verdict:?}"
    );
}

#[test]
fn detect_spa_uses_real_http_context() {
    // S11 context population: the same body is accepted under a degraded
    // context but aborts when the caller supplies the real 503 + control
    // header context (T2 header evidence correlates with the WAF status).
    let body = r#"<!DOCTYPE html><html><body><article><p>This server rendered article carries enough visible text to be classified as static content by the detector.</p></article></body></html>"#;
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "text/html".to_string());
    headers.insert("cf-mitigated".to_string(), "challenge".to_string());
    let real = InspectionContext {
        status: Some(503),
        content_type: Some("text/html".to_string()),
        headers,
        ignore_waf: false,
    };
    assert_eq!(detect_spa(body, &real), SpaSignal::WafBlocked);
    assert_eq!(
        detect_spa(body, &InspectionContext::default()),
        SpaSignal::StaticContent
    );
}
