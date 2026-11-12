#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #2611 — `webfang recover` converts a stranded batch capture spool into the
//! `.md` files its clean close never wrote.
//!
//! The load-bearing assertion is the TREE comparison: a stranded spool built
//! from the SAME page a real batch run scraped must recover into the SAME
//! markdown tree (same relative paths, same bytes) — same extractor, same
//! frontmatter, same naming. Everything else pins the failure contract:
//! corrupt lines are counted (never fatal), a missing spool exits `74`,
//! `--strict` turns partial success into `69`, and an unrecoverable spool
//! exits `3`.
//!
//! The unit-level contracts (byte offsets, corrupt-line accounting,
//! watermark) live in `application/batch/spool_export.rs`; this file pins the
//! binary surface an operator actually runs.

#[path = "common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::cli_harness::{redact_nondeterministic, spawn_budgeted, std_cmd};
use tempfile::TempDir;
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Substantial body: the shared minimum-content guard (#706) rejects tiny
/// pages, so the fixture must carry real content for the batch reference run
/// to actually produce a `.md`.
const PAGE_BODY: &str =
    "contenido de prueba sustancial para superar el guardia de contenido mínimo del extractor. ";

fn page_html() -> String {
    format!(
        "<html><body><article><h1>Guía de prueba</h1><p>{}</p></article></body></html>",
        PAGE_BODY.repeat(12)
    )
}

/// Serialize one captured record as a spool line (what `BoundedFileSink` writes).
fn spool_line(url: &str) -> String {
    let page = webfang_core::application::crawler::content_sink::CapturedPage {
        url: url.to_string(),
        html: page_html(),
    };
    serde_json::to_string(&page).expect("serialize captured page")
}

/// Collect every `.md` file under `root`, keyed by path relative to `root`.
fn md_tree(root: &Path) -> BTreeMap<String, String> {
    let mut tree = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.expect("walk output tree");
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .expect("entry is under root")
            .to_string_lossy()
            .into_owned();
        let content = std::fs::read_to_string(entry.path()).expect("read md");
        tree.insert(relative, content);
    }
    tree
}

/// Same redaction wrapper the other test crates define locally (insta derives
/// the snapshot location from the module path, so it lives here).
fn assert_snapshot_redacted(name: &str, dir: &Path, value: impl Into<String>) {
    let redacted = redact_nondeterministic(dir, &value.into());
    insta::assert_snapshot!(name, redacted);
}

/// The stranded spool of a killed batch: records in the spool, zero `.md`.
fn seed_stranded_spool(dir: &TempDir, lines: &[String]) -> PathBuf {
    let spool = dir.path().join(".webfang-batch-capture.jsonl");
    std::fs::write(&spool, lines.join("\n") + "\n").expect("write stranded spool");
    spool
}

/// Run `webfang recover --spool <p> -o <dir> [extra args]` under the shared
/// spawn budget and return the raw output.
async fn run_recover(spool: &Path, output: &Path, extra: &[&str]) -> std::process::Output {
    let mut command = std_cmd();
    command
        .arg("recover")
        .arg("--spool")
        .arg(spool)
        .arg("-o")
        .arg(output)
        .args(extra);
    spawn_budgeted("run_recover", command, None).await
}

fn exit_code(output: &std::process::Output) -> i32 {
    output.status.code().expect("process terminated by signal")
}

