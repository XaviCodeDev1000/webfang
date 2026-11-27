#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![cfg(unix)]

//! #3097 / T4 (findings E5/E6, decision D2) — a resumed crawl must say what it
//! resumed from and what it decided to skip, and a fresh run must say it is
//! fresh. Both are asserted from the OUTSIDE, through the real binary, so the
//! message cannot drift away from the run it describes.
//!
//! Four behaviours, one per test:
//!
//! 1. **Fresh.** A run without `--resume` states that no previous state was
//!    consulted, and says nothing about resuming or about a breakdown.
//! 2. **Clean resume.** A second run over the same state dir and the same
//!    output reports the state file it read, the breakdown
//!    (`7 discovered, 7 already committed, 0 pending`) and the staleness note
//!    (discovery re-runs and does not detect content changes). No checkpoint
//!    line: a completed run deletes its checkpoint
//!    (`Session::finish` → `CheckpointAction::Delete`), so there is nothing to
//!    report. Zero re-exports: the JSONL export still holds its 7 lines.
//! 3. **Crash resume.** A run killed at `after_periodic_checkpoint` leaves a
//!    checkpoint behind, so the resumed run reports the checkpoint decision the
//!    engine actually took, and at least one URL is re-driven (#3079's
//!    terminal-verdict rule: only `visited ∩ settled` may be skipped).
//! 4. **Output mismatch.** Resuming into a DIFFERENT `--output` than the one
//!    the records were committed to is a usage error (exit 64) naming both
//!    directories, not a successful run that writes nothing.
//!
//! Determinism: `--concurrency 1` makes the crawl loop advance one page per
//! iteration, so with `--checkpoint-interval 2` the third loop iteration is
//! always the first one that saves (`checkpoint_resume_crash_test` owns the
//! full cadence argument). Unix gate: case 3 asserts a death by `SIGKILL(9)`,
//! which has no Windows implementation in the crash harness.

#[path = "common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::cli_harness::{
    assert_snapshot_redacted, assert_spawn_within_output_blocking, spawn_webfang,
    spawn_webfang_with, std_scraper_cmd, wait_exit, BehavioralTest, SPAWN_LATENCY_BUDGET,
};
use tempfile::TempDir;
use webfang_core::cli::crash_points::{AFTER_PERIODIC_CHECKPOINT, ENV_VAR};
use webfang_core::cli::error::EXIT_USAGE_ERROR;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Leaf pages the seed links to; the seed itself makes `LEAVES + 1` pages.
const LEAVES: usize = 6;

/// Every page of the fixture, as the export writes its `url`.
const PAGES: usize = LEAVES + 1;

/// The literal contract lines this run must be able to produce.
const FRESH_LINE: &str = "webfang: fresh run - no previous state consulted";
const RESUMED_PREFIX: &str = "webfang: resumed from ";
const BREAKDOWN_PREFIX: &str = "webfang: breakdown: ";
const CHECKPOINT_PREFIX: &str = "webfang: checkpoint: ";
const STALENESS_LINE: &str =
    "webfang: discovery runs again on every resume and does not detect content changes";

/// Per-page body with enough text to clear the minimum-content guard and a
/// distinct title so every exported line is unique.
fn article(title: &str) -> String {
    format!(
        "<html><body><article><h1>{title}</h1>\
         <p>{title} carries enough substantive text to clear the minimum content guard comfortably.</p>\
         </article></body></html>"
    )
}

