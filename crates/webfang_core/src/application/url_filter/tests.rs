//! Tests for the URL filter surface (`matches_pattern`, `is_excluded`,
//! `is_allowed`, `extract_domain`, `is_internal_link`).
//!
//! Extracted from `url_filter.rs` to bring it under the ~500-line
//! file-size rule. Movement is verbatim; the module path
//! `url_filter::tests` is preserved via the sibling declaration, and the
//! `use super::*` imports resolve identically from the sibling module.
use super::*;
use url::Url;

#[test]
fn test_matches_pattern_wildcard() {
    assert!(matches_pattern("https://example.com/page", "*"));
    assert!(matches_pattern("https://any.domain/any/path", "*"));
}

#[test]
fn test_matches_pattern_domain_wildcard() {
    assert!(matches_pattern(
        "https://blog.example.com/post",
        "*.example.com/*"
    ));
    assert!(matches_pattern(
        "https://sub.example.com/page",
        "*.example.com"
    ));
    assert!(!matches_pattern("https://other.com/page", "*.example.com"));
}

#[test]
fn test_matches_pattern_exact_host() {
    // Exact host match (no path matching in SSRF-safe version)
    // Pattern matches HOST only, path is ignored
    assert!(matches_pattern("https://example.com/page", "example.com"));
    // Same host, different path - STILL MATCHES (host-only comparison)
    assert!(matches_pattern("https://example.com/other", "example.com"));
    // Different host - no match
    assert!(!matches_pattern("https://other.com/page", "example.com"));
}

#[test]
fn test_matches_pattern_empty() {
    assert!(matches_pattern("https://example.com", ""));
}

#[test]
fn test_is_excluded() {
    // SSRF-safe: patterns match HOSTS only
    // *.domain matches subdomains ONLY, not root domain
    // To exclude both root domain AND subdomains, need both patterns
    let patterns = vec![
        "evil.com".to_string(),
        "*.evil.com".to_string(),
        "malicious.com".to_string(),
        "*.malicious.com".to_string(),
        "*.example.com".to_string(), // Exclude all example.com subdomains
    ];

    // Root domains match
    assert!(is_excluded("https://evil.com/page", &patterns));
    assert!(is_excluded("https://malicious.com/data", &patterns));

    // Subdomains match
    assert!(is_excluded("https://blog.evil.com/admin", &patterns));
    assert!(is_excluded("https://blog.example.com/login", &patterns));

    // example.com does NOT match *.example.com (needs subdomain)
    assert!(!is_excluded("https://example.com/public/page", &patterns));
    assert!(!is_excluded("https://good.com/page", &patterns));
}

#[test]
fn test_is_excluded_wildcard_catches_unparseable_url() {
    // #1243: `["*"]` means "exclude everything". A garbage URL used to slip
    // past it because `Url::parse` vetoed the match before the wildcard was
    // ever consulted, so the exclusion filter failed open on bad input.
    let all = vec!["*".to_string()];

    assert!(is_excluded("::garbage::", &all));
    assert!(is_excluded("", &all));
    assert!(is_excluded("https://example.com/page", &all));

    // A targeted exclusion still ignores input it cannot parse: the fix
    // widens nothing beyond the universal wildcard.
    let hosts = vec!["evil.com".to_string(), "*.evil.com".to_string()];
    assert!(!is_excluded("::garbage::", &hosts));
}

#[test]
fn test_is_allowed() {
    let seed = Url::parse("https://example.com").unwrap();

    // Config with include and exclude patterns (HOSTS only)
    // To match both root domain AND subdomains, need both patterns
    let config = CrawlerConfig::builder(seed)
        .include_pattern("example.com".to_string())
        .include_pattern("*.example.com/*".to_string())
        .exclude_pattern("*.evil.com".to_string())
        .build();

    // Allowed: matches include (root domain), doesn't match exclude
    assert!(is_allowed("https://example.com/page", &config));

    // Allowed: matches include (subdomain), doesn't match exclude
    assert!(is_allowed("https://blog.example.com/post", &config));

    // Denied: matches exclude
    assert!(!is_allowed("https://evil.com/page", &config));
    assert!(!is_allowed("https://blog.evil.com/admin", &config));

    // Denied: doesn't match include
    assert!(!is_allowed("https://other.com/page", &config));
}

#[test]
fn test_is_allowed_no_patterns() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::new(seed);

    // No patterns = everything allowed
    assert!(is_allowed("https://example.com/page", &config));
    assert!(is_allowed("https://other.com/page", &config));
}

#[test]
fn test_is_allowed_include_only() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::builder(seed)
        // Match both root domain AND subdomains
        .include_pattern("example.com".to_string())
        .include_pattern("*.example.com/*".to_string())
        .build();

    // Matches include pattern (root domain)
    assert!(is_allowed("https://example.com/page", &config));

    // Matches include pattern (subdomain)
    assert!(is_allowed("https://blog.example.com/post", &config));

    // Doesn't match include pattern
    assert!(!is_allowed("https://other.com/page", &config));
}

