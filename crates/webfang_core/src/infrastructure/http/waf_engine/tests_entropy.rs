//! Tests de `waf_engine`: Entropy rules and the >100 KB size gate.
//!
//! Extraido del `mod tests` inline de `waf_engine.rs` (PR 1 de la mision Ola 1,
//! paraguas #2584) por seam del propio archivo: movimiento mecanico verbatim
//! (dedent -4), sin cambios semanticos ni renombres. `waf_engine.rs` esta
//! grandfathered en `scripts/quality-baselines.json` y el ratchet es descendente,
//! asi que el descenso del baseline viaja en el mismo PR que este corte.

use super::tests_common::ctx_with;
use super::*;
use crate::domain::waf::{InspectionContext, WafTier};

// ========================================================================
// TASK-06 — Entropy rules (REQ-WAF-06)
//
// The entropy mechanism landed in inspect() in TASK-05 (inseparable: the
// silent-challenge / obfuscated-WAF guard rails must stay green the moment
// the shims delegate to inspect). These tests provide the dedicated
// REQ-WAF-06 status-aware policy coverage — each distinguishes the new
// policy from the old "always block on entropy" behavior.
// ========================================================================

#[test]
fn test_entropy_silent_challenge_200_html_blocks() {
    // REQ-WAF-06b: 200 + HTML, <1500B, 6 scripts → BLOCK (silent challenge).
    // This is the "H3 fix" case discovery.rs depends on — MUST be kept.
    let body = "<html><script></script><script></script><script></script><script></script><script></script><script></script></html>";
    let ctx = ctx_with(Some(200), Some("text/html"));
    let verdict = WafInspector::inspect(body, &ctx);
    assert!(verdict.is_blocked, "200+HTML silent challenge must block");
    assert!(verdict
        .evidences
        .iter()
        .any(|e| e.provider == "Silent Challenge"));
}

#[test]
fn test_entropy_silent_challenge_200_non_html_passes() {
    // REQ-WAF-06b: 200 + non-HTML, <1500B, 6 scripts → NOT blocked.
    let body = "<html><script></script><script></script><script></script><script></script><script></script><script></script></html>";
    let ctx = ctx_with(Some(200), Some("text/plain"));
    let verdict = WafInspector::inspect(body, &ctx);
    assert!(
        !verdict.is_blocked,
        "200+non-HTML script density must not block"
    );
}

#[test]
fn test_entropy_silent_challenge_non_2xx_blocks() {
    // REQ-WAF-06b: non-2xx + <1500B + >5 scripts → BLOCK.
    let body = "<html><script></script><script></script><script></script><script></script><script></script><script></script></html>";
    let ctx = ctx_with(Some(503), Some("text/html"));
    let verdict = WafInspector::inspect(body, &ctx);
    assert!(verdict.is_blocked, "non-2xx silent challenge must block");
}

// ========================================================================
// #781 — Config-string false positives (Wikipedia / MediaWiki)
//
// A quoted JS/JSON config value — MediaWiki's edit-captcha config dump,
// e.g. `"wgConfirmEditCaptchaNeededForGenericEdit": "hcaptcha"` — is NOT a
// served challenge. It stays evidence (it feeds the chain under a WAF
// status), but it is not genuine challenge context: it must not escalate a
// dense legitimate page into an obfuscated-WAF block at HTTP 200.
// ========================================================================

/// Deterministic >100KB high-entropy body (byte cycle ≈ 8.0 b/B), with an
/// optional fragment appended. Same construction as the existing
/// entropy-policy tests so no randomness leaks into the suite.
fn high_entropy_body(fragment: &str) -> String {
    let mut body: String = (0u8..=255)
        .map(|b| b as char)
        .cycle()
        .take(104_000)
        .collect();
    body.push_str(fragment);
    body
}

/// MediaWiki-style captcha config excerpt (issue #781 shape, no spaces).
const MEDIAWIKI_HCAPTCHA_CONFIG: &str = r#"{"wgConfirmEditCaptchaNeededForGenericEdit":"hcaptcha","wgConfirmEditHCaptchaSiteKey":"5d0c670e-4dc5-4e69-a45e-9f5b7c2d11ab"}"#;