/// Mount `/` (hub linking every leaf), the leaves, and robots.txt.
async fn mount_hub_site(server: &MockServer) {
    let links: String = (0..LEAVES)
        .map(|i| format!(r#"<a href="/page-{i}">Page {i}</a>"#))
        .collect();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<html><body><article><h1>Hub</h1>\
             <p>The hub page carries enough substantive text to clear the minimum content guard.</p>\
             {links}</article></body></html>"
        )))
        .mount(server)
        .await;
    for i in 0..LEAVES {
        Mock::given(method("GET"))
            .and(path(format!("/page-{i}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(article(&format!("Page {i}"))))
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(server)
        .await;
}

/// The one temp root every path of a test lives under, so the snapshots carry
/// a single redacted prefix instead of three separate machine paths.
struct Scratch {
    root: TempDir,
    state: PathBuf,
    out1: PathBuf,
    out2: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let root = TempDir::new().expect("scratch tempdir");
        let state = root.path().join("state");
        let out1 = root.path().join("out1");
        let out2 = root.path().join("out2");
        // Leg 1 exports into out1, so it must exist; out2 deliberately does NOT
        // (case 4 proves nothing is written there).
        std::fs::create_dir_all(&out1).expect("create out1");
        Self {
            root,
            state,
            out1,
            out2,
        }
    }

    /// The prefix every snapshot redacts its paths against.
    fn base(&self) -> &Path {
        self.root.path()
    }
}

/// Run one scraping leg against a fixed mock server, under the harness's spawn
/// budget. `extra` carries the flags that differ between legs.
///
/// The closure moves onto the blocking pool, so it carries the URL, the paths
/// and the flags BY VALUE instead of borrowing the harness.
async fn run_leg(
    url: String,
    out: PathBuf,
    state: PathBuf,
    extra: Vec<String>,
    what: &'static str,
) -> Output {
    assert_spawn_within_output_blocking(SPAWN_LATENCY_BUDGET, what, move |child| {
        let mut cmd = std_scraper_cmd(&url, &out);
        cmd.arg("--resume")
            .arg("--state-dir")
            .arg(&state)
            // One worker: the page order the snapshot pins must be the crawl
            // order, not whichever fetch finished first on this machine.
            .arg("--concurrency")
            .arg("1")
            .arg("--max-depth")
            .arg("1")
            .arg("--max-pages")
            .arg("20")
            .arg("--delay-ms")
            .arg("0")
            .arg("--ignore-robots");
        for arg in &extra {
            cmd.arg(arg);
        }
        child.output(cmd)
    })
    .await
}

/// Run one scraping leg with persistence disabled: no `--resume`, no
/// `--state-dir`.
async fn run_fresh(url: String, out: PathBuf, what: &'static str) -> Output {
    assert_spawn_within_output_blocking(SPAWN_LATENCY_BUDGET, what, move |child| {
        let mut cmd = std_scraper_cmd(&url, &out);
        cmd.arg("--concurrency")
            .arg("1")
            .arg("--max-depth")
            .arg("1")
            .arg("--max-pages")
            .arg("20")
            .arg("--delay-ms")
            .arg("0")
            .arg("--ignore-robots");
        child.output(cmd)
    })
    .await
}

/// The stderr of a successful run, or a panic naming the real exit status.
fn successful_stderr(output: &Output, what: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "{what} must exit 0, got {:?}; stderr:\n{stderr}",
        output.status
    );
    stderr
}

/// Every non-empty line of every JSONL export under `dir` (one record per
/// line: a duplicate line is a duplicate export).
fn jsonl_records(dir: &Path) -> Vec<String> {
    let mut records = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() || entry.path().extension().is_some_and(|x| x != "jsonl") {
            continue;
        }
        let body = std::fs::read_to_string(entry.path()).expect("read jsonl export");
        records.extend(
            body.lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string),
        );
    }
    records
}

/// Split the numbers out of a `webfang: checkpoint: S settled, R re-driving,
/// Q queued` line: `(S, R, Q)`.
fn checkpoint_counts(stderr: &str) -> Option<(usize, usize, usize)> {
    let line = stderr
        .lines()
        .find(|line| line.trim_start().starts_with(CHECKPOINT_PREFIX))?;
    let numbers: Vec<usize> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| chunk.parse().expect("a run of digits parses as usize"))
        .collect();
    match numbers.as_slice() {
        [settled, re_driving, queued, ..] => Some((*settled, *re_driving, *queued)),
        _ => None,
    }
}

/// A run with persistence disabled must announce itself as fresh: no resume
/// line, no breakdown, no checkpoint line.
#[tokio::test]
async fn fresh_run_states_it_is_fresh() {
    let t = BehavioralTest::new().await;
    mount_hub_site(&t.server).await;
    let scratch = Scratch::new();

    let output = run_fresh(
        t.server.uri(),
        scratch.out1.clone(),
        "fresh run states it is fresh",
    )
    .await;
    let stderr = successful_stderr(&output, "fresh run");

    assert!(
        stderr.contains(FRESH_LINE),
        "a fresh run must say so verbatim; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(RESUMED_PREFIX),
        "a fresh run must not claim it resumed; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(BREAKDOWN_PREFIX),
        "a fresh run has no resume breakdown to report; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(CHECKPOINT_PREFIX),
        "a fresh run consulted no checkpoint; stderr:\n{stderr}"
    );

    assert_snapshot_redacted("fresh_run_stderr", scratch.base(), stderr);
}

/// A clean resume reports the state file, the breakdown and the staleness
/// note, and re-exports nothing.
#[tokio::test]
async fn clean_resume_states_what_it_resumed_from_and_the_breakdown() {
    let t = BehavioralTest::new().await;
    mount_hub_site(&t.server).await;
    let scratch = Scratch::new();

    // Leg 1: a complete crawl of the whole fixture into out1.
    let first = run_leg(
        t.server.uri(),
        scratch.out1.clone(),
        scratch.state.clone(),
        Vec::new(),
        "clean resume leg 1",
    )
    .await;
    successful_stderr(&first, "clean resume leg 1");
    assert_eq!(
        jsonl_records(&scratch.out1).len(),
        PAGES,
        "leg 1 must export every page of the fixture"
    );

    // Leg 2: identical flags over the same state dir and the same output.
    let second = run_leg(
        t.server.uri(),
        scratch.out1.clone(),
        scratch.state.clone(),
        Vec::new(),
        "clean resume leg 2",
    )
    .await;
    let stderr = successful_stderr(&second, "clean resume leg 2");

    assert!(
        stderr.contains(RESUMED_PREFIX),
        "a resumed run must name what it resumed from; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{BREAKDOWN_PREFIX}{PAGES} discovered, {PAGES} already committed, 0 pending"
        )),
        "the breakdown must report every URL as already committed; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(STALENESS_LINE),
        "a resumed run must warn that discovery re-ran without detecting content changes; \
         stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(CHECKPOINT_PREFIX),
        "a completed run deletes its checkpoint, so nothing may be reported for it; \
         stderr:\n{stderr}"
    );
    assert_eq!(
        jsonl_records(&scratch.out1).len(),
        PAGES,
        "a clean resume must not re-export a committed page"
    );

    assert_snapshot_redacted("clean_resume_stderr", scratch.base(), stderr);
}

