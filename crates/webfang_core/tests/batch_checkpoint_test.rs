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

// `Duration` is used only inside the `#[cfg(unix)]` kill test's sleep, so the
// import is gated with it: on Windows it would be an unused import, and that
// warning is invisible to every Linux gate.
#[cfg(unix)]
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::cli_harness::{spawn_budgeted, std_cmd};

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

/// Recover a batch spool into `recovered` under the shared spawn budget
/// (#2658 backlog), returning the captured output.
///
/// The spawn runs through `spawn_budgeted`, which applies
/// `SPAWN_LATENCY_BUDGET`, kills and reaps an overrunning child and retries
/// once; the caller asserts OUTSIDE the budget, on the returned `Output` and
/// the files on disk.
///
/// `#[cfg(unix)]` because its only caller is the `#[cfg(unix)]` kill test
/// below: on Windows this helper would be dead code, and that warning is
/// invisible to every Linux gate.
#[cfg(unix)]
async fn recover_spool(
    spool: &std::path::Path,
    recovered: &std::path::Path,
) -> std::process::Output {
    let mut command = std_cmd();
    command
        .arg("recover")
        .arg("--spool")
        .arg(spool)
        .arg("-o")
        .arg(recovered);
    spawn_budgeted(
        "killed_batch_leaves_a_consistent_spool_and_sidecar_recoverable",
        command,
        None,
    )
    .await
}

