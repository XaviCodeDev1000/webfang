//! Tests de `waf_engine`: Body-only degraded inspection (detect_body).
//!
//! Extraido del `mod tests` inline de `waf_engine.rs` (PR 1 de la mision Ola 1,
//! paraguas #2584) por seam del propio archivo: movimiento mecanico verbatim
//! (dedent -4), sin cambios semanticos ni renombres. `waf_engine.rs` esta
//! grandfathered en `scripts/quality-baselines.json` y el ratchet es descendente,
//! asi que el descenso del baseline viaja en el mismo PR que este corte.

use super::*;
use crate::domain::waf::{InspectionContext, WafTier};

// ========================================================================
// Body-only degraded inspection tests (via inspect, ported from waf.rs)
// ========================================================================

#[test]
fn test_detect_body_cloudflare_turnstile() {
    let html = r#"<div id="cf-turnstile" data-sitekey="abc123"></div>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Cloudflare Turnstile")
    );
}

#[test]
fn test_detect_body_cloudflare_just_a_moment() {
    let html = "<html><body><h1>Just a moment...</h1></body></html>";
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Cloudflare")
    );
}

#[test]
fn test_detect_body_cloudflare_checking_browser_degraded_does_not_block() {
    // "Checking your browser" is Fingerprint-tier: in degraded mode (no status)
    // it must NOT block (REQ-WAF-09). It is collected as evidence but the
    // verdict is not blocked without a correlated WAF status.
    let html = "<html><body>Checking your browser before accessing...</body></html>";
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(!verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Cloudflare")
    );
    assert_eq!(
        verdict.evidences.first().map(|e| e.tier),
        Some(WafTier::Fingerprint)
    );
}

#[test]
fn test_detect_body_cloudflare_checking_browser_blocks_with_waf_status() {
    // Fingerprint-tier blocks when correlated with a WAF status (403/429/503/520-529).
    let html = "<html><body>Checking your browser before accessing...</body></html>";
    let ctx = InspectionContext {
        status: Some(403),
        ..Default::default()
    };
    let verdict = WafInspector::inspect(html, &ctx);
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Cloudflare")
    );
}

#[test]
fn test_detect_body_recaptcha() {
    let html = r#"<script src="https://www.google.com/recaptcha/api.js?render=abc"></script>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("reCAPTCHA")
    );
}

#[test]
fn test_detect_body_g_recaptcha() {
    let html = r#"<div class="g-recaptcha" data-sitekey="abc"></div>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("reCAPTCHA")
    );
}

#[test]
fn test_detect_body_hcaptcha() {
    let html = r#"<div class="h-captcha" data-sitekey="abc"></div>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("hCaptcha")
    );
}

#[test]
fn test_detect_body_datadome() {
    // DataDome's own T1 marker (dd-captcha) is the decisive evidence. The old
    // fixture mixed `captcha.js` (a Generic Challenge T1) with DataDome T2
    // names; the T1 short-circuit reports the decisive challenge marker, so
    // the body uses a DataDome-specific T1 to assert DataDome attribution.
    let html = r#"<div id="dd-captcha">challenge</div>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("DataDome")
    );
}

#[test]
fn test_detect_body_perimeterx() {
    let html = r#"<script>var _pxCaptcha = {};</script>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("PerimeterX")
    );
}

#[test]
fn test_detect_body_akamai() {
    // `_abck` is a Fingerprint-tier ([E]) marker. In degraded mode T2
    // evidence never blocks, so this is not reported as a block — the
    // false positive that motivated issue #346.
    let html = r#"<input type="hidden" name="_abck" value="xxx">"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(
        !verdict.is_blocked,
        "T2 evidence must not block in degraded mode"
    );
}

#[test]
fn test_detect_body_generic_challenge() {
    let html = "<p>Please verify you are a human to continue.</p>";
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Generic Challenge")
    );
}

#[test]
fn test_detect_body_clean_html() {
    let html = r#"
        <html>
            <head><title>Normal Page</title></head>
            <body>
                <article>
                    <h1>Welcome</h1>
                    <p>This is a normal page with real content.</p>
                </article>
            </body>
        </html>
    "#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(!verdict.is_blocked);
}

#[test]
fn test_detect_body_empty() {
    let verdict = WafInspector::inspect("", &InspectionContext::default());
    assert!(!verdict.is_blocked);
}

#[test]
fn test_detect_body_aws_waf_cookie_domain_list() {
    let html = r#"<script>window.awsWafCookieDomainList = [];</script>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("AWS WAF")
    );
}

#[test]
fn test_detect_body_aws_waf_integration() {
    let html = r#"<script>AwsWafIntegration.saveReferrer();</script>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("AWS WAF")
    );
}

#[test]
fn test_detect_body_aws_waf_goku_props() {
    let html = r#"<script>window.gokuProps = {"key":"AQIDAH..."};</script>"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("AWS WAF")
    );
}

#[test]
fn test_detect_body_aws_waf_token() {
    let html = r#"<meta name="aws-waf-token" content="abc123">"#;
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("AWS WAF")
    );
}