/// Same config with spaces around the colon (pretty-printed variant).
const MEDIAWIKI_HCAPTCHA_CONFIG_SPACED: &str = r#"{ "wgConfirmEditCaptchaNeededForGenericEdit": "hcaptcha", "wgConfirmEditHCaptchaSiteKey": "5d0c670e-4dc5-4e69-a45e-9f5b7c2d11ab" }"#;

#[test]
fn test_config_string_match_json_value_is_config() {
    // #781 core: a full-quoted-string hcaptcha value preceded by a quoted
    // key is a config occurrence — evidence yes, genuine context no.
    for config in [MEDIAWIKI_HCAPTCHA_CONFIG, MEDIAWIKI_HCAPTCHA_CONFIG_SPACED] {
        let (evidences, genuine) = collect_body_evidence(config);
        assert!(
            evidences.iter().any(|e| e.matched_pattern == "hcaptcha"),
            "config occurrences still produce evidence (got {evidences:?})"
        );
        assert!(
            !genuine,
            "quoted-key JSON values are not genuine challenge context ({config})"
        );
    }
}

#[test]
fn test_config_string_match_html_attribute_is_genuine() {
    // `class="hcaptcha"` embeds the marker in an HTML attribute value
    // (preceded by '='), never in JSON value position → genuine.
    let (_, genuine) = collect_body_evidence(r#"<div class="hcaptcha"></div>"#);
    assert!(genuine, "attribute-embedded marker stays genuine");
}

#[test]
fn test_config_string_match_script_src_is_genuine() {
    // `src="https://hcaptcha.com/..."` has a URL prefix before the marker
    // inside the quoted string → genuine challenge signal.
    let (_, genuine) =
        collect_body_evidence(r#"<script src="https://hcaptcha.com/1/api.js"></script>"#);
    assert!(genuine, "script-source marker stays genuine");
}

#[test]
fn test_config_string_match_prose_is_genuine() {
    let (_, genuine) = collect_body_evidence("the site uses hcaptcha for protection");
    assert!(genuine, "unquoted prose mentions stay genuine");
}

#[test]
fn test_config_string_match_embedded_in_string_is_genuine() {
    // The marker must fill the ENTIRE quoted string; embedded mentions are
    // genuine (conservative default of the #781 filter).
    let (_, genuine) = collect_body_evidence(r#"{"msg":"say hcaptcha today"}"#);
    assert!(genuine, "marker embedded inside a string stays genuine");
}

#[test]
fn test_mediawiki_hcaptcha_config_dense_200_not_blocked() {
    // Issue #781 fixture (a): MediaWiki config with `hcaptcha` config
    // strings + >100KB dense body + HTTP 200 → NOT blocked. The config
    // occurrence is still reported as (non-genuine) fingerprint evidence.
    let body = high_entropy_body(MEDIAWIKI_HCAPTCHA_CONFIG);
    let ctx = ctx_with(Some(200), Some("text/html"));
    let verdict = WafInspector::inspect(&body, &ctx);
    assert!(
        !verdict.is_blocked,
        "config-string fingerprint must not escalate a 200 page (got {verdict:?})"
    );
    assert!(
        verdict
            .evidences
            .iter()
            .any(|e| e.matched_pattern == "hcaptcha" && e.tier == WafTier::Fingerprint),
        "the config occurrence is still collected as evidence (got {:?})",
        verdict.evidences
    );
}

#[test]
fn test_real_hcaptcha_widget_dense_200_blocks() {
    // Issue #781 fixture (b): a REAL served hCaptcha widget on a large
    // body at HTTP 200 still blocks (T1 challenge marker, any status).
    let body = high_entropy_body(
        r#"<div class="h-captcha" data-sitekey="5d0c670e"></div>
          <script src="https://hcaptcha.com/1/api.js"></script>"#,
    );
    let ctx = ctx_with(Some(200), Some("text/html"));
    let verdict = WafInspector::inspect(&body, &ctx);
    assert!(
        verdict.is_blocked,
        "real hCaptcha widget markup must still block at 200"
    );
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("hCaptcha"),
        "the challenge marker drives the block (got {:?})",
        verdict.evidences
    );
}

#[test]
fn test_config_string_hcaptcha_403_blocks() {
    // Issue #781 fixture (c): the SAME config-only body at a WAF status
    // (403) still blocks — config evidence correlates with the challenge
    // status, so nothing about detection weakens.
    let ctx = ctx_with(Some(403), Some("text/html"));
    let verdict = WafInspector::inspect(MEDIAWIKI_HCAPTCHA_CONFIG, &ctx);
    assert!(
        verdict.is_blocked,
        "config fingerprint at 403 must still block"
    );
    assert!(
        verdict
            .evidences
            .iter()
            .any(|e| e.matched_pattern == "hcaptcha"),
        "the evidence chain carries the hcaptcha fingerprint (got {:?})",
        verdict.evidences
    );
}

#[test]
fn test_genuine_fingerprint_still_escalates_entropy_200() {
    // Triangulation for the #781 gate: the entropy rule keeps escalating at
    // 200 when the coexisting fingerprint is GENUINE (prose marker here),
    // distinguishing it from the config-string case above.
    let body = high_entropy_body(" protected by cloudflare edge ");
    let ctx = ctx_with(Some(200), Some("text/html"));
    let verdict = WafInspector::inspect(&body, &ctx);
    assert!(
        verdict.is_blocked,
        "genuine T2 coexistence with high entropy must still block at 200"
    );
}

// ========================================================================
// Entropy tests — ported from waf.rs
// ========================================================================

#[test]
fn test_calculate_entropy_high() {
    let obfuscated_js: String = (0u8..=255).map(|b| b as char).collect();
    let entropy = calculate_entropy(&obfuscated_js);
    assert!(entropy > 6.0, "entropy={entropy}, expected > 6.0");
}

#[test]
fn test_calculate_entropy_low() {
    let plain_text = "Hello world, this is a normal page with regular content.";
    let entropy = calculate_entropy(plain_text);
    assert!(entropy < 5.0);
}

#[test]
fn test_is_suspicious_size() {
    assert!(is_suspicious_size(150_000));
    assert!(!is_suspicious_size(10_000));
    assert!(!is_suspicious_size(100_000));
    assert!(is_suspicious_size(100_001));
}

#[test]
fn test_detect_body_by_entropy() {
    // Create >100KB with high entropy to trigger Shannon entropy detection.
    //
    // #1615 F8: the status used to be absent here, and the rule treated an
    // unknown status as "not 200", so a large high-entropy body in DEGRADED
    // mode blocked. Degraded mode has no HTTP context at all, and its
    // documented contract is that only unambiguous challenge markers block —
    // a size/entropy heuristic is not one. The detection is still real, so
    // the row now supplies the status that correlates with a WAF.
    let high_entropy_content: String = (0u8..=255)
        .map(|b| b as char)
        .chain((0u8..=255).map(|b| b as char))
        .chain((0u8..=255).map(|b| b as char))
        .chain((0u8..=255).map(|b| b as char))
        .cycle()
        .take(104_000)
        .collect();
    let ctx = InspectionContext {
        status: Some(403),
        ..Default::default()
    };
    let verdict = WafInspector::inspect(&high_entropy_content, &ctx);
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Obfuscated WAF")
    );
}

#[test]
fn test_detect_body_small_low_entropy() {
    let small_content = "<html><body>Redirecting...</body></html>";
    let verdict = WafInspector::inspect(small_content, &InspectionContext::default());
    assert!(!verdict.is_blocked);
}

// ---------------------------------------------------------------------------
// `is_suspicious_size`: helper de produccion de uso exclusivo en tests que vivia
// como `#[cfg(test)] fn` en el padre; su unico consumidor es
// `test_is_suspicious_size`, de la seccion de entropia de abajo.
// ---------------------------------------------------------------------------

/// Check if body size indicates a potential WAF challenge (>100KB)
#[cfg(test)]
#[inline]
fn is_suspicious_size(body_len: usize) -> bool {
    body_len > SUSPICIOUS_SIZE_THRESHOLD
}
