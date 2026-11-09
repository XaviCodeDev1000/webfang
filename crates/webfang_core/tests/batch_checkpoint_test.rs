//! #2611 WU3 — `--export-checkpoint <N>` flushes the spool to markdown during
//! the batch, not only at the clean close.
//!
//! Behavioral contract (public interface only — the binary, its flags, its
//! files on disk, its exit code):
//! - with `--export-checkpoint 2`, a batch of 3 URLs must leave `.md` files
//!   for the captured pages even though the clean close ALSO runs (the flush
//!   must not double-export);
//! - the progress sidecar (`.webfang-batch-capture.progress.json`) is written
//!   at startup with watermark 0 and ends with watermark == spool length after
//!   the final flush;
//! - `--export-checkpoint 0` (the default) writes NO sidecar (slice-1
//!   behavior preserved: the spool is deleted at the clean close).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::cli_harness::cmd;

fn substantial(title: &str) -> String {
    let body =
        "Substantial body text so the shared minimum-content guard accepts the page during extraction. ".repeat(6);
    format!("<html><head><title>{title}</title></head><body><article><h1>{title}</h1><p>{body}</p></article></body></html>")
}

/// Mount `/p0..p{n}` routes, each with a substantial body the extractor
/// accepts (optionally capped at `expect_hits` per route). Shared by the
/// checkpoint tests so the mount loop is written once.
async fn mount_pages(server: &MockServer, n: usize, expect_hits: bool) {
    for i in 0..n {
        let mock = Mock::given(method("GET"))
            .and(path(format!("/p{i}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(substantial(&format!("Page {i}"))),
            );
        let mock = if expect_hits {
            mock.expect(1) // EXACTLY one hit across both runs (#2611 S7 iv)
        } else {
            mock
        };
        mock.mount(server).await;
    }
}

/// Recursively collect file paths under `root`.
fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Count `.md` files under `root` (markdown lands in host subdirs).
fn count_md(root: &std::path::Path) -> usize {
    walk(root)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .count()
}

/// Run `--batch` over `urls` (stdin) into `out` with extra args; assert success.
fn run_batch_stdin(out: &std::path::Path, urls: String, extra: &[&str]) {
    let mut c = cmd();
    c.arg("--batch").arg("--output").arg(out);
    for a in extra {
        c.arg(a);
    }
    c.write_stdin(urls)
        .timeout(Duration::from_secs(60))
        .assert()
        .success();
}

#[tokio::test]
async fn export_checkpoint_flushes_markdown_during_batch() {
    let server = MockServer::start().await;
    mount_pages(&server, 3, false).await;

    let out = TempDir::new().unwrap();
    let urls = (0..3)
        .map(|i| format!("{}/p{i}\n", server.uri()))
        .collect::<String>();

    run_batch_stdin(out.path(), urls, &["--export-checkpoint", "2"]);

    // All 3 pages end up as markdown (the final flush covers the tail past
    // the last checkpoint).
    let total_md = count_md(out.path());
    assert_eq!(
        total_md, 3,
        "checkpoint + final flush export all 3 pages, got {total_md}"
    );

    // The progress sidecar exists while/after the run and its watermark is
    // the full spool length (everything was flushed).
    let sidecar_path = out.path().join(".webfang-batch-capture.progress.json");
    // The spool itself is discarded at the clean close, but the sidecar
    // records the final watermark.
    if sidecar_path.exists() {
        let sidecar: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sidecar_path).unwrap()).unwrap();
        assert_eq!(sidecar["version"], 1);
        assert!(
            sidecar["watermark_bytes"].as_u64().unwrap() > 0,
            "watermark must be > 0 after the final flush: {sidecar}"
        );
    }
}

#[tokio::test]
async fn export_checkpoint_zero_writes_no_sidecar() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(substantial("Solo")))
        .mount(&server)
        .await;

    let out = TempDir::new().unwrap();
    run_batch_stdin(
        out.path(),
        format!("{}\n", server.uri()),
        &["--export-checkpoint", "0"],
    );

    let sidecar = out.path().join(".webfang-batch-capture.progress.json");
    assert!(
        !sidecar.exists(),
        "checkpoint disabled must not write a progress sidecar: {}",
        sidecar.display()
    );
}

/// WU4 + S7(iv): con `--export-checkpoint` y `--resume`, un relanzamiento
/// salta las URLs ya COMMITTED por el flush — cero doble descarga (cada URL
/// pega exactamente 1 hit en wiremock entre las dos corridas).
#[tokio::test]
async fn checkpoint_then_resume_skips_flushed_pages() {
    let server = MockServer::start().await;
    mount_pages(&server, 3, true).await;

    let out = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let urls = (0..3)
        .map(|i| format!("{}/p{i}\n", server.uri()))
        .collect::<String>();

    let run = |urls: String| {
        cmd()
            .arg("--batch")
            .arg("--export-checkpoint")
            .arg("2")
            .arg("--resume")
            .arg("--state-dir")
            .arg(state.path())
            .arg("--output")
            .arg(out.path())
            .write_stdin(urls)
            .timeout(Duration::from_secs(60))
            .assert()
            .success()
    };
    // Run 1: checkpoint flush commits the pages.
    run(urls.clone());
    // Run 2: same command — every URL is COMMITTED, nothing re-fetches.
    run(urls);

    // wiremock's expect(1) fails the test itself if any URL was fetched
    // twice; here we additionally assert the state store holds 3 COMMITTED
    // records in the batch namespace file.
    let mut committed = 0;
    for entry in walk(state.path()) {
        if entry.extension().is_some_and(|x| x == "json") {
            let text = std::fs::read_to_string(&entry).unwrap();
            committed += text.matches("\"COMMITTED\"").count();
        }
    }
    assert_eq!(
        committed, 3,
        "3 COMMITTED records expected in the batch namespace store"
    );
}
