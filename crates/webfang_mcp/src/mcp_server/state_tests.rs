use super::*;
use tempfile::TempDir;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::CrawlerConfig;
use webfang_core::infrastructure::scraper::dom_inspector::NoOpInspector;

/// Build a minimal Container for testing (async, needs a temp output dir).
async fn test_container() -> (TempDir, Container) {
    let tmp = TempDir::new().expect("create temp dir");
    let crawler_config = CrawlerConfig::new(url::Url::parse("https://example.com").unwrap());
    let scraper_config = ScraperConfig {
        output_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let container = Container::new(crawler_config, scraper_config)
        .await
        .expect("create test container");
    (tmp, container)
}

#[test]
fn test_default_limits_are_reasonable() {
    let limits = CategoryLimits::default();
    assert!(
        limits.ai.get() >= 1,
        "AI limit must allow at least 1 concurrent"
    );
    assert!(
        limits.scraping.get() >= 1,
        "Scraping limit must allow at least 1"
    );
    assert!(
        limits.ai < limits.scraping,
        "AI should be more restricted than scraping"
    );
}

#[test]
fn test_semaphores_created_with_correct_permits() {
    let limits = CategoryLimits::default();
    let semaphores = CategorySemaphores::from_limits(&limits);
    assert_eq!(semaphores.ai.available_permits(), limits.ai.get());
    assert_eq!(
        semaphores.scraping.available_permits(),
        limits.scraping.get()
    );
    assert_eq!(
        semaphores.obsidian.available_permits(),
        limits.obsidian.get()
    );
}

#[tokio::test]
async fn test_new_state_has_no_inspector() {
    let (_tmp, container) = test_container().await;
    let state = McpState::new(container);
    assert!(state.inspector.is_none(), "inspector must default to None");
}

#[tokio::test]
async fn test_with_inspector_sets_inspector() {
    let (_tmp, container) = test_container().await;
    let inspector: Arc<dyn DomInspectorPort> = Arc::new(NoOpInspector);
    let state = McpState::new(container).with_inspector(inspector);
    assert!(
        state.inspector.is_some(),
        "inspector must be set after with_inspector"
    );
}

#[tokio::test]
async fn test_with_limits_has_no_inspector() {
    let (_tmp, container) = test_container().await;
    let state = McpState::with_limits(container, CategoryLimits::default());
    assert!(
        state.inspector.is_none(),
        "inspector must default to None in with_limits"
    );
}

/// REQ-06: the metrics accumulator is shared across per-session clones
/// (`server.rs` clones `McpState` per session). Mirrors the `Arc::ptr_eq`
/// pattern from `server.rs::test_mcp_state_with_downloader`.
#[tokio::test]
async fn metrics_shared_across_clones() {
    use crate::mcp_server::metrics::Outcome;
    use std::time::Duration;

    let (_tmp, container) = test_container().await;
    let state = McpState::new(container);
    let state2 = state.clone();

    assert!(
        Arc::ptr_eq(&state.metrics, &state2.metrics),
        "cloned McpState must share the same metrics Arc"
    );

    // A scrape recorded through one clone is visible from the other.
    state.record_scrape(ScrapeEvent::new(
        "scrape_url",
        "a.com".to_string(),
        Outcome::Success,
        2,
        Duration::from_millis(10),
    ));

    let snap = state2.metrics_snapshot();
    assert_eq!(snap.total_events, 1, "record-through-clone is visible");
    assert_eq!(snap.success_count, 1, "success bucket recorded");
}

/// R4-session-results-global: the result slot is per-session, not
/// process-shared. A clone forks the origin's current contents into its
/// own buffer (so a seeded template reaches the session handler), and
/// afterwards the two diverge: stores through one clone stay invisible
/// from the other — concurrent MCP sessions can neither clobber nor read
/// each other's crawl results.
#[tokio::test]
async fn session_results_isolated_across_clones() {
    use webfang_core::domain::ValidUrl;

    fn fixture(url: &str, title: &str) -> webfang_core::domain::ScrapedContent {
        webfang_core::domain::ScrapedContent {
            title: title.to_string(),
            content: "body".to_string(),
            url: ValidUrl::try_from_url(url::Url::parse(url).expect("valid"))
                .expect("fixture is a plain https URL"),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
            quality_hint: None,
        }
    }

    let (_tmp, container) = test_container().await;
    let state = McpState::new(container);
    state
        .session_results
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(fixture("https://example.com/origin", "Origin run"));

    // Fork: the clone inherits the seeded contents (this is how a
    // seeded test template reaches its session handler).
    let cloned = state.clone();
    assert!(
        !Arc::ptr_eq(&state.session_results, &cloned.session_results),
        "cloned McpState must not share the session-results Arc"
    );
    assert_eq!(
        cloned
            .session_results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len(),
        1,
        "a clone forks the origin's current results"
    );

    // Divergence: a later store through the clone stays invisible from
    // the origin — one session's run never leaks into another's export.
    cloned
        .session_results
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(fixture("https://example.com/clone", "Clone run"));
    assert_eq!(
        state
            .session_results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len(),
        1,
        "stores through the clone must not leak back into the origin"
    );
}
// --- validate_export_dir (#696) ---

#[tokio::test]
async fn export_dir_relative_always_allowed() {
    let (_tmp, container) = test_container().await;
    let state = McpState::new(container); // no roots configured
    assert!(
        state.validate_export_dir(Path::new("./output")).is_ok(),
        "relative paths must pass through with no roots configured"
    );
}

#[tokio::test]
async fn export_dir_absolute_rejected_without_roots() {
    let (_tmp, container) = test_container().await;
    // Fail-closed default. `host_abs` spells the candidate as a real
    // absolute path on THIS host (POSIX literals like `/etc` are
    // root-without-prefix forms on Windows, #1608).
    let state = McpState::new(container);
    let err = state
        .validate_export_dir(Path::new(&path_gate::host_abs("/etc")))
        .expect_err("absolute path must be rejected with no roots");
    let msg = err.to_string();
    assert!(
        msg.contains("export roots"),
        "error must name the missing export roots, got: {msg}"
    );
}

#[tokio::test]
async fn export_dir_absolute_allowed_under_root() {
    let (tmp, container) = test_container().await;
    let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
    let target = tmp.path().join("exports");
    assert!(
        state.validate_export_dir(&target).is_ok(),
        "path under a configured root must be allowed"
    );
}

#[tokio::test]
async fn export_dir_absolute_rejected_outside_root() {
    let (tmp, container) = test_container().await;
    // `host_abs`: host-appropriate absolute spelling (see #1608).
    let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
    let err = state
        .validate_export_dir(Path::new(&path_gate::host_abs("/etc")))
        .expect_err("path outside roots must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("outside allowed export roots"),
        "error must name the root violation, got: {msg}"
    );
}

#[tokio::test]
async fn export_dir_sibling_prefix_is_not_a_root_match() {
    // `/tmp/rootX` must NOT satisfy root `/tmp/root` — the prefix check
    // is component-based (`starts_with`), not string-based.
    let (tmp, container) = test_container().await;
    let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
    let mut sibling = tmp.path().to_path_buf();
    let mut name = sibling
        .file_name()
        .expect("temp dir has a name")
        .to_os_string();
    name.push("_evil");
    sibling.set_file_name(name);
    assert!(
        state.validate_export_dir(&sibling).is_err(),
        "string-prefix sibling dir must not match the root"
    );
}

#[tokio::test]
async fn export_dir_dotdot_cannot_escape_root() {
    // Lexical normalization resolves `..` before the prefix check, so
    // `<root>/../etc` is rejected even though it starts with the root.
    let (tmp, container) = test_container().await;
    let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
    let mut escape = tmp.path().to_path_buf();
    escape.push("..");
    escape.push("etc");
    assert!(
        state.validate_export_dir(&escape).is_err(),
        "`..` traversal out of the root must be rejected"
    );
}

// --- validate_checkpoint_dir (#1588) — same gate, same roots ---

#[tokio::test]
async fn checkpoint_dir_relative_always_allowed() {
    let (_tmp, container) = test_container().await;
    let state = McpState::new(container); // no roots configured
    assert!(
        state
            .validate_checkpoint_dir(Path::new("./checkpoints"))
            .is_ok(),
        "relative checkpoint dirs must pass through with no roots configured"
    );
}

#[tokio::test]
async fn checkpoint_dir_absolute_rejected_without_roots() {
    let (_tmp, container) = test_container().await;
    // Fail-closed default; `host_abs` = host-appropriate absolute
    // spelling (see #1608).
    let state = McpState::new(container);
    let err = state
        .validate_checkpoint_dir(Path::new(&path_gate::host_abs("/etc")))
        .expect_err("absolute checkpoint_dir must be rejected with no roots");
    let msg = err.to_string();
    assert!(
        msg.contains("checkpoint_dir") && msg.contains("requires server-configured export roots"),
        "error must name the field and the missing export roots, got: {msg}"
    );
}

#[tokio::test]
async fn checkpoint_dir_absolute_allowed_under_root() {
    let (tmp, container) = test_container().await;
    let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
    let target = tmp.path().join("checkpoints");
    assert!(
        state.validate_checkpoint_dir(&target).is_ok(),
        "checkpoint dir under a configured root must be allowed"
    );
}

#[tokio::test]
async fn checkpoint_dir_absolute_rejected_outside_root() {
    let (tmp, container) = test_container().await;
    // `host_abs`: host-appropriate absolute spelling (see #1608).
    let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
    let err = state
        .validate_checkpoint_dir(Path::new(&path_gate::host_abs("/etc")))
        .expect_err("checkpoint_dir outside roots must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("outside allowed export roots"),
        "error must name the root violation, got: {msg}"
    );
}
