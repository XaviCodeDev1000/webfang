/// Entropy-based challenge detection (REQ-WAF-06).
/// Returns evidence ONLY when a rule decides to block (so its presence in a
/// verdict always means "blocked"):
/// - Rule (a): body > 100KB AND entropy > 5.5 b/B → block if
///   `is_t2_blocking_status(status)` (403/429/503/520-529) OR
///   `has_genuine_fingerprint` — a Fingerprint in genuine challenge context: a
///   non-config-string body match (#781 — a quoted JS/JSON config value like
///   `"wgConfirmEdit...":"hcaptcha"` is NOT genuine). A control header alone
///   does NOT qualify at status 200 (S11): a correlated WAF status is required
///   for header-sourced promotion. This keeps dense legitimate pages
///   (Wikipedia) from escalating to an obfuscated-WAF block on HTTP 200.
/// - Rule (b): body < 1500B AND > 5 `<script>` tags → block if
///   `is_t2_blocking_status(status)` OR (200 + HTML content-type). The
///   200+HTML case is the "H3 fix" silent challenge and MUST be kept
///   (`discovery.rs` depends on it).
///
/// # Why the status test is a WAF-correlated set, not "not 200" (F8, #1615)
///
/// Both rules used to block on `status != Some(200)`, with unknown status
/// counted as "not 200". That fires on *any* non-200 response: a 404 page from
/// a large site, a 301 to a canonical URL, a 500 from an origin that is simply
/// broken. A >100KB high-entropy body is completely ordinary for all three —
/// modern error pages ship CSS, fonts and JSON — so a site returning a large
/// 404 was reported as an "Obfuscated WAF" challenge and its content skipped.
/// The same shape fired in degraded mode, where the caller supplied no status
/// at all, which also contradicted the documented contract that degraded mode
/// blocks only on unambiguous challenge markers.
///
/// The shared [`crate::domain::waf::is_t2_blocking_status`] is the right predicate because it is
/// already this repo's statement of "this status correlates with a WAF", and it
/// is what the Fingerprint tier uses. Both rules now block on that correlation
/// or on genuine challenge evidence — never on a status merely differing from
/// 200.
///
/// # What is given up, stated plainly
///
/// A challenge page served as 500, 502, 418 or 404 is no longer caught by the
/// entropy heuristic alone. 5xx is excluded because `RES-01` already
/// established that 5xx bodies are dominated by vendor diagnostic noise, so
/// blocking there trades a small detection loss for removing a large class of
/// false positive. A WAF that wants to be caught here still is, on 403/429/503
/// and 520-529, and any genuine fingerprint or control header still blocks at
/// any status. A detection loss on an exotic status is the cheaper error than
/// refusing to crawl a third of the web.
use crate::domain::waf::InspectionContext;

use super::content_type::is_html_content_type;
use super::signatures::{
    ENTROPY_THRESHOLD, SILENT_CHALLENGE_MAX_BYTES, SILENT_CHALLENGE_MIN_SCRIPTS,
    SUSPICIOUS_SIZE_THRESHOLD, WAF_CONTROL_HEADERS,
};

/// Entropy-based challenge detection (REQ-WAF-06).
pub fn entropy_evidence(
    body: &str,
    ctx: &InspectionContext,
    has_genuine_fingerprint: bool,
) -> Option<crate::domain::waf::WafEvidence> {
    // Rule (a): large + high entropy → obfuscated WAF.
    if body.len() > SUSPICIOUS_SIZE_THRESHOLD {
        let entropy = calculate_entropy(body);
        if entropy > ENTROPY_THRESHOLD {
            // S11: at status 200 the caller's `has_genuine_fingerprint` may have
            // been set by control-header presence alone (`cf-mitigated` etc.).
            // A control header is T2 context, not a WAF-correlated status, so
            // it must not promote a high-entropy body to T1 there. Only a
            // genuine BODY fingerprint may do that; a correlated status is
            // required for header-sourced promotion.
            let genuine_fingerprint = if ctx.status == Some(200)
                && WAF_CONTROL_HEADERS
                    .iter()
                    .any(|(name, _)| ctx.headers.contains_key(*name))
            {
                super::evidence::collect_body_evidence(body).1
            } else {
                has_genuine_fingerprint
            };
            if crate::domain::waf::is_t2_blocking_status(ctx.status) || genuine_fingerprint {
                return Some(crate::domain::waf::WafEvidence {
                    provider: "Obfuscated WAF",
                    tier: crate::domain::waf::WafTier::Challenge,
                    // G-25 / F8: no threshold in the label. This string reaches
                    // `WafVerdict::evidence_chain`, which the HTTP client, the
                    // scraper service, the crawler and the MCP tool all pass to
                    // the user — so `>100KB` and `>5.5 b/B` were a published
                    // specification of the rule, letting a hostile page tune
                    // itself just under both. The rule still runs on those
                    // numbers; it just no longer advertises them.
                    matched_pattern: "high-entropy body",
                    source: crate::domain::waf::EvidenceSource::Body,
                });
            }
            // Informational detection (REQ-WAF-08): high entropy at a status
            // that does not correlate with a WAF, and without genuine challenge
            // context, is logged and not blocked.
            tracing::debug!(
                entropy,
                status = ?ctx.status,
                "high-entropy body without WAF-correlated status or genuine challenge \
                 context; not blocking"
            );
        }
    }

    // Rule (b): tiny body dense with <script> tags → silent challenge.
    if body.len() < SILENT_CHALLENGE_MAX_BYTES {
        let script_count = body.matches("<script").count();
        if script_count > SILENT_CHALLENGE_MIN_SCRIPTS {
            let is_html = ctx
                .content_type
                .as_deref()
                .is_some_and(is_html_content_type);
            if crate::domain::waf::is_t2_blocking_status(ctx.status)
                || (ctx.status == Some(200) && is_html)
            {
                return Some(crate::domain::waf::WafEvidence {
                    provider: "Silent Challenge",
                    tier: crate::domain::waf::WafTier::Challenge,
                    // Same G-25 reasoning as rule (a): the byte count and the
                    // script count are the rule, not the finding.
                    matched_pattern: "script-density",
                    source: crate::domain::waf::EvidenceSource::Body,
                });
            }
        }
    }

    None
}

/// Calculate Shannon entropy of a string
///
/// Used to detect obfuscated JavaScript in challenge pages, which often have
/// high entropy due to minification and encoding.
///
/// # Arguments
/// * `s` - The string to analyze
///
/// # Returns
/// * Entropy value between 0.0 (uniform distribution) and ~8.0 (high entropy)
#[inline]
pub fn calculate_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }

    let mut freq = [0u32; 256];
    let len = s.len() as f64;

    for &b in s.as_bytes() {
        freq[b as usize] += 1;
    }

    let mut entropy = 0.0;
    for &count in &freq {
        if count > 0 {
            let p = count as f64 / len;
            entropy -= p * p.log2();
        }
    }

    entropy
}
