//! Batch mode: stdin and file-based URL processing.

use crate::BehavioralTest;
use crate::{
    assert_spawn_within, assert_spawn_within_blocking, spawn_budgeted, std_cmd,
    SPAWN_LATENCY_BUDGET,
};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Run the binary under the shared spawn budget and assert success, with the
/// captured stderr in the failure message. Every spawn in this file asserts
/// success, so one wrapper keeps the call sites free of per-site closure
/// boilerplate (the jscpd duplication ratchet counts that boilerplate).
async fn spawn_ok(
    what: &'static str,
    cmd: std::process::Command,
    stdin: Option<&[u8]>,
) -> std::process::Output {
    let output = spawn_budgeted(what, cmd, stdin).await;
    assert!(
        output.status.success(),
        "{what} must succeed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Spawn `--batch` over the two cross-linked `mount_article_graph` seeds
/// under the shared budget, with extra CLI args, asserting success. One
/// builder for the tests that run the same batch command — otherwise their
/// arg chains clone (jscpd ratchet, #2658 backlog).
async fn spawn_batch_fixture(what: &'static str, t: &BehavioralTest, extra_args: &[&str]) {
    let uri = t.server.uri();
    let stdin_urls = format!("{uri}/article\n{uri}/article/2\n");
    let mut command = std_cmd();
    command.arg("--batch").arg("--output").arg(t.out.path());
    for arg in extra_args {
        command.arg(arg);
    }
    command.arg("--no-checkpoint").arg("--delay-ms").arg("0");
    spawn_ok(what, command, Some(stdin_urls.as_bytes())).await;
}

// ---------------------------------------------------------------------------
// #1215 (F-38 + F-02) fixtures: two articles that cross-link each other and a
// shared /other page. A BFS-per-seed batch run fetches /other (twice) and
// appends one export.jsonl line per fetched page, so 2 seeds yield 6 lines
// over 3 unique URLs. A scrape-per-URL run fetches exactly the 2 seeds.
// ---------------------------------------------------------------------------

/// Mount the #1215 cross-linked article graph on `server`.
async fn mount_article_graph(server: &MockServer) {
    let uri = server.uri();
    let article = format!(
        "<html><head><title>Article</title></head><body><main><article>\
         <h1>Article One</h1>\
         <p>This is the first article with enough substantive text for the extractor.</p>\
         <p><a href=\"{uri}/article/2\">Second article</a> \
         <a href=\"{uri}/other\">Other page</a></p>\
         </article></main></body></html>"
    );
    let article2 = format!(
        "<html><head><title>Article Two</title></head><body><main><article>\
         <h1>Article Two</h1>\
         <p>This is the second article with enough substantive text for the extractor.</p>\
         <p><a href=\"{uri}/article\">First article</a> \
         <a href=\"{uri}/other\">Other page</a></p>\
         </article></main></body></html>"
    );
    for (route, body) in [
        ("/article", article),
        ("/article/2", article2),
        (
            "/other",
            "<html><head><title>Other</title></head><body><main><article>\
             <h1>Other Page</h1>\
             <p>An unrelated page reachable from both articles, causing BFS overlap.</p>\
             </article></main></body></html>"
                .to_string(),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }
}

/// Read export.jsonl as parsed JSON values (one per non-empty line).
fn read_jsonl_lines(out: &std::path::Path) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(out.join("export.jsonl"))
        .expect("export.jsonl must exist after a successful --batch run");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every export.jsonl line must be valid JSON"))
        .collect()
}

/// Map every .md file under `out` by its path relative to `out`.
fn md_files_by_relative_path(out: &std::path::Path) -> HashMap<String, Vec<u8>> {
    walkdir::WalkDir::new(out)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| {
            let rel = e
                .path()
                .strip_prefix(out)
                .expect("walked path must be under out")
                .to_string_lossy()
                .into_owned();
            let bytes = std::fs::read(e.path()).expect("read .md file");
            (rel, bytes)
        })
        .collect()
}

/// The identity export.jsonl must be idempotent on: same URL + same content
/// hash is one record, no matter how many times it was fetched.
fn record_identity(v: &serde_json::Value) -> (String, String) {
    (
        v["url"].as_str().unwrap_or_default().to_string(),
        v["checksum_sha256"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

// ---------------------------------------------------------------------------
// --batch (stdin)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_stdin_processes_urls() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch Stdin Test</h1>\
                 <p>Content from batch stdin processing.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let stdin_url = format!("{}\n", server.uri());
    let mut command = std_cmd();
    command.arg("--batch");
    spawn_ok(
        "batch_stdin_processes_urls",
        command,
        Some(stdin_url.as_bytes()),
    )
    .await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "batch stdin should fetch exactly the provided URL, got {} requests",
        requests.len()
    );
}

/// #1697: an empty `--batch` on stdin must exit 64 with the Spanish message.
///
/// Runs under [`assert_spawn_within`] for the same reason as its
/// `batch_empty_file_exits_64` sibling. #1697 shipped the budget for that
/// test and this is the one it left behind: both do nothing but spawn the
/// binary, feed it an empty batch and assert the exit code, so they carry the
/// identical `windows-latest` spawn-latency flake. The flake is a property of
/// SPAWNING on that platform, not of which input path feeds the process —
/// which is why fixing the file-input half did not fix the stdin half.
///
/// Observed on `Tests (windows-latest)` for PR #1793 (run 37126785645): TRY 1
/// and TRY 2 each ran past nextest's 180 s `terminate-after` as TMT, and TRY 3
/// failed. The budget below bounds that at ~2x60 s instead (#2331 raised it
/// from 20 s after both attempts overran under load on run 40407265924), and
/// the assertion — code 64 AND the message — is unchanged, so a genuine
/// regression still fails and reports which half it got wrong.
#[test]
fn batch_empty_stdin_exits_64() {
    assert_spawn_within(
        SPAWN_LATENCY_BUDGET,
        "batch_empty_stdin_exits_64",
        move |child| {
            // #2387: the attempt runs through `BudgetChild::output` so the
            // harness can kill an overrunning spawn. stdin is `null` — an
            // immediately-readable EOF, the exact input `.write_stdin("")`
            // used to pipe — and the assert_cmd `.timeout(5s)` second net is
            // gone: the budget itself now bounds AND terminates the attempt.
            let mut command = std_cmd();
            command.arg("--batch");
            let output = child.output(command)?;

            if output.status.code() != Some(64) {
                return Err(format!(
                    "expected exit code 64, got {:?}; stderr: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            // #2795: the no-URLs error counts what the source yielded (an
            // empty file read 0 lines) instead of a bare "no URLs".
            if !stderr.contains("No hay URLs para procesar") {
                return Err(format!(
                    "stderr must name the empty batch, got: {}",
                    stderr.trim()
                ));
            }
            Ok(())
        },
    );
}

/// FIX-0 (#1235): record cardinality against the REAL binary. AUDIT-01
/// F-02 made `--batch` write every export.jsonl record TWICE (2 URLs → 4
/// lines, corrupting downstream RAG); #1225 fixed it but no test counted.
/// This pins the invariant with a NAMED test: N stdin URLs produce
/// exactly N export.jsonl records — one per unique URL, no duplicates.
#[tokio::test]
async fn batch_stdin_record_cardinality_matches_url_count() {
    let t = BehavioralTest::new().await;
    for (route, title) in [("/a", "Alpha"), ("/b", "Beta")] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><article><h1>{title}</h1>\
                     <p>Substantive body text for {title} so the extractor keeps the page.</p>\
                     </article></body></html>"
            )))
            .mount(&t.server)
            .await;
    }
    let base = t.server.uri();
    let stdin_urls = format!("{base}/a\n{base}/b\n");
    let mut command = std_cmd();
    command.arg("--batch").arg("--output").arg(t.out.path());
    spawn_ok(
        "batch_stdin_record_cardinality_matches_url_count",
        command,
        Some(stdin_urls.as_bytes()),
    )
    .await;

    assert_eq!(
        t.record_count("export.jsonl"),
        2,
        "2 stdin URLs must produce exactly 2 export.jsonl records (F-02 duplication regression)"
    );
}

// ---------------------------------------------------------------------------
// --batch file output (#631): the full pipeline must write .md + .jsonl, not
// skip export with an early return.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_stdin_writes_markdown_and_jsonl() {
    let server = MockServer::start().await;
    let out = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch File Output</h1>\
                 <p>Body text that must be written to disk.</p>\
             </article></body></html>",
        ))
        .mount(&server)
        .await;

    let stdin_url = format!("{}\n", server.uri());
    let mut command = std_cmd();
    command.arg("--batch").arg("--output").arg(out.path());
    spawn_ok(
        "batch_stdin_writes_markdown_and_jsonl",
        command,
        Some(stdin_url.as_bytes()),
    )
    .await;

    // `save_results` nests files under a per-host directory (e.g.
    // `<output>/127.0.0.1/index.md`), so walk the tree rather than the top
    // level only.
    let md_files: Vec<_> = walk_dir(out.path())
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    let jsonl_path = out.path().join("export.jsonl");

    assert!(
        !md_files.is_empty(),
        "--batch must write at least one .md file under --output (#631), dir: {:?}",
        out.path()
    );
    assert!(
        jsonl_path.exists(),
        "--batch must write export.jsonl to --output (#631), dir: {:?}",
        out.path()
    );
}

