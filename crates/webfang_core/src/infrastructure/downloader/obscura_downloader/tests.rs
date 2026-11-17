//! #2235: the `--skip obscura_downloader` of the Miri lane is GONE — a
//! submodule skip deleted this whole test tree from the lane and hid the UB
//! living inside it. The tests that spawn the `obscura` binary carry their
//! OWN gate (Command::spawn is unsupported by Miri); the pure ones (the
//! constructor and the binary-path contract) run under Miri.
use super::*;

/// Base URL for the selector path exercised below.
///
/// `#[cfg(unix)]` because its only caller spawns the fake obscura
/// binary, which is unix-only: on Windows the helper would be dead code.
#[cfg(unix)]
fn base() -> url::Url {
    url::Url::parse("https://example.com/").expect("static base URL is valid")
}

#[test]
fn test_obscura_downloader_basics() {
    let dl = ObscuraDownloader::new(15, "obscura");
    assert!(!dl.supports_interactions());
    assert_eq!(dl.memory_cost(), 30_000_000);
}

#[test]
fn test_obscura_default() {
    let dl = ObscuraDownloader::default();
    assert_eq!(dl.memory_cost(), OBSCURA_MEMORY_COST);
    assert_eq!(dl.binary(), PathBuf::from(DEFAULT_OBSCURA_BINARY).as_path());
}

/// #787: the configured binary is stored and used at spawn time — a
/// custom path is not silently replaced by the bare `PATH` name.
#[test]
fn test_obscura_stores_configured_binary() {
    let dl = ObscuraDownloader::new(15, "/opt/tools/obscura");
    assert_eq!(dl.binary(), PathBuf::from("/opt/tools/obscura").as_path());
}

/// #787: an empty binary value falls back to the default bare name so
/// `Command::new("")` never reaches the OS.
#[test]
fn test_obscura_empty_binary_falls_back_to_default() {
    let dl = ObscuraDownloader::new(15, "");
    assert_eq!(dl.binary(), PathBuf::from(DEFAULT_OBSCURA_BINARY).as_path());
}

/// #787: a configured missing binary surfaces as a spawn failure, never
/// as a fallback to whatever `obscura` happens to be on PATH.
#[cfg_attr(
    miri,
    ignore = "Command::spawn → posix_spawnattr_init unsupported by Miri (#775)"
)]
#[tokio::test]
async fn test_obscura_missing_binary_fails_fetch() {
    let dl = ObscuraDownloader::new(5, "/definitely/not/here/obscura");
    let url: Url = "https://example.com".parse().expect("valid test URL");
    let err = dl
        .fetch(&url)
        .await
        .expect_err("a missing obscura binary must fail the fetch");
    match err {
        DownloadError::Internal(msg) => {
            assert!(
                msg.contains("failed to start"),
                "error must name the spawn failure, got: {msg}"
            );
        },
        other => panic!("expected spawn Internal error, got: {other:?}"),
    }
}

/// #787: a bare-name binary that is not on PATH also fails at spawn —
/// no binary resolution happens beyond what the OS provides.
#[cfg_attr(
    miri,
    ignore = "Command::spawn → posix_spawnattr_init unsupported by Miri (#775)"
)]
#[tokio::test]
async fn test_obscura_bare_name_missing_from_path_fails_fetch() {
    let dl = ObscuraDownloader::new(5, "definitely-not-a-binary-7f3k");
    let url: Url = "https://example.com".parse().expect("valid test URL");
    let result = dl
        .fetch(&url)
        .await
        .expect_err("a binary missing from PATH must fail the fetch");
    assert!(
        matches!(result, DownloadError::Internal(_)),
        "expected a spawn failure, got: {result:?}"
    );
}

// ---- XP-S-05 (#1608) — the timeout kills the child ------------------

/// /proc/<pid>/stat process state, if the process exists. `Z` (zombie)
/// counts as dead: the kernel slot lingers until reaped, but the process
/// no longer runs.
#[cfg(unix)]
fn linux_process_state(pid: i32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field may contain spaces; everything after its closing
    // paren starts with the state char.
    let after_comm = stat.rsplit(") ").next()?;
    after_comm.split_whitespace().next()?.chars().next()
}

