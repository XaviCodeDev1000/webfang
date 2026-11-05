//! Tests for the `webfang_core::domain::url_validation` classification
//! helpers (`normalize_seed_host`, `is_internal_link`).
//!
//! Extracted from `url_utils.rs` to bring it under the ~500-line file-size
//! rule. Movement is verbatim; the module path `url_utils::tests` is
//! preserved via the sibling declaration.
use webfang_core::domain::url_validation::normalize_seed_host;

#[test]
fn normalize_seed_host_bare_domain() {
    assert_eq!(normalize_seed_host("example.com"), "example.com");
}

#[test]
fn normalize_seed_host_with_scheme() {
    assert_eq!(normalize_seed_host("https://example.com"), "example.com");
}

#[test]
fn normalize_seed_host_with_scheme_and_path() {
    assert_eq!(
        normalize_seed_host("https://example.com/path"),
        "example.com"
    );
}

#[test]
fn normalize_seed_host_bare_with_path() {
    assert_eq!(normalize_seed_host("example.com/path"), "example.com");
}

#[test]
fn normalize_seed_host_subdomain() {
    assert_eq!(normalize_seed_host("blog.example.com"), "blog.example.com");
}

#[test]
fn normalize_seed_host_with_scheme_subdomain() {
    assert_eq!(
        normalize_seed_host("https://blog.example.com"),
        "blog.example.com"
    );
}

#[test]
fn normalize_seed_host_empty() {
    assert_eq!(normalize_seed_host(""), "");
}

// --- Integration-style tests for the classification logic ---

fn classify(url: &str, seed_domain: &str) -> bool {
    webfang_core::domain::url_validation::is_internal_link(url, seed_domain)
}

#[test]
fn internal_link_bare_domain() {
    assert!(classify("https://example.com/a", "example.com"));
}

#[test]
fn internal_link_subdomain_bare() {
    assert!(classify("https://blog.example.com/x", "example.com"));
}

#[test]
fn internal_link_with_scheme() {
    assert!(classify("https://example.com/a", "https://example.com"));
}

#[test]
fn external_link() {
    assert!(!classify("https://otro.com/a", "example.com"));
}

#[test]
fn internal_link_seed_with_path() {
    assert!(classify("https://example.com/a", "example.com/path"));
}

#[test]
fn invalid_url_returns_false() {
    assert!(!classify("not-a-url", "example.com"));
}

#[test]
fn port_in_url_does_not_break() {
    assert!(classify("https://example.com:8080/a", "example.com"));
}