/// Recursively collect files under `root` (depth-first, errors skipped).
fn walk_dir(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk_dir(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// --batch --resume (#637): the run must create a resume state file.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_stdin_resume_creates_state_file() {
    let server = MockServer::start().await;
    let out = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Resume State</h1><p>This body carries enough substantive text to pass the minimum-content guard.</p></article></body></html>",
        ))
        .mount(&server)
        .await;

    let stdin_url = format!("{}\n", server.uri());
    let mut command = std_cmd();
    command
        .arg("--batch")
        .arg("--resume")
        .arg("--output")
        .arg(out.path())
        .env("XDG_CACHE_HOME", cache.path());
    spawn_ok(
        "batch_stdin_resume_creates_state_file",
        command,
        Some(stdin_url.as_bytes()),
    )
    .await;

    let state_files: Vec<_> = walk_dir(&cache.path().join("webfang/state"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert!(
        !state_files.is_empty(),
        "--batch --resume must create a state file (#637), cache: {:?}",
        cache.path()
    );
}

// ---------------------------------------------------------------------------
// --batch-file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_file_processes_urls() {
    let server = MockServer::start().await;
    let temp = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch File Test</h1>\
                 <p>Content from batch file processing.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", server.uri())).unwrap();

    let mut command = std_cmd();
    command.arg("--batch-file").arg(&batch_file);
    spawn_ok("batch_file_processes_urls", command, None).await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "batch-file should fetch exactly the URL from the file, got {} requests",
        requests.len()
    );
}