/// A crash resume reports the checkpoint decision the engine took, naming a
/// non-zero re-driving count.
#[tokio::test]
async fn crash_resume_reports_the_checkpoint_settled_counts() {
    let t = BehavioralTest::new().await;
    mount_hub_site(&t.server).await;
    let scratch = Scratch::new();
    let cache = TempDir::new().expect("hermetic cache tempdir");
    let args: Vec<String> = [
        "--resume".to_string(),
        "--state-dir".to_string(),
        scratch.state.display().to_string(),
        "--url".to_string(),
        t.server.uri(),
        "--output".to_string(),
        scratch.out1.display().to_string(),
        "--checkpoint-interval".to_string(),
        "2".to_string(),
        "--concurrency".to_string(),
        "1".to_string(),
        "--max-depth".to_string(),
        "1".to_string(),
        "--max-pages".to_string(),
        "20".to_string(),
        "--delay-ms".to_string(),
        "0".to_string(),
        "--ignore-robots".to_string(),
    ]
    .to_vec();

    // Leg 1: die on the first periodic checkpoint that is actually on disk
    // (with one worker and interval 2 that is the third loop iteration).
    let armed = format!("{AFTER_PERIODIC_CHECKPOINT}:3");
    let crashed = spawn_webfang_with(&args, cache.path(), "crash resume leg 1", |cmd| {
        cmd.env(ENV_VAR, &armed);
    });
    let (status, crash_stderr) = wait_exit(crashed, "crash resume leg 1");
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        status.signal(),
        Some(9),
        "the armed crash point must kill the run by SIGKILL(9); status={status:?}; \
         child stderr: {crash_stderr}"
    );

    // Leg 2: resume over the checkpoint the crash left behind. The crash env
    // died with the child, so nothing kills this run.
    let resumed = spawn_webfang(&args, cache.path(), "crash resume leg 2");
    let (status, stderr) = wait_exit(resumed, "crash resume leg 2");
    assert!(
        status.success(),
        "the resumed run must exit 0, got {status:?}; child stderr: {stderr}"
    );

    let counts = checkpoint_counts(&stderr).unwrap_or_else(|| {
        panic!("the resumed run must report the checkpoint decision it took; stderr:\n{stderr}")
    });
    let (settled, re_driving, queued) = counts;
    assert!(
        re_driving > 0,
        "a crash resume must re-drive the URLs it only dispatched (settled={settled}, \
         re_driving={re_driving}, queued={queued}); stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(RESUMED_PREFIX),
        "the resumed run must still name its state file; stderr:\n{stderr}"
    );

    assert_snapshot_redacted("crash_resume_stderr", scratch.base(), stderr);
}

/// Resuming into a different `--output` is a usage error (64) naming both
/// directories, never a successful run that silently writes nothing.
#[tokio::test]
async fn resume_with_a_different_output_exits_64() {
    let t = BehavioralTest::new().await;
    mount_hub_site(&t.server).await;
    let scratch = Scratch::new();

    let first = run_leg(
        t.server.uri(),
        scratch.out1.clone(),
        scratch.state.clone(),
        Vec::new(),
        "output mismatch leg 1",
    )
    .await;
    successful_stderr(&first, "output mismatch leg 1");
    assert_eq!(
        jsonl_records(&scratch.out1).len(),
        PAGES,
        "leg 1 must export every page before the mismatch is exercised"
    );

    let second = run_leg(
        t.server.uri(),
        scratch.out2.clone(),
        scratch.state.clone(),
        Vec::new(),
        "output mismatch leg 2",
    )
    .await;
    let stderr = String::from_utf8_lossy(&second.stderr).into_owned();

    assert_eq!(
        second.status.code(),
        Some(i32::from(EXIT_USAGE_ERROR)),
        "resuming into a different --output must exit 64; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&scratch.out1.display().to_string()),
        "the error must name the recorded output directory; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&scratch.out2.display().to_string()),
        "the error must name the directory the run was pointed at; stderr:\n{stderr}"
    );
    assert!(
        jsonl_records(&scratch.out2).is_empty(),
        "a refused resume must write nothing into the new output directory"
    );

    assert_snapshot_redacted("output_mismatch_stderr", scratch.base(), stderr);
}
