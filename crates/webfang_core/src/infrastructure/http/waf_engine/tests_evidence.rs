//! Tests for `waf_engine`: boundary filter, evidence cap and content-type gate.
//!
//! Extracted from the inline `mod tests` of `waf_engine.rs` (PR 1 of the Ola 1
//! mission, umbrella #2584) along the file's own seam: verbatim mechanical move
//! (dedent -4), no semantic changes and no renames. `waf_engine.rs` is
//! grandfathered in `scripts/quality-baselines.json` and the ratchet is
//! downward-only, so the baseline decrease travels in the same PR as this split.

use super::tests_common::ctx_with;
use super::*;
use crate::domain::waf::{InspectionContext, WafTier};

// ========================================================================
// TASK-03 — Boundary post-filter (REQ-WAF-04)
// ========================================================================

#[test]
fn test_boundary_rejects_trailing_underscore() {
    // Fixture 1 core: bare "akamai" [B] rejected when followed by '_'.
    let (evidences, _) = collect_body_evidence(r#"{"key": "akamai_hash"}"#);
    assert!(
        !evidences.iter().any(|e| e.matched_pattern == "akamai"),
        "bare 'akamai' must be rejected when followed by '_' (got {evidences:?})"
    );
}

#[test]
fn test_boundary_rejects_alphanumeric_prefix() {
    // bare "akamai" [B] rejected when preceded by an alphanumeric byte.
    let (evidences, _) = collect_body_evidence("xakamai");
    assert!(
        !evidences.iter().any(|e| e.matched_pattern == "akamai"),
        "bare 'akamai' must be rejected when preceded by 'x' (got {evidences:?})"
    );
}

#[test]
fn test_boundary_stands_in_prose() {
    // Fixture 4 core: "cloudflare" surrounded by spaces stands (T2 evidence).
    let (evidences, _) = collect_body_evidence("an article mentioning cloudflare in prose");
    assert!(
        evidences
            .iter()
            .any(|e| e.matched_pattern == "cloudflare" && e.tier == WafTier::Fingerprint),
        "'cloudflare' with space boundaries must stand (got {evidences:?})"
    );
}

#[test]
fn test_boundary_exempt_pattern_matches_inside_token() {
    // Fixture 5 core: "incap_ses" [E] matches inside the larger token.
    let (evidences, _) = collect_body_evidence("cookie incap_ses_123 = abc");
    assert!(
        evidences.iter().any(|e| e.matched_pattern == "incap_ses"),
        "[E] 'incap_ses' must match inside 'incap_ses_123' (got {evidences:?})"
    );
}

#[test]
fn test_boundary_t1_exempt() {
    // Challenge-tier patterns are exempt from the boundary filter.
    let (evidences, _) = collect_body_evidence("xxcf-turnstileyy");
    assert!(
        evidences
            .iter()
            .any(|e| e.matched_pattern == "cf-turnstile"),
        "T1 'cf-turnstile' matches regardless of adjacent bytes (got {evidences:?})"
    );
}

#[test]
fn test_boundary_utf8_non_ascii_is_boundary() {
    // Any non-ASCII byte counts as a boundary (UTF-8 safe).
    let (evidences, _) = collect_body_evidence("ñakamaiñ");
    assert!(
        evidences.iter().any(|e| e.matched_pattern == "akamai"),
        "non-ASCII adjacent bytes are boundaries, 'akamai' stands (got {evidences:?})"
    );
}

// ========================================================================
// RISK-01 — Evidence allocation cap (DoS hardening)
// ========================================================================

#[test]
fn test_body_evidence_capped_at_max_per_body() {
    // RISK-01: adversarial text/html cannot materialize unbounded evidence.
    // Twice-the-cap boundary-clean bare "cloudflare" matches collapse to the
    // cap (the old code returned every match — an uncapped Vec).
    let body = " cloudflare ".repeat(MAX_EVIDENCE_PER_BODY * 2);
    let (evidences, _) = collect_body_evidence(&body);
    assert_eq!(
        evidences.len(),
        MAX_EVIDENCE_PER_BODY,
        "evidence must be capped at MAX_EVIDENCE_PER_BODY"
    );
}

#[test]
fn test_body_evidence_below_cap_collects_all() {
    // Triangulation: the cap is an upper bound, not a target — a body with
    // fewer matches than the cap still collects every one of them.
    let body = " cloudflare  datadome  sucuri ";
    let (evidences, _) = collect_body_evidence(body);
    assert_eq!(
        evidences.len(),
        3,
        "all sub-cap matches collected (got {evidences:?})"
    );
}

#[test]
fn test_t1_marker_visible_past_evidence_cap() {
    // Regression (cap-vs-tier false negative): MAX_EVIDENCE_PER_BODY bounds
    // T2 accumulation, NOT detection. A body carrying a full cap of T2
    // matches FOLLOWED BY a T1 marker must still block at status 200 — the
    // T1 is a definitive verdict that noise density cannot hide. The old
    // code applied `.take(MAX)` to the raw Aho-Corasick iterator, so a T1
    // marker past 64 earlier matches was never examined → false negative.
    let body = format!(
        "{} cf-turnstile ",
        " cloudflare ".repeat(MAX_EVIDENCE_PER_BODY)
    );
    let ctx = ctx_with(Some(200), Some("text/html"));
    let verdict = WafInspector::inspect(&body, &ctx);
    assert!(
        verdict.is_blocked,
        "T1 marker past the T2 cap must still block at 200 (got {verdict:?})"
    );
    assert!(
        verdict
            .evidences
            .iter()
            .any(|e| e.tier == WafTier::Challenge),
        "the surviving evidence must be the T1 marker (got {:?})",
        verdict.evidences
    );
}

// ========================================================================
// TASK-04 — Content-Type gate / denylist (REQ-WAF-02)
// ========================================================================

#[test]
fn test_gate_skips_application_json() {
    // Fixture 1 gate: 200 application/json is NOT scanned.
    let ctx = InspectionContext {
        content_type: Some("application/json".into()),
        ..Default::default()
    };
    assert!(!should_scan_body(&ctx), "application/json must be skipped");
}

#[test]
fn test_gate_skips_structured_suffixes() {
    for ct in [
        "application/ld+json",
        "application/vnd.api+json",
        "application/atom+xml",
    ] {
        let ctx = InspectionContext {
            content_type: Some(ct.into()),
            ..Default::default()
        };
        assert!(!should_scan_body(&ctx), "{ct} must be skipped");
    }
}

#[test]
fn test_gate_scans_xhtml_carve_out() {
    // application/xhtml+xml is carved out → scanned despite the +xml suffix.
    let ctx = InspectionContext {
        content_type: Some("application/xhtml+xml".into()),
        ..Default::default()
    };
    assert!(
        should_scan_body(&ctx),
        "xhtml+xml carve-out must be scanned"
    );
}

#[test]
fn test_gate_skips_binary_assets() {
    for ct in [
        "image/png",
        "image/jpeg",
        "font/woff2",
        "application/wasm",
        "application/javascript",
    ] {
        let ctx = InspectionContext {
            content_type: Some(ct.into()),
            ..Default::default()
        };
        assert!(!should_scan_body(&ctx), "{ct} must be skipped");
    }
}

#[test]
fn test_gate_scans_text_and_missing() {
    // text/* and missing content-type → scanned (denylist, not allowlist).
    for ct in [Some("text/html"), Some("text/plain"), None] {
        let ctx = InspectionContext {
            content_type: ct.map(String::from),
            ..Default::default()
        };
        assert!(should_scan_body(&ctx), "{ct:?} must be scanned");
    }
}

#[test]
fn test_gate_ignores_charset_parameter() {
    // Parameters after ';' are ignored when classifying the MIME type.
    let ctx = InspectionContext {
        content_type: Some("application/json; charset=utf-8".into()),
        ..Default::default()
    };
    assert!(!should_scan_body(&ctx), "json with charset must be skipped");
    let ctx = InspectionContext {
        content_type: Some("text/html; charset=utf-8".into()),
        ..Default::default()
    };
    assert!(should_scan_body(&ctx), "html with charset must be scanned");
}
