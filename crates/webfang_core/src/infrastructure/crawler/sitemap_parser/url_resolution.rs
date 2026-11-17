use super::*;

/// Safely resolve a potentially-relative URL against a base URL.
///
/// Handles all RFC 3986 reference types: absolute, scheme-relative,
/// absolute-path, relative-path. Returns `None` for empty inputs.
///
/// Following **err-result-over-panic**: returns Option, never panics.
/// Following **perf-iter-over-index**: uses early returns, no loops.
///
/// # Arguments
///
/// * `base` - The base URL for resolution context
/// * `input` - The URL or path to resolve
///
/// # Returns
///
/// * `Some(Url)` - Successfully resolved URL
/// * `None` - Empty input or resolution failure
///
/// # Examples
///
/// ```
/// use url::Url;
/// use webfang_core::infrastructure::crawler::sitemap_parser::resolve_url;
///
/// let base = Url::parse("https://example.com/sitemap.xml").unwrap();
/// let resolved = resolve_url(&base, "/page.html").unwrap();
/// assert_eq!(resolved.as_str(), "https://example.com/page.html");
/// ```
#[must_use]
pub fn resolve_url(base: &Url, input: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    // Fast path: already absolute - allow any absolute URL (explicit user intent)
    if input.starts_with("http://") || input.starts_with("https://") {
        return Url::parse(input).ok();
    }

    // RFC 3986 resolution for relative and protocol-relative URLs
    let resolved = base.join(input).ok()?;

    // Bug 3: post-resolution host validation (SSRF protection for protocol-relative/relative URLs)
    let seed_host = base.host_str().unwrap_or_default();
    if is_internal_link(resolved.as_str(), seed_host) {
        Some(resolved)
    } else {
        tracing::warn!(%resolved, "SSRF: protocol-relative URL escaped seed domain");
        None
    }
}
