//! Tests for `waf_engine`: headers + body degraded inspection.
//!
//! Extracted from the inline `mod tests` of `waf_engine.rs` (PR 1 of the Ola 1
//! mission, umbrella #2584) along the file's own seam: verbatim mechanical move
//! (dedent -4), no semantic changes and no renames. `waf_engine.rs` is
//! grandfathered in `scripts/quality-baselines.json` and the ratchet is
//! downward-only, so the baseline decrease travels in the same PR as this split.

use super::*;
use crate::domain::waf::InspectionContext;
use std::collections::HashMap;

// ========================================================================
// Headers + body degraded inspection tests (via inspect)
// ========================================================================

#[test]
fn test_waf_control_header_detection() {
    // Control headers are Fingerprint-tier evidence — mere presence never
    // auto-blocks (correction B). Degraded mode (no status), so a T2 header
    // alone is clean.
    let mut headers = HashMap::new();
    headers.insert("x-datadome-response".to_string(), "blocked".to_string());
    let ctx = InspectionContext {
        headers,
        ..Default::default()
    };
    let verdict = WafInspector::inspect("normal content", &ctx);
    assert!(
        !verdict.is_blocked,
        "T2 header alone must not block (degraded)"
    );

    // cf-ray alone also doesn't trigger (common in normal requests).
    let mut headers = HashMap::new();
    headers.insert("cf-ray".to_string(), "abc123".to_string());
    let ctx = InspectionContext {
        headers,
        ..Default::default()
    };
    let verdict = WafInspector::inspect("normal content", &ctx);
    assert!(!verdict.is_blocked);
}

#[test]
fn test_waf_body_signature_detection() {
    // Test Cloudflare detection
    let verdict = WafInspector::inspect("Just a moment...", &InspectionContext::default());
    assert!(verdict.is_blocked);

    // Test reCAPTCHA detection
    let verdict = WafInspector::inspect("<div class='g-recaptcha'>", &InspectionContext::default());
    assert!(verdict.is_blocked);

    // Test normal content passes
    let verdict = WafInspector::inspect(
        "<html><body><p>Hello World</p></body></html>",
        &InspectionContext::default(),
    );
    assert!(!verdict.is_blocked);
}

#[test]
fn test_silent_challenge_detection() {
    // #1615 F8: the 200+HTML arm of rule (b) — the "H3 fix" silent challenge
    // that `discovery.rs` depends on. The status is stated explicitly
    // rather than left absent: under the old "unknown counts as non-2xx"
    // rule this row passed without one, which meant it was really testing
    // the degraded arm.
    let body = r#"<html><script></script><script></script><script></script><script></script><script></script><script></script></html>"#;
    let ctx = InspectionContext {
        status: Some(200),
        content_type: Some("text/html".to_string()),
        ..Default::default()
    };
    let verdict = WafInspector::inspect(body, &ctx);
    assert!(verdict.is_blocked);

    let body = "<html><body><p>Hello</p></body></html>";
    let verdict = WafInspector::inspect(body, &ctx);
    assert!(!verdict.is_blocked);
}

/// #1615 F8 — the script-density arm of rule (b) had the same "any non-2xx"
/// problem as rule (a): a tiny body with several `<script>` tags behind a
/// 404 was reported as a silent challenge. Ordinary statuses no longer
/// block; the WAF-correlated ones and the 200+HTML case still do.
#[test]
fn script_density_does_not_block_on_an_ordinary_non_200_status() {
    let body = r#"<html><script></script><script></script><script></script><script></script><script></script><script></script></html>"#;
    for status in [204u16, 301, 404, 500, 502] {
        let ctx = InspectionContext {
            status: Some(status),
            content_type: Some("text/html".to_string()),
            ..Default::default()
        };
        assert!(
            !WafInspector::inspect(body, &ctx).is_blocked,
            "status {status} is not WAF evidence; a script-dense body must not \
             be reported as a silent challenge"
        );
    }
    for status in [403u16, 429, 503, 520] {
        let ctx = InspectionContext {
            status: Some(status),
            content_type: Some("text/html".to_string()),
            ..Default::default()
        };
        assert!(
            WafInspector::inspect(body, &ctx).is_blocked,
            "status {status} correlates with a WAF and must still block"
        );
    }
}

/// #1615 F8 / G-25 — same non-disclosure row for rule (b)'s label.
#[test]
fn the_evidence_chain_does_not_disclose_the_script_density_thresholds() {
    let body = r#"<html><script></script><script></script><script></script><script></script><script></script><script></script></html>"#;
    let ctx = InspectionContext {
        status: Some(200),
        content_type: Some("text/html".to_string()),
        ..Default::default()
    };
    let verdict = WafInspector::inspect(body, &ctx);
    assert!(verdict.is_blocked);
    let chain = verdict.evidence_chain();
    assert!(
        !chain.contains("1500") && !chain.contains("<script"),
        "the script-density label must not publish its byte or tag bounds: \
         {chain}"
    );
}

#[test]
fn test_aho_corasick_performance() {
    let body = "This is a page with Just a moment... and recaptcha/api.js content";
    let verdict = WafInspector::inspect(body, &InspectionContext::default());
    assert!(verdict.is_blocked);
}

#[test]
fn test_supported_providers() {
    let providers = WafInspector::supported_providers();
    assert!(!providers.is_empty());
    assert!(providers.contains(&"Cloudflare"));
    assert!(providers.contains(&"reCAPTCHA"));
    assert!(providers.contains(&"DataDome"));
    assert!(providers.contains(&"AWS WAF"));
}
