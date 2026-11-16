//! Main WAF Inspector entry point.
//!
//! This module provides the [`WafInspector`] struct and its [`inspect`] method,
//! which is the single entry point for context-aware, evidence-based WAF
//! detection (REQ-WAF-01).

use crate::domain::waf::{sealed::Sealed, InspectionContext, WafInspectorPort, WafVerdict};

use super::{
    content_type::should_scan_body,
    entropy::entropy_evidence,
    evidence::{collect_body_evidence, collect_header_evidence},
    verdict::decide,
};

/// WafInspector provides multi-layer WAF detection
pub struct WafInspector;

impl Sealed for WafInspector {}

impl WafInspectorPort for WafInspector {
    fn inspect(&self, body: &str, ctx: &InspectionContext) -> WafVerdict {
        WafInspector::inspect(body, ctx)
    }
}

impl WafInspector {
    /// Inspect a response body with full HTTP context and return a verdict.
    ///
    /// This is the single entry point for context-aware, evidence-based WAF
    /// detection (REQ-WAF-01). It collects ALL evidence — boundary-filtered body
    /// signatures (REQ-WAF-03/04), control headers (REQ-WAF-03), and entropy
    /// signals (REQ-WAF-06) — then applies the verdict policy (REQ-WAF-05):
    ///
    /// - [`crate::domain::waf::WafTier::Challenge`] (T1) blocks at ANY status, including 200.
    /// - [`crate::domain::waf::WafTier::Fingerprint`] (T2) blocks only with a correlated WAF status
    ///   (403 / 429 / 503 / 520–529); in degraded mode (no status) T2 is reported
    ///   low-confidence and never blocks.
    ///
    /// `ctx.ignore_waf` yields a clean verdict immediately (REQ-WAF-02 step 1),
    /// and the content-type denylist (REQ-WAF-02) gates body scanning.
    ///
    /// The HTTP-200 entropy escalation (REQ-WAF-06 rule (a)) requires a GENUINE
    /// challenge-context Fingerprint: a config-string occurrence (#781, e.g.
    /// `"wgConfirmEdit...":"hcaptcha"` in a MediaWiki config dump) is recorded
    /// as evidence but never escalates a dense legitimate page into a block.
    ///
    /// Thread-safe: the AC automaton is immutable once compiled via `LazyLock`.
    #[must_use]
    pub fn inspect(body: &str, ctx: &InspectionContext) -> WafVerdict {
        // REQ-WAF-02 step 1: explicit bypass → clean verdict.
        if ctx.ignore_waf {
            return WafVerdict::clean();
        }

        let mut evidences = Vec::new();
        let scan_body = should_scan_body(ctx);

        // Body signatures (boundary-filtered), gated by content-type. The
        // collector also reports whether at least one GENUINE challenge-context
        // Fingerprint survived (config-string occurrences, #781, count as
        // evidence but not as genuine challenge context).
        let mut has_genuine_fingerprint = false;
        if scan_body {
            let (body_evidences, genuine) = collect_body_evidence(body);
            evidences.extend(body_evidences);
            has_genuine_fingerprint = genuine;
        }

        // Control headers are always inspected (independent of the body gate).
        // Every control header is Fingerprint-tier evidence of ACTIVE mitigation,
        // which is genuine challenge context for the entropy rule.
        let header_evidences = collect_header_evidence(&ctx.headers);
        has_genuine_fingerprint |= !header_evidences.is_empty();
        evidences.extend(header_evidences);

        // Entropy signals need full T2 awareness (rule (a) coexistence), so they
        // run after body + header evidence is collected.
        if scan_body {
            evidences.extend(entropy_evidence(body, ctx, has_genuine_fingerprint));
        }

        let is_blocked = decide(&evidences, ctx);

        // Observability (REQ-WAF-08): the engine inspects silently — both the
        // blocking and the informational path log at debug. The decision site
        // (HTTP client / scraper service / crawler discovery) owns the WARN or
        // structured error, so a block is not double-logged here.
        if is_blocked {
            tracing::debug!(
                status = ?ctx.status,
                evidences = evidences.len(),
                "WAF/CAPTCHA challenge detected; blocking response"
            );
        } else if !evidences.is_empty() {
            tracing::debug!(
                status = ?ctx.status,
                evidences = evidences.len(),
                "WAF fingerprints observed but not blocking (informational)"
            );
        }

        WafVerdict {
            is_blocked,
            evidences,
        }
    }

    /// Get the list of supported WAF providers
    #[must_use]
    pub fn supported_providers() -> Vec<&'static str> {
        super::signatures::supported_providers()
    }
}
