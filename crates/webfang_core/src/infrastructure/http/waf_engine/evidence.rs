//! Body and header evidence collection for WAF detection.
//! This module implements the Aho-Corasick pass with boundary post-filtering
//! (REQ-WAF-04), config-string detection (#781), and control-header evidence
//! collection (REQ-WAF-03).

use std::collections::HashMap;

use aho_corasick::Match;

use crate::domain::waf::{EvidenceSource, WafEvidence, WafTier};

use super::signatures::{
    BoundaryMode, MAX_EVIDENCE_PER_BODY, WAF_AC, WAF_BODY_SIGNATURES, WAF_CONTROL_HEADERS,
};

/// Collect body-signature evidence in a single Aho-Corasick pass, applying the
/// boundary post-filter (REQ-WAF-04) to Fingerprint `[B]` matches.
///
/// Returns the evidence vector plus a `has_genuine_fingerprint` flag used by the
/// entropy rule (REQ-WAF-06a, #781): a Fingerprint-tier match that is merely a
/// quoted JS/JSON **config value** (e.g. MediaWiki's
/// `"wgConfirmEditHCaptchaSiteKey": "hcaptcha"`) is still recorded — it may
/// serve the evidence chain of a status-correlated block (403/429/503) — but it
/// is NOT genuine challenge context, so it can no longer escalate a large
/// high-entropy legitimate page into an obfuscated-WAF block at HTTP 200.
///
/// The whole body is scanned: [`MAX_EVIDENCE_PER_BODY`] caps only the
/// *Fingerprint* (T2) accumulation (RISK-01 DoS hardening), never detection. A
/// [`WafTier::Challenge`] (T1) marker is a definitive verdict — the moment one
/// survives the filter it clears any accumulated T2 noise and short-circuits the
/// pass, so a T1 marker past 64 earlier T2 matches is still seen (the cap is not
/// a detection limit). T2 matches keep accumulating (bounded by the cap) and the
/// scan continues past the cap so a later T1 marker is still examined. A verdict
/// needs at most one T1 marker or one correlated T2 marker, and the T2 cap is far
/// beyond any real challenge page's signal count.
pub fn collect_body_evidence(body: &str) -> (Vec<WafEvidence>, bool) {
    let mut evidences = Vec::new();
    let mut has_genuine_fingerprint = false;
    for mat in WAF_AC.find_iter(body) {
        let sig = &WAF_BODY_SIGNATURES[mat.pattern()];
        if !passes_boundary_filter(body, &mat, sig.2, sig.3) {
            continue;
        }
        match sig.2 {
            // T1 is a definitive verdict that noise density cannot hide: drop any
            // accumulated T2 noise and short-circuit on the first T1 marker.
            WafTier::Challenge => {
                evidences.clear();
                evidences.push(WafEvidence {
                    provider: sig.1,
                    tier: sig.2,
                    matched_pattern: sig.0,
                    source: EvidenceSource::Body,
                });
                return (evidences, true);
            },
            // T2 accumulation is capped (RISK-01); keep scanning past the cap so a
            // later T1 marker is still examined.
            WafTier::Fingerprint => {
                // #781: a quoted config-string occurrence is evidence (feeds the
                // block chain under a WAF status) but not genuine challenge
                // context for the entropy rule.
                has_genuine_fingerprint |= !is_config_string_match(body, &mat);
                if evidences.len() < MAX_EVIDENCE_PER_BODY {
                    evidences.push(WafEvidence {
                        provider: sig.1,
                        tier: sig.2,
                        matched_pattern: sig.0,
                        source: EvidenceSource::Body,
                    });
                }
            },
        }
    }
    (evidences, has_genuine_fingerprint)
}

