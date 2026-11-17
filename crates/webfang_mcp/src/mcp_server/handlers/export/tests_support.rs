use super::*;
use crate::mcp_server::handlers::test_support;
use crate::mcp_server::state::McpState;
use tempfile::TempDir;
use webfang_core::domain::ValidUrl;
use webfang_core::infrastructure::crawler::robots_utils::RobotsFetcher;

/// Test helper: build an `McpUrl` from a KNOWN-VALID http(s) string.
pub(super) fn vu(s: &str) -> crate::mcp_server::params::McpUrl {
    s.parse().expect("test url must be valid http(s)")
}

/// Determinism (#705/#749): the robots check performs a real robots.txt
/// fetch, so plain unit tests drop the fetcher (no-op gate); #749 robots
/// tests wire a fetcher explicitly (see `test_handler_with_robots`).
pub(super) async fn test_handler() -> (McpHandler, TempDir) {
    let (mut state, tmp) = test_state().await;
    state.robots_fetcher = None;
    (McpHandler::new(state), tmp)
}

/// [`test_handler`] with the container's own `output_dir` declared as an
/// export root — the shape of a production deployment whose
/// `--export-roots` covers the configured output directory.
/// `process_export_pipeline` enforces the export-root gate at request
/// time (XP-P-08/G-9, issue #1608), so every test exercising the
/// pipeline needs roots covering the fixture's absolute temp `output_dir`.
pub(super) async fn test_handler_with_export_roots() -> (McpHandler, TempDir) {
    let (mut state, tmp) = test_state().await;
    state.robots_fetcher = None;
    let state = state.with_export_roots(vec![tmp.path().to_path_buf()]);
    (McpHandler::new(state), tmp)
}

/// Build a state with a real robots fetcher for #749 enforcement tests.
/// Offline-friendly: construction never touches the network.
pub(super) async fn test_handler_with_robots() -> (McpHandler, TempDir) {
    let (state, tmp) = test_state().await;
    let state = state
        .with_robots_fetcher(std::sync::Arc::new(
            RobotsFetcher::with_default_profile(5).expect("fetcher construction is offline"),
        ))
        // The pipeline gate (XP-P-08/G-9, #1608) needs roots covering the
        // fixture's absolute output_dir so the robots rejection (not the
        // root gate) is what this test observes.
        .with_export_roots(vec![tmp.path().to_path_buf()]);
    (McpHandler::new(state), tmp)
}

/// Shared state construction for `test_handler` /
/// `test_handler_with_robots`. The `TempDir` is returned so the caller
/// keeps the configured `output_dir` alive.
pub(super) async fn test_state() -> (McpState, TempDir) {
    let tmp = TempDir::new().expect("create temp dir");
    let container = test_support::container(&tmp).await;
    (McpState::new(container), tmp)
}

/// Build one fixture [`ScrapedContent`] for session seeding.
pub(super) fn seed_content(url: &str, title: &str, body: &str) -> ScrapedContent {
    ScrapedContent {
        title: title.to_string(),
        content: body.to_string(),
        url: ValidUrl::try_from_url(url::Url::parse(url).expect("valid")).expect(
            "seed fixture is a plain https URL — validation only rejects non-fetchable schemes",
        ),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
        correlation_id: None,
        quality_hint: None,
    }
}

/// Seed the session buffer with one item — the shape a finished `crawl_site`
/// run leaves behind (#1290; replaces the retired repository seeding and
/// its flush-poll, which an in-memory buffer does not need).
pub(super) fn seed_one(handler: &McpHandler) {
    let mut guard = handler
        .state
        .session_results
        .lock()
        .expect("fresh session lock is never poisoned");
    guard.push(seed_content(
        "https://example.com/seed",
        "Seed",
        "seed body",
    ));
}
