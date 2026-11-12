//! The >100 KB entropy rows of the WAF inspector, gated as a group under Miri.
//!
//! # Why these rows live in a child file
//!
//! `waf_engine.rs` is grandfathered in `scripts/quality-baselines.json` at 2570
//! lines and the size ratchet is DESCENDING: it may not grow by a single line.
//! The gates this lane needs are one line each, so they could not be written in
//! place. This is the same shape `cli/batch_flush.rs` used for
//! `batch_flush_tests.rs`, and for the same stated reason: the gates the Miri
//! lane needs would have broken the ratchet.
//!
//! Extracting the seven rows SHRINKS the parent by ~135 lines — towards the
//! 500-line rule rather than away from it — and leaves the other ninety tests
//! of the module running under the interpreter.
//!
//! # Why these seven are gated and the other ninety are not
//!
//! Each row below builds the same >100 KB deterministic high-entropy body (the
//! byte cycle ≈ 8.0 b/B) and runs the whole inspector over it. Measured in the
//! Miri run 43185176742, the `Miri (infra-slow: transport)` group took 68 min
//! against a 120-min cap, and the `http` family — which is entirely
//! `waf_engine` — accounted for ~54 of those minutes while `export` accounted
//! for ~11. These rows are that cost.
//!
//! The cost is not justified by what they can find: `waf_engine.rs` contains no
//! `unsafe`, and the interpreter's value on safe code is limited to UB in the
//! dependencies it reaches, which the other ninety rows already exercise with
//! small bodies. Gating a slow row on interpreter-only cost is the established
//! convention in this repository (see the proptest and 525-URL-loop gates).
//!
//! # What is lost, stated plainly
//!
//! The entropy policy of `REQ-WAF-06a` and the threshold-disclosure guard
//! (`the_evidence_chain_does_not_disclose_the_entropy_thresholds`) no longer run
//! under Miri. They still run in the ordinary `cargo test` lane, which is where
//! their value actually is: they pin a false-positive policy and a leaked
//! constant, not an aliasing invariant.

use super::*;

