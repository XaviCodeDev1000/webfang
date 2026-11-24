//! Tests for `waf_engine`: domain types + InspectionContext API.
//!
//! Extracted from the inline `mod tests` of `waf_engine.rs` (PR 1 of the Wave 1
//! mission, umbrella #2584) along the file's own seam: verbatim mechanical move
//! (dedent -4), no semantic changes and no renames. `waf_engine.rs` is
//! grandfathered in `scripts/quality-baselines.json` and the ratchet is
//! downward-only, so the baseline decrease travels in the same PR as this split.

use super::*;
use crate::domain::waf::{fixtures, EvidenceSource, InspectionContext, WafTier, WafVerdict};
use crate::error::ScraperError;

// ========================================================================
// TASK-01 — Domain types + InspectionContext API (REQ-WAF-01)
// ========================================================================

#[test]
fn test_inspection_context_default_is_degraded() {
    // REQ-WAF-01: Default is degraded mode (all None, ignore_waf=false).
    let ctx = InspectionContext::default();
    assert!(ctx.status.is_none(), "degraded mode has no status");
    assert!(
        ctx.content_type.is_none(),
        "degraded mode has no content-type"
    );
    assert!(ctx.headers.is_empty(), "degraded mode has no headers");
    assert!(!ctx.ignore_waf, "degraded mode does not bypass WAF");
}

#[test]
fn test_waf_verdict_clean_is_not_blocked() {
    let verdict = WafVerdict::clean();
    assert!(!verdict.is_blocked, "clean verdict must not block");
    assert!(
        verdict.evidences.is_empty(),
        "clean verdict carries no evidence"
    );
}

#[test]
fn test_waf_verdict_carries_all_evidences() {
    // REQ-WAF-01: the verdict carries ALL collected evidences, not first-hit.
    let verdict = fixtures::two_evidence_verdict(true, EvidenceSource::Body);
    assert_eq!(
        verdict.evidences.len(),
        2,
        "verdict must retain every evidence"
    );
    assert!(verdict
        .evidences
        .iter()
        .any(|e| e.tier == WafTier::Challenge));
    assert!(verdict
        .evidences
        .iter()
        .any(|e| e.tier == WafTier::Fingerprint));
}

// ========================================================================
// TASK-11 — InspectionContext from lowercased-key header maps (REQ-WAF-01)
// ========================================================================

#[test]
fn test_from_lowercase_headers_builds_full_context() {
    // scraper_service / discovery capture headers as lowercased-key
    // HashMaps; the constructor lifts status + content-type and converts
    // the map into a wreq HeaderMap for control-header evidence.
    let mut headers = std::collections::HashMap::new();
    headers.insert(
        "content-type".to_string(),
        "text/html; charset=utf-8".to_string(),
    );
    headers.insert("cf-mitigated".to_string(), "challenge".to_string());

    let ctx = InspectionContext::from_lowercase_headers(200, &headers, false);

    assert_eq!(ctx.status, Some(200));
    assert_eq!(
        ctx.content_type.as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert!(!ctx.ignore_waf);
    // Both headers survive the conversion (control header is T2 evidence).
    assert!(ctx.headers.contains_key("cf-mitigated"));
    assert!(ctx.headers.contains_key("content-type"));
}

#[test]
fn test_from_lowercase_headers_missing_content_type_is_none() {
    // No content-type entry → None (the REQ-WAF-02 gate then scans the body).
    let headers = std::collections::HashMap::new();
    let ctx = InspectionContext::from_lowercase_headers(404, &headers, true);
    assert_eq!(ctx.status, Some(404));
    assert!(ctx.content_type.is_none());
    assert!(ctx.ignore_waf, "ignore_waf flag must propagate");
    assert!(ctx.headers.is_empty());
}

#[test]
fn test_from_lowercase_headers_drives_control_header_evidence() {
    // End-to-end: a control header supplied via the lowercased map is
    // collected as Fingerprint evidence (never auto-blocking at 200).
    let mut headers = std::collections::HashMap::new();
    headers.insert("x-datadome-response".to_string(), "1".to_string());
    let ctx = InspectionContext::from_lowercase_headers(200, &headers, false);

    let verdict = WafInspector::inspect("<html>clean body</html>", &ctx);
    assert!(!verdict.is_blocked, "T2 header at 200 must not block");
    assert!(
        verdict
            .evidences
            .iter()
            .any(|e| e.matched_pattern == "x-datadome-response"),
        "control header must be collected as evidence"
    );
}

#[test]
fn test_verify_integrity_error_carries_evidence_chain() {
    // REQ-WAF-08: a blocked verdict exposes the evidence chain.
    let verdict = WafInspector::inspect("Just a moment...", &InspectionContext::default());
    assert!(verdict.is_blocked, "T1 prose must block");
    let chain = verdict.evidence_chain();
    assert!(chain.contains("Cloudflare"), "chain provider: {chain}");
    assert!(chain.contains("pattern:"), "chain pattern label: {chain}");
    assert!(chain.contains("tier:"), "chain tier label: {chain}");
}

#[test]
fn test_block_error_stays_permanent_fatal() {
    // REQ-WAF-08: ErrorClass stays PermanentFatal (exit 69).
    let err = ScraperError::waf_blocked(
        "https://example.com",
        fixtures::single_cloudflare_blocked_verdict().evidence_chain(),
    );
    assert_eq!(err.classify(), crate::error::ErrorClass::PermanentFatal);
}