/// XP-S-05 (#1608): a fetch that exceeds the timeout must TERMINATE the
/// obscura subprocess instead of abandoning it (the old `spawn_blocking`
/// + `output()` path orphaned the child). The fake binary records its
/// pid and sleeps far past the timeout; after the fetch reports
/// `Timeout`, the process must be gone (kill_on_drop).
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn timed_out_fetch_kills_the_obscura_child() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = tmp.path().join("obscura");
    let pid_path = tmp.path().join("pid.txt");
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"fetch\" ]; then\n  echo $$ > \"{pid}\"\n  sleep 30\nfi\n",
        pid = pid_path.display(),
    );
    std::fs::write(&bin_path, script).expect("write fake obscura");
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x fake obscura");

    let dl = ObscuraDownloader::new(1, &bin_path);
    let url: Url = "https://example.com/slow".parse().expect("valid test URL");
    let err = dl
        .fetch(&url)
        .await
        .expect_err("a fetch past the timeout must fail");
    assert!(
        matches!(err, DownloadError::Timeout { .. }),
        "expected Timeout, got: {err:?}"
    );

    // kill_on_drop sends the signal without waiting for the reap: poll
    // briefly until the process is gone (or a zombie).
    let pid_raw = std::fs::read_to_string(&pid_path).expect("fake obscura must record its pid");
    let pid: i32 = pid_raw.trim().parse().expect("integer pid");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let dead = match linux_process_state(pid) {
            None => true,
            Some(state) => state == 'Z',
        };
        if dead || std::time::Instant::now() >= deadline {
            assert!(dead, "obscura child (pid {pid}) survived the timeout");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

// ---- #793 — fake obscura executable --------------------------------

/// HTML body every fake-obscura fixture writes to disk.
///
/// `#[cfg(unix)]`: all three consumers are the tests that spawn the fake
/// binary, and those are unix-only.
#[cfg(unix)]
const FAKE_HTML: &str = "<!DOCTYPE html><html><body><article>\
     <h1>Obscura Article</h1>\
     <p>Rendered body text long enough for Readability to keep the article.</p>\
     </article></body></html>";

/// Write an executable fake `obscura` that records its argv to
/// `<dir>/args.txt` per `fetch` invocation, prints fixed HTML on stdout
/// and `stderr_line` on stderr (#793, #2722). Deterministic: no network,
/// no real obscura.
#[cfg(unix)]
fn write_fake_obscura(dir: &std::path::Path, body: &str, stderr_line: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let bin_path = dir.join("obscura");
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"fetch\" ]; then\n  \
         echo \"$@\" >> \"{dir}/args.txt\"\n  \
         printf '%s' '{body}'\n  \
         printf '%s' '{stderr}' >&2\nelse\n  echo \"obscura 0.2.7\"\nfi\n",
        dir = dir.display(),
        body = body.replace('\'', "'\\''"),
        stderr = stderr_line.replace('\'', "'\\''"),
    );
    std::fs::write(&bin_path, script).expect("write fake obscura");
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x fake obscura");
    bin_path
}

/// Core arrange for the fake-binary tests (#793, #2722): write the fixture,
/// build the downloader with optional UA / stdout-cap overrides, fetch
/// `url_str`, and return the recorded argv plus the fetch result. One
/// arrange block keeps the test bodies free of exact clones
/// (jscpd `--min-tokens 50`).
#[cfg(unix)]
async fn fake_fetch(
    body: &str,
    stderr_line: &str,
    user_agent: Option<&str>,
    max_page_bytes: Option<u64>,
    url_str: &str,
) -> (String, Result<FetchedPage, DownloadError>) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let bin_path = write_fake_obscura(tmp.path(), body, stderr_line);
    let mut dl = ObscuraDownloader::new(15, &bin_path);
    if let Some(ua) = user_agent {
        dl = dl.with_user_agent(Some(ua.to_string()));
    }
    if let Some(cap) = max_page_bytes {
        dl = dl.with_max_page_bytes(cap);
    }
    let url: Url = url_str.parse().expect("valid test URL");
    let result = dl.fetch(&url).await;
    let args = std::fs::read_to_string(tmp.path().join("args.txt"))
        .expect("fake obscura must have recorded its argv");
    (args, result)
}

/// #793 (REQ-OBS-01): Layer 2 must ask obscura for `--dump html` —
/// never markdown — and the HTML printed on stdout lands verbatim in
/// `FetchedPage.html`.
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn fake_binary_dump_html_args_and_page() {
    let (args, result) = fake_fetch(FAKE_HTML, "", None, None, "https://example.com/page").await;
    let page = result.expect("fake obscura fetch");

    assert!(
        page.html.contains("<h1>Obscura Article</h1>"),
        "FetchedPage.html must carry the HTML obscura printed on stdout, got: {}",
        page.html
    );
    assert_eq!(page.status, 200);

    assert!(
        args.contains("--dump html"),
        "Layer 2 must request `fetch --dump html`, got argv: {args}"
    );
    assert!(
        !args.contains("markdown"),
        "Layer 2 must never request markdown, got argv: {args}"
    );
    assert!(
        args.contains("https://example.com/page"),
        "the fetched URL must reach obscura, got argv: {args}"
    );
}

/// #793 (REQ-OBS-01 S1.3 / REQ-OBS-03): the returned page is self-
/// describing — a `text/html` content-type marker in the otherwise-empty
/// header map, and no session cookies.
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn fake_binary_page_headers_mark_html_and_stateless() {
    let (_, result) = fake_fetch(FAKE_HTML, "", None, None, "https://example.com").await;
    let page = result.expect("fake obscura fetch");

    assert_eq!(
        page.headers.get("content-type").map(String::as_str),
        Some("text/html; charset=utf-8"),
        "Layer 2 must mark its output as text/html for downstream sniffing"
    );
    assert!(
        page.cookies.is_empty(),
        "Layer 2 is sessionless — it never returns cookies"
    );
}

