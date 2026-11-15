use super::*;
use crate::mcp_server::handlers::test_support::{self, result_text};
use crate::mcp_server::state::McpState;
use tempfile::TempDir;
use wiremock::MockServer;

/// Test helper: build an `McpUrl` from a KNOWN-VALID http(s) string.
/// Invalid strings can no longer be constructed at all (#1116) — the
/// rejection tests use `serde_json::from_value` instead.
pub(super) fn vu(s: &str) -> crate::mcp_server::params::McpUrl {
    s.parse().expect("test url must be valid http(s)")
}

pub(super) async fn test_handler() -> (McpHandler, TempDir) {
    let (mut state, tmp) = test_state().await;
    // Determinism (#705): handler tests must stay offline — the robots
    // check performs a real robots.txt fetch, so unit tests drop the
    // fetcher instead of risking network access through loopback URLs.
    state.robots_fetcher = None;
    (McpHandler::new(state), tmp)
}

/// Shared state construction for `test_handler`. The `TempDir` is returned
/// so the caller keeps the configured `output_dir` alive.
///
/// Robots-enforcement tests use the shared
/// [`test_support::test_handler_with_robots_and_no_ssrf`] fixture
/// instead of a local robots handler (issue #1885).
pub(super) async fn test_state() -> (McpState, TempDir) {
    let tmp = TempDir::new().expect("create temp dir");
    let container = test_support::container(&tmp).await;
    (McpState::new(container), tmp)
}

// ── #749 robots.txt gate fixtures (modeled on
// `crates/webfang_core/tests/scraper_service_test.rs::robots`) ──

/// Minimal article HTML that Readability extracts deterministically
/// (scraping_coverage_test.rs style).
pub(super) const ARTICLE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>Public Page</title></head>
<body>
<article>
<h1>Main Heading</h1>
<p>This is the content of the article. It has enough text to be extracted by Readability.</p>
</article>
</body>
</html>"#;

/// Count requests that are NOT the robots.txt probe — zero proves the
/// gate short-circuited BEFORE any page fetch.
pub(super) async fn page_hits(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("request recording is enabled")
        .iter()
        .filter(|r| r.url.path() != "/robots.txt")
        .count()
}

/// Assert that a tool result is an honest error (`isError:true`) whose
/// text reports the robots.txt denial.
pub(super) fn assert_robots_error(result: &CallToolResult) -> String {
    let json = serde_json::to_value(result).expect("CallToolResult serializes");
    assert_eq!(
        json.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "robots denial must set isError:true, got: {json}"
    );
    let text = result_text(result);
    assert!(
        text.contains("robots.txt"),
        "error text must report the robots.txt denial, got: {text}"
    );
    text
}

/// Assert that a handler rejected a loopback/private URL via the SSRF gate.
pub(super) fn assert_ssrf_rejected(res: Result<CallToolResult, McpError>) {
    let err = res.expect_err("loopback/internal URL must be blocked by SSRF");
    let msg = err.to_string();
    assert!(
        msg.contains("SSRF"),
        "error must report SSRF protection, got: {msg}"
    );
}