/// Whether a body match is a quoted JS/JSON **config-value** occurrence rather
/// than a served challenge (#781).
///
/// A match is a config string when it fills an ENTIRE double-quoted string in
/// the value position of a quoted-key object entry: `"key": "<match>"`. This is
/// exactly the shape of MediaWiki's captcha config dump
/// (`"wgConfirmEditCaptchaNeededForGenericEdit": "hcaptcha"`). Real challenge
/// occurrences never take this shape: widget markup (`class="h-captcha"`,
/// `data-sitekey="..."`) embeds the marker in an HTML attribute value (preceded
/// by `=`, not `:`), script sources (`hcaptcha.com/1/api.js`) carry a URL
/// prefix before the opening quote, and challenge prose is unquoted. The filter
/// is conservative: anything ambiguous stays a genuine match.
///
/// # Why the JS-assignment shape is deliberately NOT handled (G-24, #1615)
///
/// An earlier revision of this comment also promised the JS-assignment shape
/// (`ident = "hcaptcha"`), and the implementation never grew it. G-24 offers
/// "implement it or narrow the comment"; the comment is narrowed, and the
/// reason is a security one rather than a scheduling one:
///
/// `ident = "hcaptcha"` and `data-sitekey="abc"` are separated only by spaces
/// around the `=`. Accepting the JS shape therefore means accepting every HTML
/// attribute whose value is a bare vendor token — which is precisely how
/// Cloudflare Turnstile, hCaptcha and reCAPTCHA ship their site keys. Treating
/// those as "configuration" filters OUT a live challenge marker, and the error
/// direction is wrong: this filter exists to prevent false POSITIVES, and
/// widening it buys nothing while risking false NEGATIVES on real challenges.
///
/// A whitespace-sensitive rule (`=` must be padded on both sides) would
/// separate the two in practice, but it is a heuristic about formatting
/// conventions, and a challenge page that minifies to `data-sitekey="x"` is
/// already served by the current, stricter rule. Adding the looser rule would
/// trade a real detection guarantee for a LOW-severity documentation defect.
///
/// So the JSON shape is the whole contract, and the comment now says so. If the
/// JS shape is ever wanted, it needs a rule that cannot also match an
/// attribute — not a relaxed separator.
///
/// The scan walks a bounded constant number of bytes backwards from the match,
/// so the Aho-Corasick pass remains the only O(N) walk over the body.
#[inline]
pub fn is_config_string_match(body: &str, mat: &Match) -> bool {
    let bytes = body.as_bytes();
    // The match must fill an ENTIRE double-quoted string: `"<match>"`.
    if mat.start() == 0 || bytes[mat.start() - 1] != b'"' || bytes.get(mat.end()) != Some(&b'"') {
        return false;
    }
    // Before the opening quote (whitespace-tolerant) must stand a colon.
    let Some(colon) = skip_back(bytes, mat.start() - 1, is_ws) else {
        return false;
    };
    if bytes[colon] != b':' {
        return false;
    }
    // Before the colon (whitespace-tolerant) must stand the key's closing quote.
    let Some(key_close) = skip_back(bytes, colon, is_ws) else {
        return false;
    };
    if bytes[key_close] != b'"' {
        return false;
    }
    // Walk back over the key's identifier chars to its opening quote; a
    // non-empty identifier key is required (`"": "value"` is not a config
    // entry of any known captcha plugin).
    let Some(key_open) = skip_back(bytes, key_close, is_ident_char) else {
        return false;
    };
    bytes[key_open] == b'"' && key_open + 1 < key_close
}

/// Scan backward from (but excluding) `from` for the first byte NOT satisfying
/// `skip`; `None` when the entire prefix satisfies `skip`.
#[inline]
fn skip_back(bytes: &[u8], from: usize, skip: impl Fn(u8) -> bool) -> Option<usize> {
    let mut i = from;
    while i > 0 {
        i -= 1;
        if !skip(bytes[i]) {
            return Some(i);
        }
    }
    None
}

/// Identifier characters accepted in JS/JSON config keys and variable names.
#[inline]
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// ASCII whitespace predicate (value form for `skip_back`).
#[inline]
fn is_ws(b: u8) -> bool {
    b.is_ascii_whitespace()
}

/// O(1) adjacent-byte boundary post-filter for Fingerprint-tier `[B]` matches
/// (REQ-WAF-04).
///
/// Rejects a bare vendor-name match when the byte immediately before its start
/// or after its end is ASCII alphanumeric or `_` — this is what stops
/// `akamai_hash` from matching bare `akamai`. UTF-8 safe: any non-ASCII byte
/// (>= 0x80) counts as a boundary. `[E]` (Exempt) patterns and all
/// Challenge-tier patterns skip the filter entirely.
#[inline]
pub fn passes_boundary_filter(
    body: &str,
    mat: &Match,
    tier: WafTier,
    boundary: BoundaryMode,
) -> bool {
    // Only Fingerprint-tier bare vendor names get the filter.
    if tier != WafTier::Fingerprint || boundary != BoundaryMode::Bare {
        return true;
    }
    let bytes = body.as_bytes();
    if mat.start() > 0 {
        let before = bytes[mat.start() - 1];
        if before.is_ascii_alphanumeric() || before == b'_' {
            return false;
        }
    }
    if mat.end() < bytes.len() {
        let after = bytes[mat.end()];
        if after.is_ascii_alphanumeric() || after == b'_' {
            return false;
        }
    }
    true
}

/// Collect control-header evidence (REQ-WAF-03).
///
/// Every control header is [`WafTier::Fingerprint`] evidence — it never
/// auto-blocks on mere presence (correction B), only when correlated with a
/// WAF status code by [`super::verdict::decide`].
pub fn collect_header_evidence(headers: &HashMap<String, String>) -> Vec<WafEvidence> {
    let mut evidences = Vec::new();
    for (name, provider) in WAF_CONTROL_HEADERS {
        if headers.get(*name).is_some() {
            evidences.push(WafEvidence {
                provider,
                tier: WafTier::Fingerprint,
                matched_pattern: name,
                source: EvidenceSource::Header,
            });
        }
    }
    evidences
}