/// #793 (REQ-OBS-01 S1.2): the HTML returned by Layer 2 must survive the
/// real downstream extraction path — CSS selector matching AND
/// Readability — the exact consumers that broke on markdown output.
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn fake_binary_html_survives_extraction_path() {
    let (_, result) = fake_fetch(FAKE_HTML, "", None, None, "https://example.com").await;
    let page = result.expect("fake obscura fetch");

    match crate::application::extraction::extract_with_selector(
        &page.html,
        "article",
        &base(),
        None,
    ) {
        crate::domain::ExtractResult::Matched(html) => {
            assert!(
                html.contains("Obscura Article"),
                "selector extraction must find the article in the dumped HTML"
            );
        },
        other => panic!("`article` must match in the dumped HTML, got: {other:?}"),
    }

    let article =
        crate::infrastructure::scraper::readability::parse(&page.html, Some("https://example.com"))
            .expect("Readability must parse the dumped HTML");
    assert!(
        article.content.contains("Obscura Article"),
        "Readability content must retain the article body"
    );
    assert!(
        article.text_content.contains("Rendered body text"),
        "Readability text extraction must retain visible text"
    );
}

// ---- #2722 (Phase 1, L2 honesty) ------------------------------------

/// S1: a pinned User-Agent reaches obscura's argv as `--user-agent`
/// (#503 identity on every layer; upstream accepts the flag at
/// subcommand level — the value travels after `--dump html`).
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn pinned_user_agent_reaches_obscura_argv() {
    let (args, result) = fake_fetch(
        FAKE_HTML,
        "",
        Some("OperatorUA/3.1"),
        None,
        "https://example.com/page",
    )
    .await;
    result.expect("fake obscura fetch");

    assert!(
        args.contains("--user-agent OperatorUA/3.1"),
        "the pinned User-Agent must reach obscura's argv, got: {args}"
    );
}

/// S1: with no pinned UA the flag must NOT appear — obscura keeps its
/// own default and L1's rotation semantics stay L1-only.
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn unpinned_user_agent_passes_no_flag() {
    let (args, result) = fake_fetch(FAKE_HTML, "", None, None, "https://example.com/page").await;
    result.expect("fake obscura fetch");

    assert!(
        !args.contains("--user-agent"),
        "an unpinned UA must not pass --user-agent, got: {args}"
    );
}

/// S2: obscura reports the final URL after redirects on stderr —
/// `FetchedPage.url` must carry it (the type contract), not the
/// requested URL.
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn stderr_page_loaded_line_becomes_fetched_page_url() {
    let stderr = "Fetching https://example.com/page...\n\
                  Page loaded: https://redirected.example/final - \"Final Title\"\n";
    let (_, result) = fake_fetch(FAKE_HTML, stderr, None, None, "https://example.com/page").await;
    let page = result.expect("fake obscura fetch");

    let expected: Url = "https://redirected.example/final"
        .parse()
        .expect("static final URL is valid");
    assert_eq!(
        page.url, expected,
        "FetchedPage.url must be the final URL obscura reported"
    );
}

/// S2: the stderr line is free text upstream — a missing/unparseable
/// line degrades to the requested URL (with a WARN) and never fails
/// the fetch.
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn missing_page_loaded_line_falls_back_to_requested_url() {
    let (_, result) = fake_fetch(FAKE_HTML, "", None, None, "https://example.com/page").await;
    let page = result.expect("fake obscura fetch");

    let expected: Url = "https://example.com/page".parse().expect("valid test URL");
    assert_eq!(
        page.url, expected,
        "no stderr report must keep the requested URL"
    );
}

/// Pure parse contract (Miri-safe): extracts the URL before the
/// ` - "<title>"` tail, accepts a tail-less line, rejects non-http(s)
/// schemes and missing lines.
#[test]
fn parse_page_loaded_url_contract() {
    let parsed =
        parse_page_loaded_url("Fetching https://a...\nPage loaded: https://final/x - \"T\"\n")
            .expect("well-formed line parses");
    assert_eq!(parsed.as_str(), "https://final/x");

    assert_eq!(
        parse_page_loaded_url("Page loaded: https://final/x")
            .as_ref()
            .map(Url::as_str),
        Some("https://final/x"),
        "a line without the title tail still parses"
    );
    assert!(
        parse_page_loaded_url("Page loaded: file:///etc/passwd - \"t\"").is_none(),
        "non-http(s) schemes are rejected"
    );
    assert!(
        parse_page_loaded_url("Fetching https://a...").is_none(),
        "no Page loaded line → None"
    );
}

/// S4 (stage 5 of the fetch guard chain): stdout past the configured cap
/// aborts with `BodyTooLarge` instead of buffering unbounded output.
#[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
#[cfg(unix)]
#[tokio::test]
async fn oversized_stdout_aborts_with_body_too_large() {
    let big_body = "x".repeat(256);
    let (_, result) = fake_fetch(&big_body, "", None, Some(64), "https://example.com/big").await;
    let err = result.expect_err("output past the cap must abort the fetch");
    assert!(
        matches!(err, DownloadError::BodyTooLarge { limit: 64 }),
        "expected BodyTooLarge {{ limit: 64 }}, got: {err:?}"
    );
}
