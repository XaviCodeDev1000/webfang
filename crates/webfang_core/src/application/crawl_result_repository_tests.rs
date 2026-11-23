use super::*;
use std::io::Write;
use tempfile::TempDir;
use url::Url;

use crate::domain::value_objects::ValidUrl;

/// Helper: URL with trailing slash removed for consistent assertions.
/// Url::parse normalizes "https://a.com" to "https://a.com/"
fn make_content(url_str: &str, title: &str) -> ScrapedContent {
    let url = Url::parse(url_str).unwrap();
    ScrapedContent {
        url: ValidUrl::new(url),
        title: title.to_string(),
        content: format!("Content for {title}"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
        correlation_id: None,
        quality_hint: None,
    }
}

/// Poll until the background writer has processed a write for the given URL.
async fn wait_for_index(repo: &CrawlResultRepositoryImpl, url: &str) {
    for _ in 0..40 {
        if repo.find_by_url(url).unwrap().is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn test_save_and_find_by_url() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

    let content = make_content("https://example.com", "Example");
    repo.save(&content).unwrap();

    // Poll until the background writer has updated the index
    wait_for_index(&repo, "https://example.com/").await;

    let found = repo.find_by_url("https://example.com/").unwrap();
    assert!(found.is_some(), "expected to find saved content");
    let found = found.unwrap();
    assert_eq!(found.title, "Example");
    assert_eq!(found.content, "Content for Example");
    // Normalized URL from url::Url includes trailing slash
    assert_eq!(found.url.as_str(), "https://example.com/");
}

#[tokio::test]
async fn test_find_by_url_unknown_returns_none() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

    let found = repo.find_by_url("https://unknown.com").unwrap();
    assert!(found.is_none(), "expected None for unknown URL");
}

#[tokio::test]
async fn test_get_all_urls_returns_all_saved() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

    repo.save(&make_content("https://a.com", "A")).unwrap();
    repo.save(&make_content("https://b.com", "B")).unwrap();
    repo.save(&make_content("https://c.com", "C")).unwrap();

    // Wait for all three writes
    wait_for_index(&repo, "https://a.com/").await;
    wait_for_index(&repo, "https://b.com/").await;
    wait_for_index(&repo, "https://c.com/").await;

    let mut urls = repo.get_all_urls().unwrap();
    urls.sort();
    // url::Url normalizes bare domains with trailing slash
    assert_eq!(
        urls,
        vec!["https://a.com/", "https://b.com/", "https://c.com/"]
    );
}

#[tokio::test]
async fn test_load_all_returns_all_saved() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

    repo.save(&make_content("https://a.com", "A")).unwrap();
    repo.save(&make_content("https://b.com", "B")).unwrap();
    repo.save(&make_content("https://c.com", "C")).unwrap();

    // Wait for all three writes to be indexed
    wait_for_index(&repo, "https://a.com/").await;
    wait_for_index(&repo, "https://b.com/").await;
    wait_for_index(&repo, "https://c.com/").await;

    let all = repo.load_all().unwrap();
    assert_eq!(all.len(), 3, "load_all should return all 3 saved items");

    let mut titles: Vec<String> = all.iter().map(|c| c.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, vec!["A", "B", "C"]);

    // Content integrity: each item carries its expected body
    for item in &all {
        assert_eq!(item.content, format!("Content for {}", item.title));
    }
}

#[tokio::test]
async fn test_load_all_empty_repository_returns_empty_vec() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

    let all = repo.load_all().unwrap();
    assert!(all.is_empty(), "load_all on fresh repo should be empty");
}

#[tokio::test]
async fn test_recovery_rebuilds_index() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");

    // First session: save entries
    {
        let repo = CrawlResultRepositoryImpl::new(log_path.clone(), 64).unwrap();
        repo.save(&make_content("https://first.com", "First"))
            .unwrap();
        repo.save(&make_content("https://second.com", "Second"))
            .unwrap();
        wait_for_index(&repo, "https://first.com/").await;
        wait_for_index(&repo, "https://second.com/").await;
    } // repo drops — writer task stops

    // Second session: recover from existing log
    {
        let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();
        let mut urls = repo.get_all_urls().unwrap();
        urls.sort();
        assert_eq!(urls, vec!["https://first.com/", "https://second.com/"]);

        let found = repo.find_by_url("https://first.com/").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, "First");
    }
}

#[tokio::test]
async fn test_empty_repository_returns_empty_urls() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

    let urls = repo.get_all_urls().unwrap();
    assert!(urls.is_empty(), "expected empty URL list for fresh repo");
}

#[tokio::test]
async fn test_save_multiple_and_read_each() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();

    let contents = vec![
        make_content("https://alpha.com/page1", "Alpha One"),
        make_content("https://beta.com/page2", "Beta Two"),
        make_content("https://gamma.com/page3", "Gamma Three"),
    ];

    for c in &contents {
        repo.save(c).unwrap();
    }

    // Wait for all writes
    wait_for_index(&repo, "https://alpha.com/page1").await;
    wait_for_index(&repo, "https://beta.com/page2").await;
    wait_for_index(&repo, "https://gamma.com/page3").await;

    let found = repo
        .find_by_url("https://alpha.com/page1")
        .unwrap()
        .unwrap();
    assert_eq!(found.title, "Alpha One");
    assert_eq!(found.content, "Content for Alpha One");

    let found = repo
        .find_by_url("https://gamma.com/page3")
        .unwrap()
        .unwrap();
    assert_eq!(found.title, "Gamma Three");
    assert_eq!(found.content, "Content for Gamma Three");
}

