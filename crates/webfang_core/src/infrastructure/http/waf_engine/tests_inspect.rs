//! Tests for `waf_engine`: verdict policy, inspect(ctx), error UX and the evidence-chain accessor.
//!
//! Extracted from the inline `mod tests` of `waf_engine.rs` (PR 1 of the Wave 1
//! mission, umbrella #2584) along the file's own seam: verbatim mechanical move
//! (dedent -4), no semantic changes and no renames. `waf_engine.rs` is
//! grandfathered in `scripts/quality-baselines.json` and the ratchet is
//! downward-only, so the baseline decrease travels in the same PR as this split.

use super::tests_common::ctx_with;
use super::*;
use crate::domain::waf::{fixtures, EvidenceSource, InspectionContext, WafTier, WafVerdict};
use std::collections::HashMap;

// ========================================================================
// TASK-05 — Verdict policy + inspect(ctx) (REQ-WAF-05)
// ========================================================================

// `ctx_with` does not live here: its destination is dictated by its consumers
// (three different children), so it lives in `tests_common` with `pub(super)`.
// See that file's header.

#[test]
fn test_inspect_t1_blocks_at_200() {
    // Fixture 3: Turnstile (T1) at 200 → BLOCK.
    let ctx = ctx_with(Some(200), Some("text/html"));
    let verdict = WafInspector::inspect(r#"<div id="cf-turnstile"></div>"#, &ctx);
    assert!(verdict.is_blocked, "T1 must block at 200");
    assert!(verdict
        .evidences
        .iter()
        .any(|e| e.tier == WafTier::Challenge));
}

#[test]
fn test_inspect_cf_503_challenge_blocks() {
    // Fixture 2: Cloudflare 503 challenge (T1 prose) → BLOCK.
    let ctx = ctx_with(Some(503), Some("text/html"));
    let verdict = WafInspector::inspect("<title>Just a moment...</title>", &ctx);
    assert!(verdict.is_blocked, "T1 prose at 503 must block");
}

#[test]
fn test_inspect_t2_only_at_200_passes() {
    // Fixtures 1/4/5: T2-only evidence at 200 → PASS (but evidence collected).
    let ctx = ctx_with(Some(200), Some("text/html"));
    let verdict = WafInspector::inspect("an article mentioning cloudflare in prose", &ctx);
    assert!(!verdict.is_blocked, "T2 at 200 must not block");
    assert!(!verdict.evidences.is_empty(), "T2 evidence still collected");
}

#[test]
fn test_inspect_t2_body_at_503_does_not_block() {
    // FIX B: on 5xx a bare vendor mention in the BODY is ubiquitous diagnostic
    // noise (the body analog of the purged cf-ray header) — NOT blocking, so a
    // genuine transient 503 retries instead of dying as an instant WafChallenge.
    let ctx = ctx_with(Some(503), Some("text/html"));
    let verdict = WafInspector::inspect("served by cloudflare", &ctx);
    assert!(
        !verdict.is_blocked,
        "bare body vendor mention (T2 Body) at 503 must NOT block (got {verdict:?})"
    );
    assert!(
        !verdict.evidences.is_empty(),
        "T2 evidence is still collected (informational)"
    );
}

#[test]
fn test_inspect_t2_body_at_403_blocks() {
    // FIX B: 403 is NOT 5xx, so a bare body vendor mention (T2 Body) still
    // blocks — the body-vs-header carve-out applies only to 5xx.
    let ctx = ctx_with(Some(403), Some("text/html"));
    let verdict = WafInspector::inspect("served by cloudflare", &ctx);
    assert!(
        verdict.is_blocked,
        "bare body vendor mention (T2 Body) at 403 must block (got {verdict:?})"
    );
}

#[test]
fn test_inspect_t2_blocking_status_set() {
    // Only 403/429/503/520-529 correlate with T2 evidence. Exercised via a
    // control HEADER (cf-mitigated): header-sourced T2 blocks on every
    // correlated status, including the 5xx ones (RES-01), so this validates
    // is_t2_blocking_status directly. Body-sourced T2 at 5xx is carved out
    // by FIX B — see test_inspect_t2_body_at_503_does_not_block.
    for status in [403u16, 429, 503, 520, 525, 529] {
        let mut headers = HashMap::new();
        headers.insert("cf-mitigated".to_string(), "challenge".to_string());
        let ctx = InspectionContext {
            status: Some(status),
            content_type: Some("text/html".into()),
            headers,
            ..Default::default()
        };
        let verdict = WafInspector::inspect("ordinary body", &ctx);
        assert!(verdict.is_blocked, "T2 header at {status} must block");
    }
    for status in [200u16, 201, 301, 404, 500, 519, 530] {
        let mut headers = HashMap::new();
        headers.insert("cf-mitigated".to_string(), "challenge".to_string());
        let ctx = InspectionContext {
            status: Some(status),
            content_type: Some("text/html".into()),
            headers,
            ..Default::default()
        };
        let verdict = WafInspector::inspect("ordinary body", &ctx);
        assert!(!verdict.is_blocked, "T2 header at {status} must NOT block");
    }
}

#[test]
fn test_collect_body_evidence_source_is_body() {
    // FIX B: body-signature evidence is tagged EvidenceSource::Body so the
    // verdict can carve out 5xx body noise from header-sourced mitigation.
    let (evidences, genuine) = collect_body_evidence("an article mentioning cloudflare in prose");
    assert!(
        !evidences.is_empty(),
        "expected body evidence (got {evidences:?})"
    );
    assert!(
        evidences.iter().all(|e| e.source == EvidenceSource::Body),
        "body evidence must be tagged Body (got {evidences:?})"
    );
    assert!(genuine, "prose vendor mention is genuine challenge context");
}

#[test]
fn test_collect_header_evidence_source_is_header() {
    // FIX B: control-header evidence is tagged EvidenceSource::Header so it
    // still blocks on 5xx (active mitigation) unlike body-sourced T2.
    let mut headers = HashMap::new();
    headers.insert("cf-mitigated".to_string(), "challenge".to_string());
    let evidences = collect_header_evidence(&headers);
    assert_eq!(evidences.len(), 1, "expected one header evidence");
    assert!(
        evidences.iter().all(|e| e.source == EvidenceSource::Header),
        "header evidence must be tagged Header (got {evidences:?})"
    );
}

#[test]
fn test_inspect_degraded_t2_never_blocks() {
    // Degraded mode (no status): T2 reported low-confidence, never blocks.
    let ctx = InspectionContext::default();
    let verdict = WafInspector::inspect("served by cloudflare", &ctx);
    assert!(!verdict.is_blocked, "degraded T2 must not block");
    assert!(!verdict.evidences.is_empty(), "T2 evidence still emitted");
}

#[test]
fn test_inspect_degraded_t1_blocks() {
    // Degraded mode: T1 still blocks.
    let ctx = InspectionContext::default();
    let verdict = WafInspector::inspect("<div class='g-recaptcha'></div>", &ctx);
    assert!(verdict.is_blocked, "degraded T1 must block");
}

#[test]
fn test_inspect_ignore_waf_clean() {
    // REQ-WAF-02 step 1: ignore_waf → clean verdict even with T1 present.
    let ctx = InspectionContext {
        ignore_waf: true,
        content_type: Some("text/html".into()),
        ..Default::default()
    };
    let verdict = WafInspector::inspect("<div id='cf-turnstile'></div>", &ctx);
    assert!(!verdict.is_blocked, "ignore_waf must yield clean verdict");
    assert!(
        verdict.evidences.is_empty(),
        "ignore_waf collects no evidence"
    );
}

#[test]
fn test_inspect_control_header_t2_never_auto_blocks() {
    // Correction B: a control header alone (T2) never blocks on mere presence.
    let mut headers = HashMap::new();
    headers.insert("x-datadome-response".to_string(), "blocked".to_string());
    let ctx = InspectionContext {
        headers,
        ..Default::default()
    };
    let verdict = WafInspector::inspect("normal content here", &ctx);
    assert!(
        !verdict.is_blocked,
        "header alone must not block (degraded)"
    );
    assert!(verdict
        .evidences
        .iter()
        .any(|e| e.matched_pattern == "x-datadome-response"));
}

#[test]
fn test_inspect_control_header_with_503_blocks() {
    // Control header (T2) + correlated status → block.
    let mut headers = HashMap::new();
    headers.insert("cf-mitigated".to_string(), "challenge".to_string());
    let ctx = InspectionContext {
        headers,
        status: Some(503),
        content_type: Some("text/html".into()),
        ..Default::default()
    };
    let verdict = WafInspector::inspect("normal content here", &ctx);
    assert!(verdict.is_blocked, "cf-mitigated + 503 must block");
}

#[test]
fn test_inspect_json_gate_not_scanned() {
    // Fixture 1: 200 application/json with akamai_hash → not scanned → clean.
    let ctx = ctx_with(Some(200), Some("application/json"));
    let verdict = WafInspector::inspect(r#"{"key": "akamai_hash"}"#, &ctx);
    assert!(!verdict.is_blocked, "json body must not be scanned");
    assert!(verdict.evidences.is_empty(), "no evidence from gated body");
}

#[test]
fn test_inspect_collects_all_evidences() {
    // REQ-WAF-01: the verdict carries ALL collected evidences, not first-hit.
    // T2-only body at 403 (a T1 marker would short-circuit the pass; a 5xx
    // status would carve out body-sourced T2 per FIX B): every boundary-clean
    // fingerprint is collected and correlates with the non-5xx WAF status.
    let ctx = ctx_with(Some(403), Some("text/html"));
    let verdict =
        WafInspector::inspect("protected by cloudflare and datadome and perimeterx", &ctx);
    assert!(verdict.is_blocked);
    assert!(
        verdict.evidences.len() >= 3,
        "multiple T2 evidences collected (got {:?})",
        verdict.evidences
    );
}

// ========================================================================
// TASK-07 — Error UX (evidence chain) + observability (REQ-WAF-08)
// ========================================================================

#[test]
fn test_evidence_chain_spanish_lists_all_evidences() {
    let chain = fixtures::two_evidence_verdict(false, EvidenceSource::Body).evidence_chain();
    // Every evidence's provider + pattern + tier label is listed.
    assert!(chain.contains("Cloudflare"), "chain: {chain}");
    assert!(chain.contains("cf-turnstile"), "chain: {chain}");
    assert!(chain.contains("challenge"), "Challenge label: {chain}");
    assert!(chain.contains("Akamai"), "chain: {chain}");
    assert!(chain.contains("akamai"), "chain: {chain}");
    assert!(chain.contains("fingerprint"), "Fingerprint label: {chain}");
}

#[test]
fn test_evidence_chain_empty_fallback() {
    assert_eq!(WafVerdict::clean().evidence_chain(), "Unknown WAF");
}

// ========================================================================
// TASK-10 — Public verdict evidence-chain accessor (REQ-WAF-08)
// ========================================================================

#[test]
fn test_verdict_evidence_chain_lists_all_evidences() {
    // REQ-WAF-08: callers (client/scraper_service/discovery/MCP) format the
    // Evidence chain straight from the verdict they received.
    let verdict = fixtures::two_evidence_verdict(true, EvidenceSource::Body);
    let chain = verdict.evidence_chain();
    assert!(chain.contains("Cloudflare"), "chain: {chain}");
    assert!(chain.contains("cf-turnstile"), "chain: {chain}");
    assert!(chain.contains("challenge"), "Challenge label: {chain}");
    assert!(chain.contains("Akamai"), "chain: {chain}");
    assert!(chain.contains("fingerprint"), "Fingerprint label: {chain}");
}

#[test]
fn test_verdict_evidence_chain_clean_is_fallback() {
    // A clean verdict (no evidence) renders the generic fallback label.
    let verdict = WafVerdict::clean();
    assert_eq!(verdict.evidence_chain(), "Unknown WAF");
}