#[test]
fn test_is_allowed_exclude_only() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::builder(seed)
        // Exclude both root domain AND subdomains
        .exclude_pattern("evil.com".to_string())
        .exclude_pattern("*.evil.com".to_string())
        .build();

    // Doesn't match exclude pattern
    assert!(is_allowed("https://example.com/page", &config));
    assert!(is_allowed("https://example.com/public/page", &config));

    // Matches exclude pattern (root domain)
    assert!(!is_allowed("https://evil.com/page", &config));

    // Matches exclude pattern (subdomain)
    assert!(!is_allowed("https://blog.evil.com/admin", &config));
}

#[test]
fn test_extract_domain() {
    assert_eq!(
        extract_domain("https://example.com/page"),
        Some("example.com".to_string())
    );
    assert_eq!(
        extract_domain("https://blog.example.com/post"),
        Some("blog.example.com".to_string())
    );
    assert_eq!(
        extract_domain("http://sub.domain.example.com/path"),
        Some("sub.domain.example.com".to_string())
    );
    assert_eq!(extract_domain("invalid-url"), None);
}

#[test]
fn test_extract_domain_with_credentials() {
    assert_eq!(
        extract_domain("http://user:pass@domain.com/path"),
        Some("domain.com".to_string())
    );
}

#[test]
fn test_extract_domain_with_port() {
    assert_eq!(
        extract_domain("https://domain.com:8080/path"),
        Some("domain.com".to_string())
    );
}

#[test]
fn test_extract_domain_ipv6() {
    assert_eq!(
        extract_domain("http://[::1]:8080"),
        Some("[::1]".to_string())
    );
}

#[test]
fn test_is_internal_link() {
    assert!(is_internal_link("https://example.com/page", "example.com"));
    assert!(is_internal_link(
        "https://www.example.com/page",
        "example.com"
    ));
    assert!(is_internal_link(
        "https://blog.example.com/post",
        "example.com"
    ));
    assert!(!is_internal_link("https://other.com/page", "example.com"));
    assert!(!is_internal_link("invalid-url", "example.com"));
}

#[test]
fn test_is_allowed_complex_scenarios() {
    let seed = Url::parse("https://example.com").unwrap();

    // Multiple include patterns (HOSTS only)
    // To match both root domain AND subdomains, need both patterns
    let config = CrawlerConfig::builder(seed)
        .include_pattern("blog.example.com".to_string())
        .include_pattern("*.blog.example.com/*".to_string())
        .include_pattern("docs.example.com".to_string())
        .include_pattern("*.docs.example.com/*".to_string())
        .exclude_pattern("draft.example.com".to_string())
        .exclude_pattern("*.draft.example.com".to_string())
        .build();

    // Allowed: matches blog include (root domain)
    assert!(is_allowed("https://blog.example.com/post-1", &config));

    // Allowed: matches blog include (subdomain)
    assert!(is_allowed("https://my.blog.example.com/post-1", &config));

    // Allowed: matches docs include (root domain)
    assert!(is_allowed("https://docs.example.com/guide", &config));

    // Denied: matches exclude (root domain)
    assert!(!is_allowed("https://draft.example.com/post", &config));

    // Denied: matches exclude (subdomain)
    assert!(!is_allowed("https://my.draft.example.com/guide", &config));

    // Denied: doesn't match any include
    assert!(!is_allowed("https://example.com/shop/products", &config));
    assert!(!is_allowed("https://other.com/page", &config));
}

// ========== SSRF PREVENTION TESTS ==========

#[test]
fn test_matches_pattern_ssrf_bypass_attempt() {
    // Evil URL with query params containing target domain should NOT match
    assert!(!matches_pattern(
        "https://evil.com/?q=example.com/path",
        "*.example.com/*"
    ));

    assert!(!matches_pattern(
        "https://attacker.com/?redirect=example.com/admin",
        "*.example.com/*"
    ));

    assert!(!matches_pattern(
        "https://malicious.com/redirect?url=example.com/secret",
        "*.example.com/*"
    ));
}

#[test]
fn test_is_allowed_exclude_takes_precedence_over_include() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::builder(seed)
        .include_pattern("example.com".to_string())
        .exclude_pattern("admin.example.com".to_string())
        .build();

    // Exclude wins over include
    assert!(!is_allowed("https://admin.example.com/settings", &config));
    // Non-excluded include still passes
    assert!(is_allowed("https://example.com/page", &config));
}

#[test]
fn test_is_allowed_multiple_exclude_patterns() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::builder(seed)
        .exclude_pattern("admin.example.com".to_string())
        .exclude_pattern("draft.example.com".to_string())
        .build();

    assert!(!is_allowed("https://admin.example.com/page", &config));
    assert!(!is_allowed("https://draft.example.com/post", &config));
    assert!(is_allowed("https://public.example.com/page", &config));
}

#[test]
fn test_is_excluded_ssrf_safe() {
    // SSRF bypass attempt should NOT be excluded (different host)
    let patterns = vec!["*.example.com".to_string()];

    // Evil domain should NOT match example.com pattern
    assert!(!is_excluded(
        "https://evil.com/?q=example.com/admin",
        &patterns
    ));

    // Real example.com subdomain SHOULD match
    assert!(is_excluded("https://admin.example.com/page", &patterns));
}
