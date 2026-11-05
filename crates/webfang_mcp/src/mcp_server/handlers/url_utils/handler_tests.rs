//! Tests for the six URL-utility tool handlers, via `test_support`
//! (`test_handler` + `result_text`).
//!
//! Extracted from `url_utils.rs` to bring it under the ~500-line file-size
//! rule. Movement is verbatim; the module path `url_utils::handler_tests`
//! is preserved via the sibling declaration, and the `use super::*` imports
//! resolve identically from the sibling module.
/// Test helper: build an `McpUrl` from a KNOWN-VALID http(s) string.
fn vu(s: &str) -> crate::mcp_server::params::McpUrl {
    s.parse().expect("test url must be valid http(s)")
}

use super::*;
use crate::mcp_server::handlers::test_support::{result_text, test_handler};

use rmcp::handler::server::wrapper::Parameters;

#[tokio::test]
async fn validate_url_valid() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .validate_url(Parameters(ValidateUrlParams {
            url: "https://example.com/path?q=1".to_string(),
        }))
        .await
        .expect("validate_url returns Ok");
    let text = result_text(&res);
    assert!(
        text.contains("\"valid\": true"),
        "valid url flagged: {text}"
    );
    assert!(text.contains("example.com"), "host extracted: {text}");
}

#[tokio::test]
async fn validate_url_invalid_returns_json_not_error() {
    let (handler, _tmp) = test_handler().await;
    // Bug #7 regression: invalid URL must return a tool-level result
    // with valid:false, NOT a protocol-level McpError (issue #590).
    let res = handler
        .validate_url(Parameters(ValidateUrlParams {
            url: "not a url".to_string(),
        }))
        .await
        .expect("invalid URL must return Ok with JSON payload, not Err");
    let text = result_text(&res);
    assert!(
        text.contains("\"valid\": false"),
        "invalid URL must report valid:false, got: {text}"
    );
    assert!(
        text.contains("reason"),
        "invalid URL must include a reason field, got: {text}"
    );
}

#[tokio::test]
async fn validate_url_rejects_non_http_scheme() {
    let (handler, _tmp) = test_handler().await;
    // Issue #606: a non-http(s) scheme must be reported valid:false, not
    // accepted as valid:true.
    let res = handler
        .validate_url(Parameters(ValidateUrlParams {
            url: "ftp://example.com/file".to_string(),
        }))
        .await
        .expect("validate_url returns Ok");
    let text = result_text(&res);
    assert!(
        text.contains("\"valid\": false"),
        "ftp scheme must be valid:false, got: {text}"
    );
    assert!(
        text.contains("scheme"),
        "reason must mention the scheme, got: {text}"
    );
}

#[tokio::test]
async fn extract_domain_valid() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .extract_domain(Parameters(ExtractDomainParams {
            url: vu("https://www.example.com/x"),
        }))
        .await
        .expect("extract_domain returns Ok");
    let text = result_text(&res);
    assert_eq!(text, "www.example.com", "domain extracted: {text}");
}

#[tokio::test]
async fn extract_domain_strips_trailing_dot() {
    let (handler, _tmp) = test_handler().await;
    // Issue #606: a fully-qualified domain with a trailing root-label dot
    // must be normalized to strip the dot.
    let res = handler
        .extract_domain(Parameters(ExtractDomainParams {
            url: vu("https://books.toscrape.com./path"),
        }))
        .await
        .expect("extract_domain returns Ok");
    let text = result_text(&res);
    assert_eq!(text, "books.toscrape.com", "trailing dot stripped: {text}");
}

/// #1116: an unparseable URL is unrepresentable at the `McpUrl`
/// boundary — deserialization fails (rmcp → -32602).
#[test]
fn extract_domain_invalid_url_is_unrepresentable() {
    let res = serde_json::from_value::<ExtractDomainParams>(serde_json::json!({
        "url": "not a url"
    }));
    assert!(
        res.is_err(),
        "invalid URL must fail to deserialize: {res:?}"
    );
}

#[tokio::test]
async fn normalize_url_valid() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .normalize_url(Parameters(NormalizeUrlParams {
            url: vu("https://example.com/a/b/#frag"),
        }))
        .await
        .expect("normalize_url returns Ok");
    let text = result_text(&res);
    assert!(!text.contains("#frag"), "fragment must be stripped: {text}");
}