/// #1697: an empty `--batch-file` must exit 64 with the Spanish message.
///
/// Runs under [`assert_spawn_within`] because this test carries the
/// `windows-latest` spawn-latency flake: its `.timeout(5s)` was not honoured
/// there and the first attempt burned nextest's whole 180 s `terminate-after`
/// before the retry passed in 3 s. The budget below bounds that at ~2x60 s
/// instead (#2331 raised it from 20 s after both attempts overran under load
/// on run 40407265924), and the assertion — code 64 AND the message — is
/// unchanged, so a genuine regression still fails and reports which half it
/// got wrong.
#[test]
fn batch_empty_file_exits_64() {
    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, "").unwrap();

    // The shared counting-error shell (#2387 machinery inside); #2795 made
    // the error count what the source yielded (an empty file read 0 lines).
    assert_batch_file_exit(
        "batch_empty_file_exits_64",
        batch_file,
        &[],
        temp.path(),
        64,
        &["No hay URLs para procesar"],
    );
}

// ---------------------------------------------------------------------------
// Batch timeout tests
// ---------------------------------------------------------------------------

/// A --batch-file where one URL succeeds and one times out must report the
/// failure WITHOUT discarding the success: the fast URL's .md is still
/// written, the run exits 69 and stderr names the slow URL with the timeout
/// keyword — the partial-failure half of the batch timeout contract (#2517).
///
/// #2271 anchor: this test used to pin "does not hang" with a wall clock
/// (`elapsed < 25 s` — exactly what failed at 26.2 s on a loaded machine
/// while the product was correct). #2498 removed the clock in favour of the
/// observable contract, and #2517 gave this scenario its own property so the
/// twin timeout tests stopped asserting the same thing
/// (R2-twin-batch-timeout-tests): `batch_file_timeout_reports_failures` keeps
/// the all-failures anchor, this one pins survival of the successful sibling.
///
/// The attempt runs under [`assert_spawn_within_blocking`]: the shared budget
/// is the only clock in the test, and an overrunning child is killed and
/// reaped instead of being orphaned by a bare tokio timeout (#2387).
#[tokio::test]
async fn batch_file_timeout_does_not_hang() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/fast"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Fast</h1>\
             <p>This fast article carries plenty of substantive text so it \
             comfortably clears the fifty character minimum content guard.</p>\
             </article></body></html>",
        ))
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&t.server)
        .await;

    let uri = t.server.uri();
    let slow_url = format!("{uri}/slow");
    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{uri}/fast\n{slow_url}\n")).unwrap();
    let out_path = t.out.path().to_path_buf();

    assert_spawn_within_blocking(
        SPAWN_LATENCY_BUDGET,
        "batch_file_timeout_does_not_hang",
        move |child| {
            let mut command = std_cmd();
            command
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--timeout-secs")
                .arg("1")
                .arg("--output")
                .arg(&out_path);
            let output = child.output(command)?;

            if output.status.code() != Some(69) {
                return Err(format!(
                    "expected exit code 69 (partial batch failure), got {:?}; stderr: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            // The survival contract (#2517): the fast URL's page is exported
            // even though its sibling timed out — verified empirically before
            // pinning. A content-poor fast page would instead fail
            // ExtractionFailed and contribute to the all-items-failed
            // exit-65 semantics, which is why the fixture carries real body
            // text.
            let md = md_files_by_relative_path(&out_path);
            if !md.keys().any(|k| k.contains("fast")) {
                return Err(format!(
                    "the fast URL must still produce its .md after the sibling \
                     timeout, got {md:?}"
                ));
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains(&slow_url) {
                return Err(format!(
                    "stderr should contain the failed URL, got: {stderr}"
                ));
            }
            let lowered = stderr.to_lowercase();
            if !(lowered.contains("timeout") || lowered.contains("timed out")) {
                return Err(format!("stderr should mention timeout, got: {stderr}"));
            }
            Ok(())
        },
    )
    .await;
}

/// A --batch-file with a slow endpoint and --timeout-secs 1 must exit 69
/// and stderr must contain the failed URL and a timeout keyword.
///
/// #2271 sibling conversion: same observable contract, same budget machinery,
/// same reason — the bare tokio timeout this test used to carry was a clock
/// that did not depend on the product and did not kill the child it abandoned.
#[tokio::test]
async fn batch_file_timeout_reports_failures() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&t.server)
        .await;

    let batch_file = t.out.path().join("urls.txt");
    let slow_url = format!("{}/slow", t.server.uri());
    std::fs::write(&batch_file, format!("{slow_url}\n")).unwrap();
    let out_path = t.out.path().to_path_buf();

    assert_spawn_within_blocking(
        SPAWN_LATENCY_BUDGET,
        "batch_file_timeout_reports_failures",
        move |child| {
            let mut command = std_cmd();
            command
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--timeout-secs")
                .arg("1")
                .arg("--output")
                .arg(&out_path);
            let output = child.output(command)?;

            if output.status.code() != Some(69) {
                return Err(format!(
                    "expected exit code 69, got {:?}",
                    output.status.code()
                ));
            }

            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains(&slow_url) {
                return Err(format!(
                    "stderr should contain the failed URL, got: {stderr}"
                ));
            }
            let lowered = stderr.to_lowercase();
            if !(lowered.contains("timeout") || lowered.contains("timed out")) {
                return Err(format!("stderr should mention timeout, got: {stderr}"));
            }
            Ok(())
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// #1215 (F-38 + F-02): --batch scrapes each URL (one page per URL).
// ---------------------------------------------------------------------------

/// `--batch` over 2 cross-linked fixture URLs must yield exactly 2 export.jsonl
/// lines over 2 unique URLs: each seed is scraped once, never BFS-expanded
/// (which fetched /other and appended 6 lines over 3 unique URLs), and the
/// append-only export never holds duplicate (url, content_hash) records.
#[tokio::test]
async fn batch_scrapes_each_url_once_no_jsonl_duplicates() {
    let t = BehavioralTest::new().await;
    mount_article_graph(&t.server).await;
    spawn_batch_fixture("batch_scrapes_each_url_once_no_jsonl_duplicates", &t, &[]).await;

    let lines = read_jsonl_lines(t.out.path());
    let unique_urls: HashSet<String> = lines
        .iter()
        .filter_map(|v| v["url"].as_str().map(str::to_string))
        .collect();
    let unique_records: HashSet<(String, String)> = lines.iter().map(record_identity).collect();

    assert_eq!(
        lines.len(),
        2,
        "--batch must scrape exactly the 2 seed URLs (got {} export.jsonl lines)",
        lines.len()
    );
    assert_eq!(
        unique_urls.len(),
        2,
        "export.jsonl must hold 2 unique URLs, got {unique_urls:?}"
    );
    assert_eq!(
        unique_records.len(),
        lines.len(),
        "export.jsonl must hold no duplicate (url, content_hash) records"
    );

    // The BFS expansion fetched /other; a scrape-per-URL run must never touch it.
    let requests = t.server.received_requests().await.unwrap();
    let other_hits = requests.iter().filter(|r| r.url.path() == "/other").count();
    assert_eq!(
        other_hits, 0,
        "--batch must not crawl links discovered on seed pages (/other was fetched {other_hits}x)"
    );
}

/// `--batch --single-page` must equal per-URL `--single-page` scrapes: every
/// .md file byte-identical, export.jsonl identical modulo the per-run
/// `timestamp_utc` (a run-time instant by design, proven to be the ONLY
/// divergence between two identical back-to-back runs).
#[tokio::test]
async fn batch_single_page_matches_per_url_single_page() {
    let t = BehavioralTest::new().await;
    mount_article_graph(&t.server).await;
    let uri = t.server.uri();
    let urls = [format!("{uri}/article"), format!("{uri}/article/2")];

    // Batch run over both seeds with --single-page. This test spawns the
    // binary THREE times (this run plus the two reference runs below), each
    // under the shared budget with one retry, so it carries its own 600s
    // ceiling in .config/nextest.toml (#2658 backlog).
    spawn_batch_fixture(
        "batch_single_page_matches_per_url_single_page",
        &t,
        &["--single-page"],
    )
    .await;
    let batch_md = md_files_by_relative_path(t.out.path());
    let batch_jsonl = read_jsonl_lines(t.out.path());

    // Per-URL --single-page reference runs, one output dir each.
    let ref1 = TempDir::new().unwrap();
    let ref2 = TempDir::new().unwrap();
    for (url, dir) in [(&urls[0], &ref1), (&urls[1], &ref2)] {
        let mut command = std_cmd();
        command
            .arg("--url")
            .arg(url)
            .arg("--single-page")
            .arg("--output")
            .arg(dir.path())
            .arg("--no-checkpoint")
            .arg("--delay-ms")
            .arg("0");
        spawn_ok(
            "batch_single_page_matches_per_url_single_page",
            command,
            None,
        )
        .await;
    }
    let mut reference_md = md_files_by_relative_path(ref1.path());
    reference_md.extend(md_files_by_relative_path(ref2.path()));
    let mut reference_records: Vec<(String, String)> = Vec::new();
    for dir in [ref1.path(), ref2.path()] {
        reference_records.extend(read_jsonl_lines(dir).iter().map(record_identity));
    }

    assert_eq!(
        batch_md, reference_md,
        "--batch --single-page .md files must be byte-identical to per-URL --single-page"
    );

    let mut batch_records: Vec<(String, String)> =
        batch_jsonl.iter().map(record_identity).collect();
    batch_records.sort();
    reference_records.sort();
    assert_eq!(
        batch_records, reference_records,
        "--batch --single-page export.jsonl must hold the same (url, content_hash) \
         records as the per-URL --single-page runs"
    );
}

/// `--batch --max-pages 1` must still scrape EVERY seed (#1215: the report of
/// seeds silently dropped with exit 0). In scrape-per-URL mode the crawl
/// budget never gates seed fetching — each seed is one page by construction.
#[tokio::test]
async fn batch_max_pages_one_still_scrapes_every_seed() {
    let t = BehavioralTest::new().await;
    mount_article_graph(&t.server).await;
    spawn_batch_fixture(
        "batch_max_pages_one_still_scrapes_every_seed",
        &t,
        &["--max-pages", "1"],
    )
    .await;

    let lines = read_jsonl_lines(t.out.path());
    let unique_urls: HashSet<String> = lines
        .iter()
        .filter_map(|v| v["url"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        lines.len(),
        2,
        "--max-pages 1 must not starve batch seeds (got {} lines)",
        lines.len()
    );
    assert_eq!(unique_urls.len(), 2, "both seeds must be scraped");
}

// ---------------------------------------------------------------------------
// #2795 — batch input reconciliation: the final summary reconciles discarded
// input lines, the per-line warning carries the line number, `--strict-input`
// turns a rejected line into exit 78, and the no-URLs error reports counts.
// The discard itself stays intentional (F-R3-1): these tests pin operator
// reconciliation, never the removal of the discard.
// ---------------------------------------------------------------------------

/// Mount one single-route 200 article whose body clears the extractor's
/// content floor. One builder for the single-URL fixtures — the jscpd
/// ratchet counts three inline copies of the same mount block otherwise.
async fn mount_single_article(server: &MockServer, route: &str, title: &str) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<html><body><article><h1>{title}</h1>\
             <p>Substantive body text so the extractor accepts this page.</p>\
             </article></body></html>"
        )))
        .mount(server)
        .await;
}

/// Shared shell for the counting-error batch tests (#1697 + #2795): spawn
/// `--batch-file <file> [extra args]` under the spawn budget and expect one
/// exact exit code plus every stderr substring in `must_contain`. One
/// builder so the jscpd ratchet counts the BudgetChild boilerplate once,
/// not once per test.
fn assert_batch_file_exit(
    what: &'static str,
    batch_file: std::path::PathBuf,
    extra_args: &[&str],
    output_dir: &std::path::Path,
    expected_code: i32,
    must_contain: &'static [&'static str],
) {
    let extra: Vec<String> = extra_args.iter().map(ToString::to_string).collect();
    let output_dir = output_dir.to_path_buf();
    assert_spawn_within(SPAWN_LATENCY_BUDGET, what, move |child| {
        let mut command = std_cmd();
        command
            .arg("--batch-file")
            .arg(&batch_file)
            .arg("--output")
            .arg(&output_dir);
        for arg in &extra {
            command.arg(arg);
        }
        let output = child.output(command)?;

        if output.status.code() != Some(expected_code) {
            return Err(format!(
                "expected exit code {expected_code}, got {:?}; stderr: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        for needle in must_contain {
            if !stderr.contains(needle) {
                return Err(format!(
                    "stderr must contain {needle:?}, got: {}",
                    stderr.trim()
                ));
            }
        }
        Ok(())
    });
}

/// S1/S5 (#2795): a 4-line input — 2 valid URLs (one OK, one HTTP 500), 1
/// relative path, 1 blank line — reconciles in ONE summary line: the
/// succeeded/failed totals over the valid URLs, then the input accounting
/// (lines read = valid + rejected + blank). Before #2795 the summary said
/// `2/2`-style totals only and the discarded lines vanished from it.
#[tokio::test]
async fn batch_summary_reconciles_discarded_lines() {
    let t = BehavioralTest::new().await;
    mount_single_article(&t.server, "/ok", "Ok").await;
    Mock::given(method("GET"))
        .and(path("/broken"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&t.server)
        .await;

    let uri = t.server.uri();
    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(
        &batch_file,
        format!("{uri}/ok\n{uri}/broken\nnot-a-url/relative/path\n\n"),
    )
    .unwrap();
    let out_path = t.out.path().to_path_buf();

    assert_spawn_within_blocking(
        SPAWN_LATENCY_BUDGET,
        "batch_summary_reconciles_discarded_lines",
        move |child| {
            let mut command = std_cmd();
            command
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--output")
                .arg(&out_path);
            let output = child.output(command)?;
            let stdout = String::from_utf8_lossy(&output.stdout);

            if !stdout.contains("Batch complete: 1/2 succeeded, 1 failed") {
                return Err(format!(
                    "summary must reconcile the 2 valid URLs (1 ok, 1 failed), got: {stdout}"
                ));
            }
            if !stdout.contains("4 lines read: 2 valid, 1 rejected, 1 blank, 0 comment") {
                return Err(format!(
                    "summary must reconcile the discarded input lines, got: {stdout}"
                ));
            }
            Ok(())
        },
    )
    .await;
}

/// S2/S7 (#2795): the per-line rejection warning carries the physical input
/// line number and NEVER echoes the offending text (F-R3-1: a malformed line
/// can still carry credential-looking content).
#[tokio::test]
async fn batch_reject_warning_carries_line_number() {
    let t = BehavioralTest::new().await;
    mount_single_article(&t.server, "/only", "Only").await;

    let uri = t.server.uri();
    // Line 1 valid, line 2 blank, line 3 invalid: the warning must say `line: 3`.
    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(
        &batch_file,
        format!("{uri}/only\n\nnot-a-url/relative/path\n"),
    )
    .unwrap();
    let out_path = t.out.path().to_path_buf();

    assert_spawn_within_blocking(
        SPAWN_LATENCY_BUDGET,
        "batch_reject_warning_carries_line_number",
        move |child| {
            let mut command = std_cmd();
            command
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--output")
                .arg(&out_path);
            let output = child.output(command)?;
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !stderr.contains("batch line rejected by URL entry validation, line: 3") {
                return Err(format!(
                    "warning must carry the physical line number, got: {stderr}"
                ));
            }
            if stderr.contains("not-a-url") {
                return Err("the warning must never echo the offending line text (F-R3-1)".into());
            }
            Ok(())
        },
    )
    .await;
}

/// S3 (#2795): `--strict-input` turns any rejected input line into exit 78
/// with a message that counts the lines, instead of continuing with the rest.
/// The gate fires at entry validation — before any fetch — so this test needs
/// no server: the valid-looking URL is never fetched.
#[test]
fn batch_strict_input_fails_on_rejected_line() {
    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(
        &batch_file,
        "http://127.0.0.1:9/never-fetched\nnot-a-url/relative/path\n",
    )
    .unwrap();

    assert_batch_file_exit(
        "batch_strict_input_fails_on_rejected_line",
        batch_file,
        &["--strict-input"],
        temp.path(),
        78,
        &["--strict-input activo: la entrada tiene líneas rechazadas \
             (líneas leídas: 2, válidas: 1, rechazadas: 1"],
    );
}

/// S3/S5 boundary (#2795): blank and comment lines are legal input, not
/// rejected lines — `--strict-input` does NOT trip on them, and the summary
/// still shows their counts so the arithmetic closes.
#[tokio::test]
async fn batch_strict_input_ignores_blank_and_comment_lines() {
    let t = BehavioralTest::new().await;
    mount_single_article(&t.server, "/legal", "Legal").await;

    let uri = t.server.uri();
    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(
        &batch_file,
        format!("# list generated by the operator\n{uri}/legal\n\n"),
    )
    .unwrap();
    let out_path = t.out.path().to_path_buf();

    assert_spawn_within_blocking(
        SPAWN_LATENCY_BUDGET,
        "batch_strict_input_ignores_blank_and_comment_lines",
        move |child| {
            let mut command = std_cmd();
            command
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--strict-input")
                .arg("--output")
                .arg(&out_path);
            let output = child.output(command)?;

            if !output.status.success() {
                return Err(format!(
                    "blank and comment lines must not trip --strict-input, got {:?}; \
                     stderr: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.contains("3 lines read: 1 valid, 0 rejected, 1 blank, 1 comment") {
                return Err(format!(
                    "summary must reconcile the legal non-URL lines, got: {stdout}"
                ));
            }
            Ok(())
        },
    )
    .await;
}

/// S6 (#2795): a file with NO valid URL reports what it read and why it
/// failed — per-line warnings with line numbers plus a counting error — not
/// a bare "no URLs". Exit stays 64 (matrix row 15: InvalidUrl → usage error).
#[test]
fn batch_all_invalid_file_reports_counts() {
    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, "not-a-url/one\nalso-bad/two\n").unwrap();

    assert_batch_file_exit(
        "batch_all_invalid_file_reports_counts",
        batch_file,
        &[],
        temp.path(),
        64,
        &[
            "No hay URLs para procesar (líneas leídas: 2, válidas: 0, rechazadas: 2",
            "line: 1, reason:",
            "line: 2, reason:",
        ],
    );
}

/// T3 characterization (#2795): a non-empty file of only blank lines fails
/// with the counts — 3 lines read, 3 blank — not a bare "no URLs". Exit 64.
#[test]
fn batch_blank_only_file_reports_counts() {
    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, "\n\n\n").unwrap();

    assert_batch_file_exit(
        "batch_blank_only_file_reports_counts",
        batch_file,
        &[],
        temp.path(),
        64,
        &[
            "No hay URLs para procesar (líneas leídas: 3, válidas: 0, rechazadas: 0, \
             vacías: 3",
        ],
    );
}