/// The shared partial-corruption fixture: one valid page plus raw `extra`
/// lines in the stranded spool, recovered into `<scratch>/out`.
async fn recover_with_lines(
    extra: &[&str],
    flags: &[&str],
) -> (TempDir, std::process::Output, PathBuf) {
    let scratch = TempDir::new().expect("temp dir");
    let mut lines = vec![spool_line("https://example.test/a")];
    lines.extend(extra.iter().map(|line| (*line).to_string()));
    let spool = seed_stranded_spool(&scratch, &lines);
    let out = scratch.path().join("out");
    let output = run_recover(&spool, &out, flags).await;
    (scratch, output, out)
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The load-bearing test: a stranded spool with the SAME page a real batch
/// run scraped recovers into the SAME markdown tree — same relative paths,
/// same bytes (#2611's promise: same extractor, same frontmatter, same names).
#[tokio::test]
async fn recover_tree_matches_a_real_batch_export() {
    let server = MockServer::start().await;
    let html = page_html();
    Mock::given(method("GET"))
        .and(wm_path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html.clone()))
        .expect(1)
        .mount(&server)
        .await;

    let scratch = TempDir::new().expect("temp dir");

    // Reference: a REAL batch run over the same page, clean close.
    let batch_out = scratch.path().join("batch-out");
    let batch_file = scratch.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", server.uri())).expect("write batch file");
    let mut batch_cmd = std_cmd();
    batch_cmd
        .arg("--batch-file")
        .arg(&batch_file)
        .arg("-o")
        .arg(&batch_out);
    let batch = spawn_budgeted("recover_tree_matches_a_real_batch_export", batch_cmd, None).await;
    assert!(
        batch.status.success(),
        "reference batch must succeed, stderr: {}",
        stderr_of(&batch)
    );
    let reference = md_tree(&batch_out);
    assert!(
        !reference.is_empty(),
        "reference batch must have produced .md files"
    );

    // Stranded: the SAME captured page, hand-written into a spool, no close.
    let recover_out = scratch.path().join("recover-out");
    let spool = seed_stranded_spool(&scratch, &[spool_line(&server.uri())]);
    let output = run_recover(&spool, &recover_out, &[]).await;
    assert!(
        output.status.success(),
        "recover must succeed, stderr: {}",
        stderr_of(&output)
    );

    assert_eq!(
        md_tree(&recover_out),
        reference,
        "recovered tree must equal the batch export tree byte for byte"
    );
}

/// A corrupt record is a PAGE failure: counted, reported, never fatal (#2611).
#[tokio::test]
async fn recover_skips_corrupt_lines_and_exits_zero_without_strict() {
    let (_scratch, output, out) = recover_with_lines(&["{broken json"], &[]).await;

    assert_eq!(exit_code(&output), 0, "corrupt lines must not fail recover");
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("Convertidas: 1"),
        "one page recovered, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Corruptas: 1"),
        "the corrupt line must be counted, got:\n{stdout}"
    );
    let tree = md_tree(&out);
    assert_eq!(tree.len(), 1, "only the valid page was written");
}

/// A spool that cannot be opened exits `74` with the Spanish message.
#[tokio::test]
async fn recover_missing_spool_exits_74() {
    let scratch = TempDir::new().expect("temp dir");
    let missing = scratch.path().join("no-such-spool.jsonl");
    let out = scratch.path().join("out");

    let output = run_recover(&missing, &out, &[]).await;

    assert_eq!(exit_code(&output), 74, "missing spool is an I/O error");
    assert!(
        stderr_of(&output).contains("No se pudo abrir el spool"),
        "the error must name the failure in Spanish"
    );
}

/// `--strict` turns a partial recovery into exit `69`.
#[tokio::test]
async fn recover_strict_exits_69_on_partial_success() {
    let (_scratch, output, out) = recover_with_lines(&["{broken json"], &["--strict"]).await;

    assert_eq!(
        exit_code(&output),
        69,
        "--strict must fail a partial recovery"
    );
    let tree = md_tree(&out);
    assert_eq!(tree.len(), 1, "the valid page is still written before 69");
}

/// A spool where NOTHING is recoverable exits `3` (all scrapers failed).
#[tokio::test]
async fn recover_all_corrupt_exits_3() {
    let scratch = TempDir::new().expect("temp dir");
    let spool = seed_stranded_spool(
        &scratch,
        &["{broken".to_string(), "also broken".to_string()],
    );
    let out = scratch.path().join("out");

    let output = run_recover(&spool, &out, &[]).await;

    assert_eq!(
        exit_code(&output),
        3,
        "zero recovered pages from a non-empty spool is a scraper failure"
    );
}

/// The operator-facing summary is stable (paths redacted).
#[tokio::test]
async fn recover_summary_is_a_stable_snapshot() {
    let (scratch, output, _out) = recover_with_lines(&["{broken json"], &[]).await;
    let stdout = stdout_of(&output);

    assert_snapshot_redacted("recover_summary", scratch.path(), stdout);
}
