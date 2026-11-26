//! Verdict policy for WAF detection (REQ-WAF-05).
//! - [`WafTier::Challenge`] blocks at ANY status (including 200 and degraded).
//! - [`WafTier::Fingerprint`] blocks only when the status correlates with a WAF
//!   response (403 / 429 / 503 / 520–529); degraded mode (no status) never
//!   blocks on Fingerprint evidence (reported low-confidence).
//!
//! FIX B (body-vs-header granularity): on a 5xx response a Fingerprint match
//! sourced from the BODY ([`EvidenceSource::Body`]) is ubiquitous diagnostic
//! noise — the body analog of the purged `cf-ray` header — and does NOT block,
//! so a genuine transient 5xx retries. Fingerprint evidence sourced from a
//! control HEADER ([`EvidenceSource::Header`], e.g. `cf-mitigated`) signals
//! active mitigation and still blocks on 5xx (RES-01). The distinction is by
//! evidence source, not status — [`crate::domain::waf::is_t2_blocking_status`] is unchanged.
//!
//! Entropy challenges are emitted as [`WafTier::Challenge`] evidence only when
//! their own policy (REQ-WAF-06) already decided to block, so they short-circuit
//! here like any other Challenge marker.

use crate::domain::waf::{is_t2_blocking_status, EvidenceSource, WafEvidence, WafTier};

use super::signatures::{BoundaryMode, WAF_BODY_SIGNATURES};

/// Whether an HTTP status is a 5xx server error (FIX B body-vs-header carve-out).
#[inline]
fn is_5xx(status: Option<u16>) -> bool {
    matches!(status, Some(500..=599))
}

/// Whether a body match is a vendor marker rather than a bare vendor word.
///
/// Specific tokens, domains and cookie names (`_abck`, `incap_ses`,
/// `hcaptcha.com`, …) are real mitigation signals; the `Bare` vendor words in
/// ordinary prose are the diagnostic noise the 5xx carve-out targets (S11).
fn is_vendor_marker(pattern: &str) -> bool {
    WAF_BODY_SIGNATURES.iter().any(|(sig, _, tier, boundary)| {
        sig == &pattern && *tier == WafTier::Fingerprint && *boundary != BoundaryMode::Bare
    })
}

/// Verdict policy (REQ-WAF-05).
pub fn decide(evidences: &[WafEvidence], ctx: &crate::domain::waf::InspectionContext) -> bool {
    for ev in evidences {
        match ev.tier {
            WafTier::Challenge => return true,
            WafTier::Fingerprint => {
                if !is_t2_blocking_status(ctx.status) {
                    continue;
                }
                // RES-01 extension: on 5xx a bare vendor mention in the BODY is
                // ubiquitous diagnostic noise (the body analog of the purged
                // cf-ray header) — not blocking. A vendor-specific marker in
                // the body is a real signal and is retained (S11). Header-sourced
                // T2 (cf-mitigated etc.) signals active mitigation and still blocks.
                if is_5xx(ctx.status)
                    && ev.source == EvidenceSource::Body
                    && !is_vendor_marker(ev.matched_pattern)
                {
                    continue;
                }
                return true;
            },
        }
    }
    false
}
