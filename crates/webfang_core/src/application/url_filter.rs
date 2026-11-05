//! URL filtering module
//!
//! Provides URL pattern matching and filtering functionality for the crawler.
//!
//! # Rules Applied
//!
//! - **own-borrow-over-clone**: Accepts `&str` not `&String`, `&[T]` not `&Vec<T>`
//! - **opt-inline**: Inlines hot path functions
//! - **test-proptest-properties**: Property-based tests for pattern matching
//! - **url-no-string-split**: Uses url crate for RFC 3986 compliant parsing
//! - **security-ssrf-prevention**: Delegates to domain::matches_pattern (SSRF-safe host comparison)

use crate::domain::CrawlerConfig;
use std::sync::atomic::{AtomicBool, Ordering};

static BARE_PATTERN_WARNED: AtomicBool = AtomicBool::new(false);

/// Check if a URL matches a glob-style pattern (dual-mode)
///
/// Two modes depending on whether the pattern starts with `/`:
///
/// - **Path pattern** (starts with `/`): matched against the URL path component.
///   Example: `/pricing`, `/admin/*`, `/api/v2/*`
/// - **Host-only pattern** (no leading `/`): matched against the parsed hostname.
///   Example: `example.com`, `*.example.com`, `*.example.com/*`
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **opt-inline**: Inlined for hot path performance.
/// Following **security-ssrf-prevention**: Delegates to domain implementation (SSRF-safe).
///
/// # Arguments
///
/// * `url` - URL to test
/// * `pattern` - Glob-style pattern (* for wildcard)
///
/// # Returns
///
/// `true` if the URL matches the pattern
///
/// # Examples
///
/// ```
/// use webfang_core::application::url_filter::matches_pattern;
///
/// assert!(matches_pattern("https://example.com/page", "*"));
/// assert!(matches_pattern("https://example.com/pricing", "/pricing"));
/// assert!(matches_pattern("https://blog.example.com/post", "*.example.com"));
/// assert!(!matches_pattern("https://other.com/page", "example.com"));
/// ```
#[inline]
#[must_use]
pub fn matches_pattern(url: &str, pattern: &str) -> bool {
    // Delegate to domain implementation (SSRF-safe)
    crate::domain::matches_pattern(url, pattern)
}

/// Check if a URL is excluded by any of the exclude patterns
///
/// Following **own-borrow-over-clone**: Accepts `&[String]` not `&Vec<String>`.
/// Following **opt-inline**: Inlined for hot path performance.
///
/// # Arguments
///
/// * `url` - URL to test
/// * `patterns` - Slice of exclude patterns
///
/// # Returns
///
/// `true` if the URL matches any exclude pattern
///
/// # Examples
///
/// ```
/// use webfang_core::application::url_filter::is_excluded;
///
/// // To exclude both root domain AND subdomains, need both patterns
/// let patterns = vec![
///     "evil.com".to_string(),
///     "*.evil.com".to_string(),
///     "malicious.com".to_string(),
///     "*.malicious.com".to_string(),
/// ];
/// assert!(is_excluded("https://evil.com/page", &patterns));
/// assert!(is_excluded("https://blog.evil.com/admin", &patterns));
/// assert!(!is_excluded("https://example.com/page", &patterns));
/// ```
#[inline]
#[must_use]
pub fn is_excluded(url: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| matches_pattern(url, pattern))
}

/// Check if a URL is allowed based on the crawler configuration
///
/// Following **own-borrow-over-clone**: Accepts `&CrawlerConfig` reference.
///
/// # Arguments
///
/// * `url` - URL to test
/// * `config` - Crawler configuration with include/exclude patterns
///
/// # Returns
///
/// `true` if the URL is allowed (matches include and doesn't match exclude)
///
/// # Examples
///
/// ```
/// use webfang_core::{application::url_filter::is_allowed, domain::CrawlerConfig};
/// use url::Url;
///
/// let seed = Url::parse("https://example.com").unwrap();
/// let config = CrawlerConfig::builder(seed)
///     // Match both root domain AND subdomains
///     .include_pattern("example.com".to_string())
///     .include_pattern("*.example.com/*".to_string())
///     // Exclude both root domain AND subdomains
///     .exclude_pattern("evil.com".to_string())
///     .exclude_pattern("*.evil.com".to_string())
///     .build();
///
/// assert!(is_allowed("https://example.com/page", &config));
/// assert!(is_allowed("https://blog.example.com/post", &config));
/// assert!(!is_allowed("https://evil.com/page", &config));
/// assert!(!is_allowed("https://blog.evil.com/admin", &config));
/// assert!(!is_allowed("https://other.com/page", &config));
/// ```
#[inline]
#[must_use]
pub fn is_allowed(url: &str, config: &CrawlerConfig) -> bool {
    if !BARE_PATTERN_WARNED.swap(true, Ordering::SeqCst) {
        let bare: Vec<&str> = config
            .exclude_patterns
            .iter()
            .chain(config.include_patterns.iter())
            .filter(|p| !p.starts_with('/') && !p.starts_with("*/") && !p.starts_with("*.'"))
            .map(|p| p.as_str())
            .collect();
        if !bare.is_empty() {
            tracing::warn!(
                "Patterns without a leading / or */ are matched against hostname only, not the URL path. Use /pricing or */pricing for path matching. Affected: {:?}",
                bare
            );
        }
    }

    if is_excluded(url, &config.exclude_patterns) {
        return false;
    }
    config.matches_include(url)
}

/// Extract domain from URL using url crate (RFC 3986 compliant)
///
/// Following **url-no-string-split**: Uses `url::Url::parse().host_str()`
/// instead of string splitting for proper RFC 3986 compliance.
///
/// Handles:
/// - Credentials: `http://user:pass@domain.com` → `domain.com`
/// - Ports: `https://domain.com:8080/path` → `domain.com`
/// - IPv6: `http://[::1]:8080` → `[::1]`
///
/// # Arguments
///
/// * `url` - URL to extract domain from
///
/// # Returns
///
/// Domain string or None if URL is invalid
///
/// # Examples
///
/// ```
/// use webfang_core::application::url_filter::extract_domain;
///
/// assert_eq!(extract_domain("https://example.com/page"), Some("example.com".to_string()));
/// assert_eq!(extract_domain("https://blog.example.com/post"), Some("blog.example.com".to_string()));
/// assert_eq!(extract_domain("http://user:pass@domain.com:8080/path"), Some("domain.com".to_string()));
/// assert_eq!(extract_domain("http://[::1]:8080"), Some("[::1]".to_string()));
/// ```
#[inline]
#[must_use]
pub fn extract_domain(url: &str) -> Option<String> {
    crate::domain::url_validation::extract_domain(url)
}

/// Check if a URL is internal (same domain as seed)
///
/// Following **own-borrow-over-clone**: Accepts `&str` for both parameters.
///
/// # Arguments
///
/// * `url` - URL to check
/// * `seed_domain` - Domain of the seed URL
///
/// # Returns
///
/// `true` if the URL belongs to the same domain
///
/// # Examples
///
/// ```
/// use webfang_core::application::url_filter::is_internal_link;
///
/// assert!(is_internal_link("https://example.com/page", "example.com"));
/// assert!(is_internal_link("https://www.example.com/page", "example.com"));
/// assert!(!is_internal_link("https://other.com/page", "example.com"));
/// ```
#[inline]
#[must_use]
pub fn is_internal_link(url: &str, seed_domain: &str) -> bool {
    crate::domain::url_validation::is_internal_link(url, seed_domain)
}

#[cfg(test)]
mod tests;
