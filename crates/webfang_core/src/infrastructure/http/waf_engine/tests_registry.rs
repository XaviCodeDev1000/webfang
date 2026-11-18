//! Tests for `waf_engine`: unified registry + reclassification table.
//!
//! Extracted from the inline `mod tests` of `waf_engine.rs` (PR 1 of the Ola 1
//! mission, umbrella #2584) along the file's own seam: verbatim mechanical move
//! (dedent -4), no semantic changes and no renames. `waf_engine.rs` is
//! grandfathered in `scripts/quality-baselines.json` and the ratchet is
//! downward-only, so the baseline decrease travels in the same PR as this split.

use super::*;
use crate::domain::waf::WafTier;
use std::collections::HashSet;

// ========================================================================
// TASK-02 — Unified registry + reclassification table (REQ-WAF-03, REQ-WAF-10)
// ========================================================================

#[test]
fn test_registry_has_no_del_residue() {
    // REQ-WAF-03: the 14 DEL entries must be purged from the body registry.
    const DEL: &[&str] = &[
        "cf-ray",
        "cf-cache-status",
        "cf-dns",
        "dd=",
        "data-domain",
        "_nfv",
        "bot detection",
        "automated requests",
        "security check",
        "anti-bot",
        "attack detected",
        "suspicious activity",
        "verify.js",
        "bot-check",
    ];
    for sig in WAF_BODY_SIGNATURES {
        assert!(
            !DEL.contains(&sig.0),
            "DEL pattern '{}' must not be in the registry",
            sig.0
        );
    }
}

#[test]
fn test_registry_patterns_are_unique() {
    // #1189: dedup by (pattern, tier); no exact pattern duplicates remain.
    let mut seen: HashSet<&str> = HashSet::new();
    for sig in WAF_BODY_SIGNATURES {
        assert!(seen.insert(sig.0), "duplicate pattern '{}'", sig.0);
    }
}

#[test]
fn test_registry_every_entry_has_provider() {
    for sig in WAF_BODY_SIGNATURES {
        assert!(!sig.0.is_empty(), "empty pattern in registry");
        assert!(!sig.1.is_empty(), "pattern '{}' has empty provider", sig.0);
    }
}

#[test]
fn test_registry_case_variant_challenge_prose_both_present() {
    // #1189: Aho-Corasick is case-sensitive, so "Checking your browser" and
    // "checking your browser" are distinct patterns and both must remain.
    let patterns: Vec<&str> = WAF_BODY_SIGNATURES.iter().map(|(p, _, _, _)| *p).collect();
    assert!(
        patterns.contains(&"Checking your browser"),
        "title-case prose missing"
    );
    assert!(
        patterns.contains(&"checking your browser"),
        "lowercase prose missing"
    );
}

#[test]
fn test_registry_folds_spa_markers() {
    // REQ-WAF-10: spa_detector WAF_MARKERS fold into the unified registry.
    let patterns: Vec<&str> = WAF_BODY_SIGNATURES.iter().map(|(p, _, _, _)| *p).collect();
    assert!(
        patterns.contains(&"challenge-running"),
        "challenge-running not folded"
    );
    assert!(
        patterns.contains(&"data-sitekey"),
        "data-sitekey not folded"
    );
    assert!(patterns.contains(&"hcaptcha"), "bare hcaptcha not folded");
}

#[test]
fn test_registry_tiers_are_valid() {
    // Every entry carries an explicit tier (Challenge or Fingerprint).
    for sig in WAF_BODY_SIGNATURES {
        assert!(
            sig.2 == WafTier::Challenge || sig.2 == WafTier::Fingerprint,
            "pattern '{}' has invalid tier",
            sig.0
        );
    }
}

#[test]
fn test_control_headers_purged_of_non_waf() {
    // REQ-WAF-03: x-wordpress (not a WAF header) and x-cdn (generic CDN) deleted.
    // RES-01: cf-ray and x-sucuri-id purged — ubiquitous trace headers present
    // on 100% of a vendor's traffic carry zero challenge evidence, so treating
    // them as Fingerprint evidence block-correlated every genuine transient 503
    // behind Cloudflare/Sucuri. Only status-correlated control headers remain.
    let names: Vec<&str> = WAF_CONTROL_HEADERS.iter().map(|(n, _)| *n).collect();
    assert!(
        !names.contains(&"x-wordpress"),
        "x-wordpress must be deleted"
    );
    assert!(!names.contains(&"x-cdn"), "x-cdn must be deleted");
    assert!(
        !names.contains(&"cf-ray"),
        "cf-ray trace header must be purged (RES-01)"
    );
    assert!(
        !names.contains(&"x-sucuri-id"),
        "x-sucuri-id trace header must be purged (RES-01)"
    );
    assert!(names.contains(&"x-datadome-response"));
    assert!(names.contains(&"cf-mitigated"));
    assert!(names.contains(&"x-akamai-edge-auth"));
    assert_eq!(
        names.len(),
        3,
        "exactly 3 correlated control headers expected"
    );
}
