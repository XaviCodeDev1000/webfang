//! Content-Type gate for WAF body scanning (REQ-WAF-02).
//!
//! Decides whether the response body should be scanned for WAF signatures:
//! 1. `application/xhtml+xml` carve-out → scan (it is HTML despite `+xml`).
//! 2. Skip structured data (`+json`/`+xml` suffix, `application/json`,
//!    `application/xml`, `text/json`, `text/xml`) and binary assets
//!    (`image/*`, `font/*`, `application/wasm`, `application/javascript`).
//! 3. `text/*`, missing content-type, or anything else not denied → scan
//!    (this is a denylist, not an allowlist).
//!
//! The `ignore_waf` short-circuit (REQ-WAF-02 step 1) happens in
//! [`super::inspector::WafInspector::inspect`] before this gate is consulted.

use crate::domain::waf::InspectionContext;

/// Whether the response body should be scanned for WAF signatures.
pub fn should_scan_body(ctx: &InspectionContext) -> bool {
    // Missing content-type → scan.
    let Some(raw) = &ctx.content_type else {
        return true;
    };
    // Strip parameters (e.g. "text/html; charset=utf-8" → "text/html").
    let mime = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    // Carve-out: XHTML is HTML — scan it (checked before the +xml skip).
    if mime == "application/xhtml+xml" {
        return true;
    }
    // Denylist: structured data formats are never HTML challenges.
    let is_json = mime.ends_with("+json") || mime == "application/json" || mime == "text/json";
    let is_xml = mime.ends_with("+xml") || mime == "application/xml" || mime == "text/xml";
    if is_json || is_xml {
        return false;
    }
    // Denylist: binary assets.
    if mime.starts_with("image/") || mime.starts_with("font/") {
        return false;
    }
    if mime == "application/wasm" || mime == "application/javascript" {
        return false;
    }
    // text/* or anything else not denied → scan.
    true
}

/// Whether a content-type denotes HTML (for the 200+HTML silent-challenge rule).
pub fn is_html_content_type(ct: &str) -> bool {
    let mime = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime == "text/html" || mime == "application/xhtml+xml"
}
