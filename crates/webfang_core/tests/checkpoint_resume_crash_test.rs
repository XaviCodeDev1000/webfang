#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #3079 / E13 — a periodic checkpoint must not swallow pages that were never
//! exported.
//!
//! `CrawlScheduler::next_url` marks a URL in `CrawlCheckpoint.visited` when it
//! HANDS IT OUT, not when the page is exported. The executed probe (E13) showed
//! the cost: `--checkpoint-interval 2` + SIGKILL left `visited` with 11 URLs,
//! 6 pages exported, and `--resume` asked only for `/robots.txt` and `/`,
//! exported 1 of 11 lines and exited **0** — silent loss, reported as success.
//!
//! This file is the permanent binary-level probe under the current
//! unconditional crash-injection mechanism: the first run dies at
//! `after_periodic_checkpoint` (the periodic checkpoint is on disk and no
//! dispatch has happened since), the second run resumes over the same state
//! dir, and the union of both runs must export every page EXACTLY once.
//!
//! Determinism: `--concurrency 1` makes the loop advance one page per
//! iteration, so the third loop iteration is always the first one that finds
//! `pages_crawled == 2` (a multiple of `--checkpoint-interval 2`) and saves.
//! The crash point is armed at that occurrence; no sleep decides anything.
//!
//! Unix gate: the assertion is a death by `SIGKILL(9)`, and the crash harness
//! aborts (never signals) where `kill_self` has no unix implementation.
#![cfg(unix)]

#[path = "common/mod.rs"]
mod common;

use std::path::Path;

use common::cli_harness::{spawn_webfang, spawn_webfang_with, wait_exit, BehavioralTest};
use common::hub_site::{mount_hub_site, LEAVES};
use tempfile::TempDir;
use webfang_core::cli::crash_points::{AFTER_PERIODIC_CHECKPOINT, ENV_VAR};

/// Crawl arguments shared by both legs: checkpoint every 2 pages, one worker
/// (the deterministic loop cadence), one level deep.
fn crawl_args(state_dir: &Path, out: &Path, base: &str) -> Vec<String> {
    [
        "--resume",
        "--state-dir",
        &state_dir.display().to_string(),
        "--url",
        base,
        "--output",
        &out.display().to_string(),
        "--checkpoint-interval",
        "2",
        "--concurrency",
        "1",
        "--max-depth",
        "1",
        "--max-pages",
        "20",
        "--delay-ms",
        "0",
        "--ignore-robots",
        "--quiet",
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect()
}

/// The checkpoint files the run wrote under `dir` (F-01 scopes them per seed).
fn checkpoint_files(dir: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.file_name().is_some_and(|name| {
                        let name = name.to_string_lossy();
                        name.starts_with("crawl_checkpoint") && name.ends_with(".json")
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every URL exported to JSONL, in file order (the export shape is one record
/// per line, so a duplicate line is a duplicate export).
fn jsonl_urls(t: &BehavioralTest) -> Vec<String> {
    let mut urls = Vec::new();
    for file in t.find_files("jsonl") {
        let body = std::fs::read_to_string(&file).expect("read jsonl export");
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON line");
            let url = value["url"].as_str().expect("line carries the page URL");
            urls.push(url.to_string());
        }
    }
    urls
}

/// A periodic checkpoint written just before a crash must not turn the pages it
/// had only DISPATCHED into pages a resume skips.
#[tokio::test]
async fn periodic_checkpoint_crash_resume_exports_every_page_once() {
    let t = BehavioralTest::new().await;
    mount_hub_site(&t.server).await;
    let base = t.server.uri();
    let state_dir = TempDir::new().expect("state tempdir");
    let cache = TempDir::new().expect("hermetic cache tempdir");
    let args = crawl_args(state_dir.path(), t.out.path(), &base);

    // Leg 1: die on the first periodic checkpoint that is actually on disk,
    // which with one worker and interval 2 is the third loop iteration.
    let armed = format!("{AFTER_PERIODIC_CHECKPOINT}:3");
    let crashed = spawn_webfang_with(&args, cache.path(), "checkpoint crash run", |cmd| {
        cmd.env(ENV_VAR, &armed);
    });
    let (status, stderr) = wait_exit(crashed, "checkpoint crash run");
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        status.signal(),
        Some(9),
        "the armed crash point must kill the run by SIGKILL(9); status={status:?}; \
         child stderr: {stderr}"
    );
    assert!(!status.success(), "a crashed run is never a clean exit");

    // The crash point promised a checkpoint on disk: it must be there, carry
    // the dispatched URLs, and record the durable page count.
    let files = checkpoint_files(state_dir.path());
    assert!(
        !files.is_empty(),
        "the periodic checkpoint must exist after the crash; child stderr: {stderr}"
    );
    let raw = std::fs::read(&files[0]).expect("read checkpoint");
    let checkpoint: serde_json::Value =
        serde_json::from_slice(&raw[4.min(raw.len())..]).expect("checkpoint payload is JSON");
    let visited = checkpoint["visited"]
        .as_array()
        .expect("checkpoint carries visited");
    assert!(
        !visited.is_empty(),
        "a checkpoint written after two pages must carry visited URLs"
    );
    assert!(
        checkpoint["settled"].as_array().is_some(),
        "the payload must carry the terminal-verdict set (version 2, serde default)"
    );
    assert!(
        checkpoint["pages_crawled"].as_u64().unwrap_or(0) >= 2,
        "the periodic save fires at a multiple of the interval"
    );

    // Leg 2: resume over the same state dir; the crash env is gone with the
    // child, so nothing kills this run.
    let resumed = spawn_webfang(&args, cache.path(), "checkpoint resume run");
    let (status, stderr) = wait_exit(resumed, "checkpoint resume run");
    assert!(
        status.success(),
        "the resumed run must exit 0, got {status:?}; child stderr: {stderr}"
    );

    // Every page exactly once: the seed plus each leaf, no page lost by the
    // crash and none exported twice by the resume.
    let mut expected: Vec<String> = vec![format!("{base}/")];
    expected.extend((0..LEAVES).map(|i| format!("{base}/page-{i}")));
    expected.sort();

    let mut exported = jsonl_urls(&t);
    exported.sort();
    assert_eq!(
        exported, expected,
        "the resumed export must contain every page exactly once (#3079)"
    );

    let markdown = t.find_files("md");
    assert_eq!(
        markdown.len(),
        LEAVES + 1,
        "one markdown document per crawled page, got {markdown:?}"
    );
}
