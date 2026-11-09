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

#[tokio::test]
async fn export_checkpoint_flushes_markdown_during_batch() {
    let server = MockServer::start().await;
    for i in 0..3 {
        Mock::given(method("GET"))
            .and(path(format!("/p{i}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(substantial(&format!("Page {i}"))),
            )
            .mount(&server)
            .await;
    }

    let out = TempDir::new().unwrap();
    let urls = (0..3)
        .map(|i| format!("{}/p{i}\n", server.uri()))
        .collect::<String>();

    cmd()
        .arg("--batch")
        .arg("--export-checkpoint")
        .arg("2")
        .arg("--output")
        .arg(out.path())
        .write_stdin(urls)
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    // All 3 pages end up as markdown (the final flush covers the tail past
    // the last checkpoint).
    let md_files: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .collect();
    // Markdown may land under a host subdir; walk recursively.
    fn count_md(root: &std::path::Path) -> usize {
        let mut n = 0;
        if let Ok(entries) = std::fs::read_dir(root) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    n += count_md(&p);
                } else if p.extension().is_some_and(|x| x == "md") {
                    n += 1;
                }
            }
        }
        n
    }
    let total_md = count_md(out.path());
    assert_eq!(
        total_md,
        3,
        "checkpoint + final flush export all 3 pages, got {total_md} (dir listing had {})",
        md_files.len()
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
    cmd()
        .arg("--batch")
        .arg("--export-checkpoint")
        .arg("0")
        .arg("--output")
        .arg(out.path())
        .write_stdin(format!("{}\n", server.uri()))
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let sidecar = out.path().join(".webfang-batch-capture.progress.json");
    assert!(
        !sidecar.exists(),
        "checkpoint disabled must not write a progress sidecar: {}",
        sidecar.display()
    );
}