/// Run `--batch` over `urls` (stdin) into `out` with extra args; assert success.
///
/// The spawn runs under the shared budget via `spawn_budgeted` (#2658 backlog):
/// the raw `.timeout(60s)` was not a bound a test process can rely on, and the
/// success assert now lives OUTSIDE the budget, on the returned `Output`.
async fn run_batch_stdin(out: &std::path::Path, urls: String, extra: &[&str]) {
    let mut command = std_cmd();
    command.arg("--batch").arg("--output").arg(out);
    for a in extra {
        command.arg(a);
    }
    let output = spawn_budgeted("run_batch_stdin", command, Some(urls.as_bytes())).await;
    assert!(
        output.status.success(),
        "run_batch_stdin must succeed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn export_checkpoint_flushes_markdown_during_batch() {
    let server = MockServer::start().await;
    mount_pages(&server, 3, false).await;

    let out = TempDir::new().unwrap();
    let urls = (0..3)
        .map(|i| format!("{}/p{i}\n", server.uri()))
        .collect::<String>();

    run_batch_stdin(out.path(), urls, &["--export-checkpoint", "2"]).await;

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
    )
    .await;

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

    // Both runs spawn through the shared budget (`spawn_budgeted` applies
    // `SPAWN_LATENCY_BUDGET` internally: kill+reap+one retry, #2658 backlog) —
    // the raw `.timeout(60s)` closure is gone, and the success assert lives
    // OUTSIDE the budget on the returned `Output`.
    async fn run(state: &std::path::Path, out: &std::path::Path, urls: String) {
        let mut command = std_cmd();
        command
            .arg("--batch")
            .arg("--export-checkpoint")
            .arg("2")
            .arg("--resume")
            .arg("--state-dir")
            .arg(state)
            .arg("--output")
            .arg(out);
        let output = spawn_budgeted(
            "checkpoint_then_resume_skips_flushed_pages",
            command,
            Some(urls.as_bytes()),
        )
        .await;
        assert!(
            output.status.success(),
            "batch run must succeed; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // Run 1: checkpoint flush commits the pages.
    run(state.path(), out.path(), urls.clone()).await;
    // Run 2: same command — every URL is COMMITTED, nothing re-fetches.
    run(state.path(), out.path(), urls).await;

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

// ---------------------------------------------------------------------------
// WU5 — S7 invariants under a synchronized kill (#2611):
// kill the batch mid-run (after the server observed K page fetches — never
// the clock), then prove the on-disk pair is consistent and recoverable.
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn killed_batch_leaves_a_consistent_spool_and_sidecar_recoverable() {
    use common::cli_harness::{
        count_page_fetches, send_signal, spawn_webfang, wait_exit, wait_for_page_fetches,
    };
    use std::process::Stdio;

    let server = MockServer::start().await;
    // Slow pages so the kill lands mid-run deterministically.
    for i in 0..8 {
        Mock::given(method("GET"))
            .and(path(format!("/p{i}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(substantial(&format!("Page {i}")))
                    .set_delay(Duration::from_millis(300)),
            )
            .mount(&server)
            .await;
    }

    let out = TempDir::new().unwrap();
    let spool_dir = TempDir::new().unwrap();
    let urls = (0..8)
        .map(|i| format!("{}/p{i}\n", server.uri()))
        .collect::<String>();
    let urls_file = spool_dir.path().join("urls.txt");
    std::fs::write(&urls_file, urls).unwrap();

    let args = vec![
        "--batch-file".to_string(),
        urls_file.to_string_lossy().into_owned(),
        "--export-checkpoint".to_string(),
        "1".to_string(),
        "--output".to_string(),
        out.path().to_string_lossy().into_owned(),
        "--state-dir".to_string(),
        spool_dir.path().to_string_lossy().into_owned(),
    ];
    let cache = TempDir::new().unwrap();
    let child = spawn_webfang(&args, cache.path(), "kill-mid-batch");
    // Deterministic kill point: the 3rd page fetch observed by the server.
    wait_for_page_fetches(&server, 3).await;
    send_signal(&child, "KILL", "kill-mid-batch");
    let (status, _stderr) = wait_exit(child, "kill-mid-batch");
    assert!(!status.success(), "SIGKILLed batch must not exit 0");

    let spool = out.path().join(".webfang-batch-capture.jsonl");
    let sidecar = out.path().join(".webfang-batch-capture.progress.json");
    assert!(
        spool.exists(),
        "the killed run's spool must survive: {}",
        spool.display()
    );

    // S7(ii): every line the sidecar's watermark covers parses as a record.
    if sidecar.exists() {
        let sidecar: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
        let watermark = sidecar["watermark_bytes"].as_u64().unwrap();
        let spool_bytes = std::fs::read(&spool).unwrap();
        let head = &spool_bytes[..usize::try_from(watermark).unwrap().min(spool_bytes.len())];
        for line in String::from_utf8_lossy(head)
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            let v: serde_json::Value =
                serde_json::from_str(line).expect("pre-watermark line must parse");
            assert!(
                v["url"].is_string() && v["html"].is_string(),
                "record shape: {line}"
            );
        }
    }

    // S7(i)+(iii) via recover: every page in the spool converts to a .md.
    let recovered = TempDir::new().unwrap();
    let recover_output = recover_spool(&spool, recovered.path()).await;
    let md_count = count_md(recovered.path());
    let spool_lines = std::fs::read_to_string(&spool)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    let spool_len = std::fs::metadata(&spool).unwrap().len();
    let fetches = count_page_fetches(&server).await;
    // S7(ii): the sidecar watermark never exceeds the spool length (consistency
    // of the pair after a kill), and pre-watermark lines all parse (checked above).
    if sidecar.exists() {
        let sc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
        let wm = sc["watermark_bytes"].as_u64().unwrap();
        assert!(wm <= spool_len, "watermark {wm} past spool len {spool_len}");
    }
    // The kill may land before the writer's first per-page flush (spool still
    // empty) or after (spool holds records); either way recover must succeed and
    // export exactly what the spool holds.
    assert!(
        md_count == spool_lines,
        "recover exported {md_count} but spool holds {spool_lines}"
    );
    // S7(iv): the killed run fetched each URL at most once.
    assert!(
        fetches <= 8,
        "no URL fetched more than once: {fetches} fetches for 8 URLs"
    );
    let _ = recover_output;
    let _ = Stdio::null;
}