#[test]
#[cfg_attr(miri, ignore)] // >100KB entropy body; the transport group's dominant cost under Miri
fn test_entropy_obfuscated_200_no_t2_passes() {
    // REQ-WAF-06a: 200, >100KB, >5.5 b/B, no T2 marker → NOT blocked
    // (old behavior blocked unconditionally — this is the policy change).
    let high_entropy: String = (0u8..=255)
        .map(|b| b as char)
        .cycle()
        .take(104_000)
        .collect();
    let ctx = InspectionContext {
        status: Some(200),
        content_type: Some("text/html".to_string()),
        ..Default::default()
    };
    let verdict = WafInspector::inspect(&high_entropy, &ctx);
    assert!(
        !verdict.is_blocked,
        "200 high-entropy without T2 must not block"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // >100KB entropy body; the transport group's dominant cost under Miri
fn test_entropy_obfuscated_non_2xx_blocks() {
    // REQ-WAF-06a: non-2xx + >100KB + >5.5 b/B → BLOCK.
    let high_entropy: String = (0u8..=255)
        .map(|b| b as char)
        .cycle()
        .take(104_000)
        .collect();
    let ctx = InspectionContext {
        status: Some(503),
        content_type: Some("text/html".to_string()),
        ..Default::default()
    };
    let verdict = WafInspector::inspect(&high_entropy, &ctx);
    assert!(verdict.is_blocked, "non-2xx high-entropy must block");
    assert!(verdict
        .evidences
        .iter()
        .any(|e| e.provider == "Obfuscated WAF"));
}

#[test]
#[cfg_attr(miri, ignore)] // >100KB entropy body; the transport group's dominant cost under Miri
fn test_entropy_obfuscated_200_with_t2_blocks() {
    // REQ-WAF-06a: 200 + >100KB + >5.5 b/B + coexisting T2 marker → BLOCK.
    // The ascending byte cycle contains no signature, so append a
    // boundary-clean bare vendor name to force T2 coexistence.
    let mut high_entropy: String = (0u8..=255)
        .map(|b| b as char)
        .cycle()
        .take(104_000)
        .collect();
    high_entropy.push_str(" cloudflare ");
    let ctx = InspectionContext {
        status: Some(200),
        content_type: Some("text/html".to_string()),
        ..Default::default()
    };
    let verdict = WafInspector::inspect(&high_entropy, &ctx);
    assert!(
        verdict.is_blocked,
        "200 high-entropy with T2 coexistence must block"
    );
}

/// 500 is a broken origin, and 204 is a successful no-content response.
///
/// Each of these previously blocked. None of them is WAF evidence.
#[test]
#[cfg_attr(miri, ignore)] // >100KB entropy body; the transport group's dominant cost under Miri
fn high_entropy_does_not_block_on_an_ordinary_non_200_status() {
    let body: String = (0u8..=255)
        .map(|b| b as char)
        .cycle()
        .take(104_000)
        .collect();
    for status in [204u16, 301, 404, 410, 418, 500, 502] {
        let ctx = InspectionContext {
            status: Some(status),
            content_type: Some("text/html".to_string()),
            ..Default::default()
        };
        let verdict = WafInspector::inspect(&body, &ctx);
        assert!(
            !verdict.is_blocked,
            "status {status} is not WAF evidence; a {}-byte body must not be \
             reported as an obfuscated-WAF challenge (evidences: {:?})",
            body.len(),
            verdict
                .evidences
                .iter()
                .map(|e| (e.provider, e.matched_pattern))
                .collect::<Vec<_>>()
        );
    }
}

/// The same body, degraded. `InspectionContext::default()` carries no
/// status, and the documented contract for degraded mode is that only
/// unambiguous challenge markers block — so a heuristic must not.
#[test]
#[cfg_attr(miri, ignore)] // >100KB entropy body; the transport group's dominant cost under Miri
fn high_entropy_does_not_block_in_degraded_mode() {
    let body: String = (0u8..=255)
        .map(|b| b as char)
        .cycle()
        .take(104_000)
        .collect();
    let verdict = WafInspector::inspect(&body, &InspectionContext::default());
    assert!(
        !verdict.is_blocked,
        "degraded mode has no status to correlate, so entropy alone must not \
         block (evidences: {:?})",
        verdict
            .evidences
            .iter()
            .map(|e| (e.provider, e.matched_pattern))
            .collect::<Vec<_>>()
    );
}

/// And the WAF-correlated statuses still block, so the row above is a
/// false-positive fix and not a disabled rule.
#[test]
#[cfg_attr(miri, ignore)] // >100KB entropy body; the transport group's dominant cost under Miri
fn high_entropy_still_blocks_on_a_waf_correlated_status() {
    let body: String = (0u8..=255)
        .map(|b| b as char)
        .cycle()
        .take(104_000)
        .collect();
    for status in [403u16, 429, 503, 520, 529] {
        let ctx = InspectionContext {
            status: Some(status),
            ..Default::default()
        };
        assert!(
            WafInspector::inspect(&body, &ctx).is_blocked,
            "status {status} correlates with a WAF and must still block"
        );
    }
}

/// #1615 F8 / G-25 — the rule's thresholds must not be published.
///
/// `matched_pattern` reaches the user through `evidence_chain`, so a label
/// reading `>100KB` and `>5.5 b/B` was a specification of the rule: a
/// hostile page could size and pad itself to sit just under both. The rule
/// still runs on those numbers; this row pins that it stops ADVERTISING
/// them, and that no number from the constants leaks into the chain.
#[test]
#[cfg_attr(miri, ignore)] // >100KB entropy body; the transport group's dominant cost under Miri
fn the_evidence_chain_does_not_disclose_the_entropy_thresholds() {
    let body: String = (0u8..=255)
        .map(|b| b as char)
        .cycle()
        .take(104_000)
        .collect();
    let ctx = InspectionContext {
        status: Some(403),
        ..Default::default()
    };
    let verdict = WafInspector::inspect(&body, &ctx);
    assert!(verdict.is_blocked, "the rule must still fire");
    let chain = verdict.evidence_chain();
    for leak in ["100KB", "100_000", "5.5", "b/B", "1500", ">5"] {
        assert!(
            !chain.contains(leak),
            "the evidence chain discloses the rule's threshold {leak:?}: {chain}"
        );
    }
}