/// #1116: a scheme-less string is unrepresentable as `McpUrl` — the
/// "no scheme" case that used to be a runtime guard is now a type error
/// at deserialization.
#[test]
fn normalize_url_no_scheme_is_unrepresentable() {
    let res = serde_json::from_value::<NormalizeUrlParams>(serde_json::json!({
        "url": "example.com"
    }));
    assert!(
        res.is_err(),
        "URL without scheme must fail to deserialize: {res:?}"
    );
}

#[tokio::test]
async fn match_url_pattern_true() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .match_url_pattern(Parameters(MatchUrlPatternParams {
            url: vu("https://example.com/articles/1"),
            pattern: "**/articles/*".to_string(),
        }))
        .await
        .expect("match_url_pattern returns Ok");
    let text = result_text(&res);
    assert_eq!(text, "true", "pattern should match: {text}");
}

#[tokio::test]
async fn is_internal_link_true() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .is_internal_link(Parameters(IsInternalLinkParams {
            url: vu("https://example.com/x"),
            seed_domain: "example.com".to_string(),
        }))
        .await
        .expect("is_internal_link returns Ok");
    let text = result_text(&res);
    assert_eq!(text, "true", "internal link expected: {text}");
}

#[tokio::test]
async fn url_to_file_path_valid() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .url_to_file_path(Parameters(UrlToFilePathParams {
            url: vu("https://example.com/docs/page"),
        }))
        .await
        .expect("url_to_file_path returns Ok");
    let text = result_text(&res);
    assert!(
        text.contains("example.com"),
        "file path must include domain: {text}"
    );
}

/// G-14 (issue #1608): a non-http(s) scheme is unrepresentable as
/// `McpUrl` — deserialization fails before the handler runs, exactly
/// like every sibling tool on the #1116 boundary.
#[test]
fn url_to_file_path_non_http_scheme_is_unrepresentable() {
    let res = serde_json::from_value::<UrlToFilePathParams>(serde_json::json!({
        "url": "file:///etc/passwd"
    }));
    assert!(
        res.is_err(),
        "file:// URL must fail to deserialize: {res:?}"
    );
}

/// PI-12 (#1601): the user-visible path/claim of `url_to_file_path` must
/// derive from the sanitized artifact filename — never embed the raw,
/// attacker-controlled URL. A hostile URL with percent-encoded traversal
/// and control characters must surface only its neutralized form: no raw
/// URL text, no encoded traversal bytes, no control characters in the
/// reported path.
#[tokio::test]
async fn url_to_file_path_hostile_url_reports_only_the_sanitized_path() {
    let (handler, _tmp) = test_handler().await;
    let raw = "https://example.com/..%2F..%2Fetc%2Fpasswd%0Ahidden";
    let res = handler
        .url_to_file_path(Parameters(UrlToFilePathParams { url: vu(raw) }))
        .await
        .expect("url_to_file_path returns Ok");
    let text = result_text(&res);

    assert!(
        !text.contains(raw),
        "the raw attacker-controlled URL must never appear in the response: {text}"
    );
    for encoded in ["%2F", "%2f", "%0A", "%0a"] {
        assert!(
            !text.contains(encoded),
            "encoded hostile byte {encoded} must be neutralized away: {text}"
        );
    }
    // The claim is the reported path itself: parse the JSON payload (the
    // pretty-printed envelope would otherwise trip a whole-text control
    // check with its own newlines) and assert on `full_path`.
    let payload: serde_json::Value =
        serde_json::from_str(&text).expect("payload must be the JSON path object");
    let full_path = payload
        .get("full_path")
        .and_then(|v| v.as_str())
        .expect("payload must carry full_path");
    assert!(
        !full_path.chars().any(char::is_control),
        "no control character may survive into the reported path: {full_path:?}"
    );
    // The claim still derives from the sanitized artifact filename under
    // the domain directory — the path stays inside ./output/<domain>/.
    assert!(
        full_path.starts_with("./output/example.com/"),
        "path must stay confined under the domain directory: {full_path}"
    );
    assert!(
        full_path.contains("etc-passwd"),
        "the sanitized filename (traversal flattened, controls replaced) is the claim: {full_path}"
    );
}
