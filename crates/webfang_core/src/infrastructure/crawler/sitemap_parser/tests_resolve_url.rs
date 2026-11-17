use super::*;

#[test]
fn test_resolve_url_relative_paths() {
    let base = Url::parse("https://example.com/sitemap.xml").unwrap();

    let resolved = resolve_url(&base, "../page").unwrap();
    assert_eq!(resolved.as_str(), "https://example.com/page");

    let resolved = resolve_url(&base, "page.html").unwrap();
    assert_eq!(resolved.as_str(), "https://example.com/page.html");

    let resolved = resolve_url(&base, "/page").unwrap();
    assert_eq!(resolved.as_str(), "https://example.com/page");

    // Protocol-relative URL to different host is blocked by SSRF protection (Bug 3)
    assert!(resolve_url(&base, "//other/page").is_none());
}

#[test]
fn test_resolve_url_empty_input() {
    let base = Url::parse("https://example.com").unwrap();

    assert!(resolve_url(&base, "").is_none());
    assert!(resolve_url(&base, "   ").is_none());
}

// -- Mutation-killing tests for sitemap_parser --

// Gap A: resolve_url — absolute URL passthrough
#[test]
fn test_resolve_url_absolute_passthrough() {
    let base = Url::parse("https://example.com/sitemap.xml").unwrap();

    let resolved = resolve_url(&base, "https://other.com/page").unwrap();
    assert_eq!(resolved.as_str(), "https://other.com/page");

    let resolved = resolve_url(&base, "http://insecure.com/page").unwrap();
    assert_eq!(resolved.as_str(), "http://insecure.com/page");
}

#[test]
fn test_resolve_url_absolute_overrides_base() {
    let base = Url::parse("https://example.com/sitemap.xml").unwrap();
    let resolved = resolve_url(&base, "https://completely-different.org/path").unwrap();
    assert_eq!(resolved.host_str(), Some("completely-different.org"));
}