#[tokio::test]
async fn test_crash_safe_partial_record() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");

    // Write a valid record followed by a partial (truncated) record
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();

        // Valid record
        let content = make_content("https://valid.com", "Valid");
        let payload = serde_json::to_vec(&content).unwrap();
        let len = (payload.len() as u32).to_le_bytes();
        file.write_all(&len).unwrap();
        file.write_all(&payload).unwrap();
        file.write_all(b"\n").unwrap();

        // Partial record: write only 2 bytes of a 4-byte length prefix
        file.write_all(&[0xFF, 0xFF]).unwrap();
        file.flush().unwrap();
    }

    // Recovery should succeed and only contain valid.com
    let repo = CrawlResultRepositoryImpl::new(log_path, 64).unwrap();
    let urls = repo.get_all_urls().unwrap();
    assert_eq!(urls, vec!["https://valid.com/"]);

    // And we can retrieve the valid record
    let found = repo.find_by_url("https://valid.com/").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().title, "Valid");
}

// ============================================================================
// #1121 — background writer: drain-on-shutdown, joined handle, flush truth.
// ============================================================================

/// `shutdown()` must drain every buffered record and join the writer:
/// 50 saves accepted into the bounded channel with NO polling in between
/// must all be on disk when shutdown returns. The pre-fix shape dropped
/// the `JoinHandle` (writer death invisible) and detached the task from
/// runtime exit, so buffered records could be lost silently.
#[tokio::test]
async fn test_shutdown_drains_buffered_writes() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 1024).unwrap();

    for i in 0..50 {
        repo.save(&make_content(
            &format!("https://drain-{i}.example.com"),
            &format!("Drain {i}"),
        ))
        .expect("save accepted into bounded channel");
    }

    repo.shutdown()
        .await
        .expect("clean shutdown drains and joins the writer");

    // Idempotent: second shutdown finds no handle and succeeds.
    repo.shutdown().await.expect("shutdown is idempotent");

    let all = repo.load_all().expect("load_all after shutdown");
    assert_eq!(
        all.len(),
        50,
        "every save acknowledged before shutdown must be persisted"
    );
}

/// After `shutdown()` the send side is closed: a late `save` must fail
/// explicitly instead of accepting data that will never be persisted.
#[tokio::test]
async fn test_save_after_shutdown_fails_explicitly() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("crawl_results.bin");
    let repo = CrawlResultRepositoryImpl::new(log_path, 8).unwrap();

    repo.shutdown().await.expect("clean shutdown");

    let err = repo
        .save(&make_content("https://late.example.com", "Late"))
        .expect_err("save after shutdown must fail");
    assert!(
        err.to_string().contains("writer caído"),
        "late save must name the dead writer, got: {err}"
    );
}

// ============================================================================
// Task 5.1 memory probe — repository index growth (BEFORE numbers).
// Reuses this module's make_content helper; no byte assertions by
// design (Q3 MEASURE FIRST).
// ============================================================================
/// Channel capacity `N + 1_024` exceeds the total load `N` because `save()`
/// is a non-blocking `try_send`: any capacity below `N` makes acceptance
/// depend on the worker's drain timing, the assumption that saturated the
/// 1024-slot channel under CI load (#1170).
#[tokio::test]
async fn memory_probe_repository_index_growth_20k_results() {
    const N: usize = 20_000;
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = CrawlResultRepositoryImpl::new(dir.path().join("probe-log.jsonl"), N + 1_024)
        .expect("repository builds");
    let before = crate::infrastructure::observability::memory_probe::rss_bytes();

    for i in 0..N {
        let content = make_content(
            &format!("https://probe.example.com/doc-{i}"),
            &format!("Probe document {i}"),
        );
        repo.save(&content).expect("save accepted");
    }
    // Deterministic rendezvous: shutdown() drains the queue in order and
    // joins the writer thread (#1121), so every record is guaranteed to be
    // in the index when it returns — no polling windows, no sleep yields.
    repo.shutdown()
        .await
        .expect("clean shutdown drains and joins the writer");
    assert_eq!(repo.index.len(), N, "every save must land in the index");

    let after = crate::infrastructure::observability::memory_probe::rss_bytes();
    crate::infrastructure::observability::memory_probe::append_report(
        "BEFORE - crawl_result_repository index",
        &format!(
            "entries={} rss_before={} rss_after={} delta={}",
            repo.index.len(),
            crate::infrastructure::observability::memory_probe::fmt_rss(before),
            crate::infrastructure::observability::memory_probe::fmt_rss(after),
            crate::infrastructure::observability::memory_probe::fmt_rss(
                after.and_then(|a| before.map(|b| a.saturating_sub(b)))
            ),
        ),
    );
}
