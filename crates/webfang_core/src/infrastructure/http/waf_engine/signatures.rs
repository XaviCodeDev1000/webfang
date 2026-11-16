//! WAF signature registry and Aho-Corasick automaton.
//!
//! This module contains the unified body signature registry (`WAF_BODY_SIGNATURES`),
//! control headers (`WAF_CONTROL_HEADERS`), boundary mode classification, and the
//! compiled Aho-Corasick automaton (`WAF_AC`) for O(N) multi-pattern matching.

use aho_corasick::AhoCorasick;
use std::collections::HashSet;
use std::sync::LazyLock;

use crate::domain::waf::WafTier;

/// Control headers that indicate WAF processing (REQ-WAF-03).
///
/// Every control header is [`WafTier::Fingerprint`] evidence — it NEVER
/// auto-blocks on mere presence, only when correlated with a WAF status code
/// (correction B). The retained headers correlate with *active* processing or
/// mitigation: a DataDome response, a Cloudflare mitigation flag, an Akamai
/// edge-auth challenge — so status-correlated blocking is the intended #346
/// semantics.
///
/// RES-01: `cf-ray` and `x-sucuri-id` were purged. They are ubiquitous trace
/// headers — present on 100% of Cloudflare / Sucuri-proxied traffic, including
/// every genuine transient origin failure — so a fingerprint that rides on all
/// of a vendor's traffic carries zero challenge evidence. Treating them as
/// Fingerprint evidence block-correlated every real 503 behind those vendors
/// (instant `WafChallenge`, zero retries). They are noise, not signal.
/// `x-wordpress` (not a real WAF header) and `x-cdn` (generic CDN header) were
/// purged earlier as false-positive risks.
pub const WAF_CONTROL_HEADERS: &[(&str, &str)] = &[
    ("x-datadome-response", "DataDome"),
    ("cf-mitigated", "Cloudflare"),
    ("x-akamai-edge-auth", "Akamai"),
];

/// Boundary post-filter mode for a body signature (REQ-WAF-04).
///
/// Only [`BoundaryMode::Bare`] patterns (bare vendor names) get the O(1)
/// adjacent-byte boundary filter. [`BoundaryMode::Exempt`] patterns may match
/// inside a larger token (cookies/tokens); [`BoundaryMode::Phrase`] patterns
/// are challenge prose / widget markers that need no filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryMode {
    /// Bare vendor name — apply the boundary post-filter.
    Bare,
    /// Boundary-exempt — may match inside a larger token (`[E]`).
    Exempt,
    /// Phrase/exact — no filter needed (`—`).
    Phrase,
}

