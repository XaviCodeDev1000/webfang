//! Naming, filters and stream helpers for the asset downloader: URL
//! include/exclude matching, filename derivation (slug / content-disposition /
//! hash), sanitize, backoff computation and the response stream adapter.
//! Split desde `adapters/downloader/mod.rs` bajo la regla ~500 (#2584, Ola 1).

use std::time::Duration;

use wreq::Response;

use crate::domain::ValidUrl;

pub(crate) fn into_stream(
    response: Response,
) -> impl futures::StreamExt<Item = wreq::Result<bytes::Bytes>> {
    response.bytes_stream()
}

/// Check if a URL matches include/exclude filters.
///
/// If `include_patterns` is empty, all URLs pass the include check.
/// `exclude_patterns` are always applied (deny wins).
pub(crate) fn url_matches_filters(
    url: &ValidUrl,
    includes: &[String],
    excludes: &[String],
) -> bool {
    if excludes.iter().any(|p| pattern_matches_asset(url, p)) {
        return false;
    }
    if includes.is_empty() {
        return true;
    }
    includes.iter().any(|p| pattern_matches_asset(url, p))
}

/// Match a URL against a pattern, supporting both extension globs (`*.pdf`)
/// and the standard host/path glob from `domain::pattern_matching`.
pub(crate) fn pattern_matches_asset(url: &ValidUrl, pattern: &str) -> bool {
    let p = pattern.trim();
    // Extension glob: *.ext (but NOT host globs like *.example.com which contain a dot after the prefix)
    if let Some(ext) = p.strip_prefix("*.") {
        if !ext.is_empty() && !ext.contains('.') {
            // The URL is already parsed — the old `Url::parse(url)` re-parse
            // per pattern per asset is gone (#1117).
            let last = url.path().rsplit('/').next().unwrap_or("");
            let low = last.to_ascii_lowercase();
            let ext_low = ext.to_ascii_lowercase();
            return low.ends_with(&format!(".{ext_low}"));
        }
    }
    crate::domain::matches_pattern(url.as_str(), pattern)
}

/// Compute exponential backoff delay with jitter.
pub(crate) fn compute_backoff_delay(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    use rand::Rng;
    use std::cmp::min;

    // Exponential: base * 2^(attempt-1), clamped to max. Shift is capped at 62
    // to avoid panic from shifting u64 by >= 64 bits (attempt=1 is shift=0).
    let shift = (attempt - 1).min(62);
    let delay_ms = min(base_ms.saturating_mul(1u64 << shift), max_ms);
    // Add jitter: 75%-125% of delay, then clamp final result to max_ms
    let jitter = delay_ms / 4;
    let offset = rand::rng().random_range(0..=jitter.saturating_mul(2));
    let final_ms = min(
        delay_ms.saturating_sub(jitter).saturating_add(offset),
        max_ms,
    );
    Duration::from_millis(final_ms)
}

/// Derive a slug from the last path segment of a URL.
pub(crate) fn derive_slug_from_url(url: &ValidUrl) -> String {
    // The URL is already parsed — the old `Url::parse(url).ok()` re-parse
    // and its silent empty-slug fallback are gone (#1117).
    url.path()
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && *s != "/")
        .map(String::from)
        .unwrap_or_default()
}

/// UTF-8 safe percent-decoding for Content-Disposition filenames.
/// Uses the `percent-encoding` crate for correct handling of truncated
/// sequences, multi-byte chars, and all edge cases in RFC 5987/3986.
pub(crate) fn percent_decode_utf8(input: &str) -> String {
    percent_encoding::percent_decode_str(input)
        .decode_utf8_lossy()
        .into_owned()
}

/// Sanitize an untrusted filename into a safe single path component.
///
/// Thin delegate over
/// [`crate::domain::crawler_port::filename::sanitize_filename_component`]
/// — the single source of truth for filename sanitization. Returns an
/// empty string when nothing safe remains (fully hostile input such as
/// `.` / `..`), so callers like [`Downloader::generate_filename`] fall back to
/// hash-based naming.
pub(crate) fn sanitize_filename(name: &str) -> String {
    crate::domain::crawler_port::filename::sanitize_filename_component(name).unwrap_or_default()
}

/// Parse `filename=` from a Content-Disposition header value.
///
/// RFC 6266 / 5987: parameter names are case-insensitive. We lowercase
/// the header value before matching to handle `Filename=`, `FILENAME=`, etc.
pub(crate) fn parse_content_disposition_header(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();

    // Try filename*=utf-8''encoded first (RFC 5987)
    if let Some(start) = lower.find("filename*=utf-8''") {
        let encoded = &value[start + "filename*=utf-8''".len()..];
        let name: String = encoded
            .chars()
            .take_while(|c| *c != ';' && *c != ' ')
            .collect();
        let decoded = percent_decode_utf8(&name);
        return Some(decoded);
    }

    // Try filename="name" or filename=name (case-insensitive)
    let after = lower.find("filename=")?;
    let rest = &value[after + "filename=".len()..];
    let name = if let Some(inner) = rest.strip_prefix('"') {
        // Quoted: filename="name.pdf"
        let end = inner.find('"')?;
        &inner[..end]
    } else {
        // Unquoted: filename=name.pdf
        rest.split(';').next().unwrap_or(rest).trim()
    };

    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// MIME type to file extension mapping
pub(crate) fn mime_type_to_extension(mime: &str) -> Option<String> {
    let mime = mime.trim();
    match mime {
        "image/jpeg" | "image/jpg" => Some("jpg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        "image/svg+xml" => Some("svg".to_string()),
        "image/bmp" => Some("bmp".to_string()),
        "image/tiff" => Some("tiff".to_string()),
        "image/x-icon" => Some("ico".to_string()),
        "application/pdf" => Some("pdf".to_string()),
        "application/msword" => Some("doc".to_string()),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("docx".to_string())
        },
        "application/vnd.ms-excel" => Some("xls".to_string()),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("xlsx".to_string())
        },
        "application/vnd.ms-powerpoint" => Some("ppt".to_string()),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            Some("pptx".to_string())
        },
        "text/csv" => Some("csv".to_string()),
        "application/vnd.oasis.opendocument.text" => Some("odt".to_string()),
        "application/vnd.oasis.opendocument.spreadsheet" => Some("ods".to_string()),
        "application/epub+zip" => Some("epub".to_string()),
        "application/rtf" => Some("rtf".to_string()),
        "text/plain" => Some("txt".to_string()),
        "application/json" => Some("json".to_string()),
        "application/xml" | "text/xml" => Some("xml".to_string()),
        _ => None,
    }
}