/// Unified WAF body signature registry (REQ-WAF-03 PRIMARY, REQ-WAF-10).
///
/// Each entry is `(pattern, provider, tier, boundary_mode)`:
/// - `tier`: [`WafTier::Challenge`] (T1 — blocks any status) or
///   [`WafTier::Fingerprint`] (T2 — evidence only, blocks with a WAF status).
/// - `boundary_mode`: [`BoundaryMode::Bare`] → O(1) boundary post-filter
///   (REQ-WAF-04); `Exempt`/`Phrase` → no filter.
///
/// Reclassified per the spec table; the 14 DEL entries are purged; the
/// `spa_detector` `WAF_MARKERS` are folded in (REQ-WAF-10). Aho-Corasick is
/// case-sensitive, so case-variant prose ("Checking your browser" vs
/// "checking your browser") are distinct entries.
pub const WAF_BODY_SIGNATURES: &[(&str, &str, WafTier, BoundaryMode)] = &[
    // ── Cloudflare ──
    (
        "cf-turnstile",
        "Cloudflare Turnstile",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "challenge-platform",
        "Cloudflare JS Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "Just a moment...",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "Checking your browser",
        "Cloudflare",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "__cf_chl_f_tk",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Exempt,
    ),
    (
        "cf-browser-verification",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "_cf_chl_opt",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "cloudflare",
        "Cloudflare",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ),
    // ── Google reCAPTCHA ──
    (
        "g-recaptcha",
        "reCAPTCHA",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "recaptcha/api.js",
        "reCAPTCHA",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "grecaptcha.execute",
        "reCAPTCHA",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "recaptcha.net",
        "reCAPTCHA",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "recaptcha Enterprise",
        "reCAPTCHA",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    // ── hCaptcha ──
    (
        "hcaptcha.com",
        "hCaptcha",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "h-captcha",
        "hCaptcha",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "hcaptcha-api",
        "hCaptcha",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "hcaptcha.js",
        "hCaptcha",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "hcaptcha",
        "hCaptcha",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ), // folded from spa_detector
    (
        "h-captcha-response",
        "hCaptcha",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ), // hidden response field injected by the hCaptcha widget
    // ── DataDome ──
    (
        "datadome",
        "DataDome",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ),
    (
        "dd-captcha",
        "DataDome",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "datadome.co",
        "DataDome",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    // ── PerimeterX / HUMAN Security ──
    (
        "perimeterx",
        "PerimeterX",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ),
    (
        "_pxCaptcha",
        "PerimeterX",
        WafTier::Challenge,
        BoundaryMode::Exempt,
    ),
    (
        "px-captcha",
        "PerimeterX",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "perimeterx.net",
        "PerimeterX",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "human-security",
        "HUMAN",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "px-init",
        "PerimeterX",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    // ── Akamai Bot Manager ──
    (
        "_abck",
        "Akamai Bot Manager",
        WafTier::Fingerprint,
        BoundaryMode::Exempt,
    ),
    (
        "SensorData",
        "Akamai Bot Manager",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "akamai-bot-manager",
        "Akamai Bot Manager",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "akamai.net",
        "Akamai",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    ("akamai", "Akamai", WafTier::Fingerprint, BoundaryMode::Bare),
    // ── Imperva / Incapsula ──
    (
        "imperva",
        "Imperva",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ),
    (
        "incapsula",
        "Imperva",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "_Incapsula_Resource",
        "Imperva",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "visid_incap",
        "Imperva Incapsula",
        WafTier::Fingerprint,
        BoundaryMode::Exempt,
    ),
    (
        "incap_ses",
        "Imperva Incapsula",
        WafTier::Fingerprint,
        BoundaryMode::Exempt,
    ),
    // ── Sucuri ──
    ("sucuri", "Sucuri", WafTier::Fingerprint, BoundaryMode::Bare),
    (
        "sucuri.net",
        "Sucuri",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    // ── F5 ──
    (
        "BIGipServer",
        "F5",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    // ── Generic challenge prose / scripts ──
    (
        "Please verify you are a human",
        "Generic Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "verify you are human",
        "Generic Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "checking your browser",
        "Browser Verification",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "captcha-delivery",
        "Challenge Delivery",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "__js_challenge__",
        "JS Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "challenge.js",
        "Generic Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "captcha.js",
        "Generic Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "challenge-running",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ), // folded from spa_detector
    (
        "data-sitekey",
        "Generic Captcha",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ), // folded from spa_detector
    // ── AWS WAF ──
    (
        "awsWafCookieDomainList",
        "AWS WAF",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "AwsWafIntegration",
        "AWS WAF",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "gokuProps",
        "AWS WAF",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "aws-waf-token",
        "AWS WAF",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
];

/// Shannon entropy threshold for obfuscated WAF detection
pub const ENTROPY_THRESHOLD: f64 = 5.5;

/// Body size threshold (100KB) above which entropy analysis is applied
pub const SUSPICIOUS_SIZE_THRESHOLD: usize = 100_000;

/// Maximum body size (bytes) for silent-challenge script-density analysis (REQ-WAF-06b)
pub const SILENT_CHALLENGE_MAX_BYTES: usize = 1500;

/// Minimum `<script>` tag count to flag a silent challenge (REQ-WAF-06b)
pub const SILENT_CHALLENGE_MIN_SCRIPTS: usize = 5;

/// Maximum number of Fingerprint (T2) evidences accumulated per inspection pass
/// (RISK-01 — DoS-hardening bound on the evidence allocation).
///
/// This caps the T2 allocation, NOT detection sensitivity: the whole body is
/// scanned, and a [`WafTier::Challenge`] (T1) marker short-circuits the pass the
/// moment it survives the boundary filter, so a T1 marker past this many earlier
/// T2 matches is still detected (see [`collect_body_evidence`]). A verdict needs
/// at most one T1 marker (blocks any status) or one [`WafTier::Fingerprint`]
/// marker correlated with a WAF status, and 64 is far beyond any real challenge
/// page's signal count. Without the cap the all-evidence collector materialized
/// every Aho-Corasick match into an uncapped `Vec` (~3-4x body size on
/// adversarial text/html under concurrency); the old first-hit detector was
/// O(1). This bound keeps the worst case bounded-linear.
pub const MAX_EVIDENCE_PER_BODY: usize = 64;

/// Aho-Corasick automaton for O(N) multi-pattern body matching.
///
/// Compiled once from [`WAF_BODY_SIGNATURES`] on first access (thread-safe via
/// `LazyLock`). Signatures are compile-time constants and validated in tests — but a
/// misconfigured signature would silently panic on first use. We therefore
/// make the builder fallible and surface clear diagnostics if it ever happens.
// LCOV_EXCL_START defensive: waf-automaton-build — signature patterns are compile-time constants verified in tests
pub static WAF_AC: LazyLock<AhoCorasick> = LazyLock::new(|| {
    AhoCorasick::new(WAF_BODY_SIGNATURES.iter().map(|(sig, _, _, _)| sig)).unwrap_or_else(|err| {
        panic!(
            "WAF body signature automaton build failed — verify the {} compile-time \
                 signatures in `WAF_BODY_SIGNATURES` (patterns must be valid UTF-8 and \
                 within byte-depth limits). Aho-Corasick build error: {err}",
            WAF_BODY_SIGNATURES.len()
        )
    })
});
// LCOV_EXCL_STOP

/// Get the list of supported WAF providers from the signature registry.
#[must_use]
pub fn supported_providers() -> Vec<&'static str> {
    let mut providers: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for sig in WAF_BODY_SIGNATURES {
        let provider = sig.1;
        if !seen.contains(provider) {
            seen.insert(provider);
            providers.push(provider);
        }
    }
    providers.sort();
    providers
}
