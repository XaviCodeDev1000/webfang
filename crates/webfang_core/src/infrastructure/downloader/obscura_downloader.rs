//! Obscura subprocess downloader.
//!
//! Wraps the `obscura` CLI tool via `tokio::process::Command`, which keeps the
//! runtime non-blocking and — crucially — owns the child handle so the
//! timeout can kill it (XP-S-05, #1608): `kill_on_drop(true)` terminates the
//! subprocess when the `Child` future is dropped, so a timed-out fetch never
//! leaves an orphaned obscura process behind.
//! Returns **HTML** output (`fetch --dump html`, #793) so Readability,
//! CSS-selector extraction, and WAF inspection receive the format they expect;
//! the returned [`FetchedPage`] carries a `content-type: text/html` header
//! marker so downstream sniffing stays honest.
//!
//! The binary is configurable through `ObscuraDownloader::new`
//! (#787): a value carrying a path separator is invoked exactly as given
//! (absolute or relative path); a bare name is resolved from `PATH` by the
//! OS. The default is the bare name `obscura`. Hybrid preflight enforces a
//! minimum binary version of 0.2.5 (REQ-OBS-02, cli/preflight.rs).
//!
//! **What Layer 2 is — a cheap render, not an anti-WAF guarantee (#1710).**
//! L2 renders a page far cheaper than the Chromiumoxide Layer 3 (~30 MB per
//! subprocess against ~200 MB + CDP). That is the whole claim: it is NOT an
//! anti-WAF guarantee. webfang never passes obscura's `-stealth` variant, and
//! a WAF challenge surfaced at any layer aborts the escalation instead of
//! being retried against it (see `hybrid_router.rs`). The L2-vs-L3 hit-rate
//! is derived from the per-layer log events the router already emits — there
//! are no layer counters, and no strict mode.
//!
//! **Sessionless contract (REQ-OBS-03):** Layer 2 injects no cookies and keeps
//! no storage directory (`--storage-dir` is deliberately not wired — see
//! Engram obs #2358). Pages behind a session are
//! expected to render empty here and escalate to Layer 3 (Chromiumoxide),
//! which injects [`CookieBridge`](crate::domain::cookie_bridge::CookieBridge)
//! cookies by domain.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, instrument, warn};
use url::Url;

use super::{DownloadError, Downloader, FetchedPage};

/// Memory budget for one Obscura subprocess (~30 MB).
const OBSCURA_MEMORY_COST: usize = 30_000_000;

/// Default timeout per page fetch.
const DEFAULT_OBSCURA_TIMEOUT: Duration = Duration::from_secs(15);

/// Auxiliary cap for the child's stderr read (log lines plus the
/// `Page loaded:` navigation report). Truncated, never fatal — stderr only
/// feeds error messages and the final-URL parse (#2722).
const OBSCURA_STDERR_CAP: u64 = 64 * 1024;

/// Default binary name, resolved from `PATH` at spawn time.
///
/// ADR-0012 sub-slice 3.B-1b moved the canonical literal to
/// [`crate::domain::downloader_factory::DEFAULT_OBSCURA_BINARY`] because
/// `application::crawler` needs it for [`EngineOptions::default`] and must
/// not name an infrastructure downloader item. This `pub use` keeps the
/// historical `webfang_core::infrastructure::downloader::obscura_downloader::DEFAULT_OBSCURA_BINARY`
/// path resolving without duplicating the value.
///
/// [`EngineOptions::default`]: crate::application::crawler::engine::EngineOptions
pub use crate::domain::downloader_factory::DEFAULT_OBSCURA_BINARY;

use crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES;

/// Synthetic content-type for Layer 2 pages (#793): a subprocess downloader
/// has no real response headers, so we mark what obscura was asked to dump.
/// The lowercase key + `text/html` mime are the shape
/// `InspectionContext::from_lowercase_headers` and the WAF engine's
/// `is_html_content_type` consume.
const OBSCURA_CONTENT_TYPE: &str = "text/html; charset=utf-8";

fn obscura_html_headers() -> HashMap<String, String> {
    HashMap::from([("content-type".to_string(), OBSCURA_CONTENT_TYPE.to_string())])
}

/// Prefix of obscura's stderr navigation line (v0.2.6/v0.2.7): the CLI emits
/// `Page loaded: <final-url> - "<title>"` after every navigation, gated only
/// by `--quiet` — an `eprintln!` that bypasses the tracing subscriber, so it
/// reaches the captured stderr at every log level.
const PAGE_LOADED_PREFIX: &str = "Page loaded: ";

/// Parse the final URL obscura reports on stderr (#2722 H4).
///
/// The line is free text upstream: anything unparseable returns `None` and
/// the caller degrades to the requested URL. Only absolute `http`/`https`
/// URLs are accepted — the entry `ValidUrl` guard never admits other
/// schemes, so a `file://` render must not pass as a redirect target.
fn parse_page_loaded_url(stderr: &str) -> Option<Url> {
    let line = stderr
        .lines()
        .find(|line| line.starts_with(PAGE_LOADED_PREFIX))?;
    let rest = line.strip_prefix(PAGE_LOADED_PREFIX)?.trim();
    // The URL runs to the ` - "<title>"` tail; a future upstream format
    // without the title still parses through the `unwrap_or` fallback.
    let raw = rest
        .split_once(" - \"")
        .map(|(url_part, _)| url_part)
        .unwrap_or(rest)
        .trim();
    let parsed = Url::parse(raw).ok()?;
    matches!(parsed.scheme(), "http" | "https").then_some(parsed)
}

/// Subprocess-based downloader that shells out to `obscura fetch --dump html`
/// (#793 — was markdown; the downstream pipeline is HTML-only).
///
/// No cookies, no connection pool — each invocation is independent and
/// sessionless (REQ-OBS-03).
pub struct ObscuraDownloader {
    timeout: Duration,
    binary: PathBuf,
    /// Operator-pinned User-Agent (#503): when `Some`, propagated to obscura
    /// as `--user-agent` (#2722 H5). `None` keeps obscura's own default UA —
    /// Layer 1's UA-pool rotation is deliberately not replicated here.
    user_agent: Option<String>,
    /// Stage 5 of the fetch guard chain: cap for the child's stdout read.
    /// Beyond the cap the fetch aborts with
    /// [`DownloadError::BodyTooLarge`] instead of buffering unbounded output.
    max_page_bytes: u64,
}

impl ObscuraDownloader {
    /// Build a downloader running `binary` for every fetch (#787).
    ///
    /// `binary` is either the name of an executable resolved from `PATH`
    /// (e.g. `"obscura"`) or an explicit path invoked as-is. An empty value
    /// falls back to [`DEFAULT_OBSCURA_BINARY`] so `Command::new("")` never
    /// reaches the OS.
    pub(crate) fn new(timeout_secs: u64, binary: impl Into<PathBuf>) -> Self {
        let binary = binary.into();
        let binary = if binary.as_os_str().is_empty() {
            PathBuf::from(DEFAULT_OBSCURA_BINARY)
        } else {
            binary
        };
        Self {
            timeout: Duration::from_secs(timeout_secs),
            binary,
            user_agent: None,
            max_page_bytes: DEFAULT_MAX_PAGE_BYTES,
        }
    }

    /// Pin the User-Agent obscura sends on Layer 2 (#503, #2722 H5): the
    /// value travels to obscura's argv as `--user-agent`. `None` (the
    /// default) passes no flag — obscura keeps its own default UA.
    pub(crate) fn with_user_agent(mut self, user_agent: Option<String>) -> Self {
        self.user_agent = user_agent;
        self
    }

    /// Cap the child's stdout read at `max_page_bytes` (stage 5 of the fetch
    /// guard chain). Defaults to [`DEFAULT_MAX_PAGE_BYTES`].
    pub(crate) fn with_max_page_bytes(mut self, max_page_bytes: u64) -> Self {
        self.max_page_bytes = max_page_bytes;
        self
    }

    /// The configured obscura binary (path or bare name) — test accessor.
    #[cfg(test)]
    pub(crate) fn binary(&self) -> &std::path::Path {
        &self.binary
    }

    /// The pinned User-Agent — test accessor for the builder wiring (#2722).
    #[cfg(test)]
    pub(crate) fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }

    /// The configured stdout cap — test accessor for the builder wiring.
    #[cfg(test)]
    pub(crate) fn max_page_bytes(&self) -> u64 {
        self.max_page_bytes
    }
}

impl Default for ObscuraDownloader {
    fn default() -> Self {
        Self::new(DEFAULT_OBSCURA_TIMEOUT.as_secs(), DEFAULT_OBSCURA_BINARY)
    }
}

impl ObscuraDownloader {
    #[instrument(skip(self), fields(url = %url, binary = %self.binary.display()))]
    async fn fetch_inner(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        debug!(
            binary = %self.binary.display(),
            "Obscura fetch: {url}"
        );

        let url_string = url.to_string();
        let binary = self.binary.clone();

        // Defense in depth: a typed Url always carries a scheme (e.g. https://) so it can
        // never be mistaken for an obscura flag; reject defensively anyway.
        // LCOV_EXCL_START defensive: defense-in-depth — a typed Url always carries a scheme, never a leading '-'
        if url_string.starts_with('-') {
            return Err(DownloadError::Internal(
                "URL inválida: no puede comenzar con '-'".to_string(),
            ));
        }
        // LCOV_EXCL_STOP

        // XP-S-05 (#1608): the child handle is owned by the awaited future
        // with `kill_on_drop(true)`. On timeout the future (and the handle)
        // is dropped, which terminates the subprocess — no orphaned obscura
        // keeps fetching after the caller gave up. No Mutex is held across
        // the await: the `Child` is moved into the future, never shared.
        let mut command = Command::new(&binary);
        command
            .args(["fetch", "--dump", "html"])
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // S1 (#2722 H5): the operator-pinned User-Agent travels to obscura's
        // argv; `None` passes no flag and obscura keeps its own default UA.
        if let Some(ua) = &self.user_agent {
            command.args(["--user-agent", ua]);
        }
        command.arg(&url_string);

        let child = command.spawn().map_err(|e| {
            DownloadError::Internal(format!("obscura process failed to start: {e}"))
        })?;

        // Stage 5 of the fetch guard chain (#2722): the stdout read is capped
        // at `max_page_bytes` (+1 detects overflow) and the stderr read at a
        // small auxiliary cap, both drained concurrently with `wait()` so a
        // chatty child can never deadlock the pipes or inflate memory.
        let max_page_bytes = self.max_page_bytes;
        let result =
            timeout(self.timeout, async move {
                let mut child = child;
                let stdout = child.stdout.take().ok_or_else(|| {
                    DownloadError::Internal("obscura stdout pipe unavailable".into())
                })?;
                let stderr = child.stderr.take().ok_or_else(|| {
                    DownloadError::Internal("obscura stderr pipe unavailable".into())
                })?;

                let mut out_buf = Vec::new();
                let mut err_buf = Vec::new();
                let mut capped_out = stdout.take(max_page_bytes.saturating_add(1));
                let mut capped_err = stderr.take(OBSCURA_STDERR_CAP.saturating_add(1));
                let (out_res, _, wait_res) = tokio::join!(
                    capped_out.read_to_end(&mut out_buf),
                    capped_err.read_to_end(&mut err_buf),
                    child.wait(),
                );
                out_res.map_err(DownloadError::Io)?;
                Ok((out_buf, err_buf, wait_res.ok()))
            })
            .await;

        match result {
            Ok(Ok((out_bytes, err_bytes, exit_status))) => {
                if !exit_status.is_some_and(|status| status.success()) {
                    let stderr = String::from_utf8_lossy(&err_bytes);
                    let reported = exit_status
                        .map(|status| status.to_string())
                        .unwrap_or_else(|| "unknown (wait failed)".to_string());
                    return Err(DownloadError::Internal(format!(
                        "obscura exited with {reported}: {stderr}"
                    )));
                }

                // The take() reader stops at cap+1: one byte over means the
                // rendered page exceeded the limit.
                if out_bytes.len() as u64 > max_page_bytes {
                    return Err(DownloadError::BodyTooLarge {
                        limit: max_page_bytes,
                    });
                }

                let html = String::from_utf8_lossy(&out_bytes).to_string();

                // S2 (#2722 H4): obscura reports the final URL on stderr.
                // The line is free text upstream: a parse failure degrades to
                // the requested URL with a WARN — it never fails the fetch.
                let final_url = parse_page_loaded_url(&String::from_utf8_lossy(&err_bytes))
                    .unwrap_or_else(|| {
                        warn!(
                            url = %url,
                            "obscura stderr did not report the final URL — keeping the requested URL"
                        );
                        url.clone()
                    });

                debug!(
                    dump = "html",
                    bytes = html.len(),
                    final_url = %final_url,
                    status = 200,
                    status_source = "synthesized",
                    "Obscura returned {} bytes",
                    html.len()
                );

                Ok(FetchedPage {
                    url: final_url,
                    html,
                    status: 200,
                    headers: obscura_html_headers(),
                    cookies: vec![],
                })
            },
            Ok(Err(e)) => Err(e),
            Err(_) => Err(DownloadError::timeout(self.timeout.as_secs())),
        }
    }
}

impl Downloader for ObscuraDownloader {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(self.fetch_inner(url))
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        OBSCURA_MEMORY_COST
    }
}

/// #2235: the `--skip obscura_downloader` of the Miri lane is GONE — a
/// submodule skip deleted this whole test tree from the lane and hid the UB
/// living inside it. The tests that spawn the `obscura` binary carry their
/// OWN gate (Command::spawn is unsupported by Miri); the pure ones (the
/// constructor and the binary-path contract) run under Miri.
#[cfg(test)]
mod tests {
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

    /// #793 (REQ-OBS-01): Layer 2 must ask obscura for `--dump html` —
    /// never markdown — and the HTML printed on stdout lands verbatim in
    /// `FetchedPage.html`.
    #[cfg_attr(miri, ignore = "Command::spawn unsupported by Miri (#775)")]
    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_dump_html_args_and_page() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML, "");

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com/page".parse().expect("valid test URL");
        let page = dl.fetch(&url).await.expect("fake obscura fetch");

        assert!(
            page.html.contains("<h1>Obscura Article</h1>"),
            "FetchedPage.html must carry the HTML obscura printed on stdout, got: {}",
            page.html
        );
        assert_eq!(page.status, 200);

        let args = std::fs::read_to_string(tmp.path().join("args.txt"))
            .expect("fake obscura must have recorded its argv");
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
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML, "");

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com".parse().expect("valid test URL");
        let page = dl.fetch(&url).await.expect("fake obscura fetch");

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
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML, "");

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com".parse().expect("valid test URL");
        let page = dl.fetch(&url).await.expect("fake obscura fetch");

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

        let article = crate::infrastructure::scraper::readability::parse(
            &page.html,
            Some("https://example.com"),
        )
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
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML, "");

        let dl = ObscuraDownloader::new(15, &bin_path)
            .with_user_agent(Some("OperatorUA/3.1".to_string()));
        let url: Url = "https://example.com/page".parse().expect("valid test URL");
        dl.fetch(&url).await.expect("fake obscura fetch");

        let args = std::fs::read_to_string(tmp.path().join("args.txt"))
            .expect("fake obscura must have recorded its argv");
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
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML, "");

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com/page".parse().expect("valid test URL");
        dl.fetch(&url).await.expect("fake obscura fetch");

        let args = std::fs::read_to_string(tmp.path().join("args.txt"))
            .expect("fake obscura must have recorded its argv");
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
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let stderr = "Fetching https://example.com/page...\n\
                      Page loaded: https://redirected.example/final - \"Final Title\"\n";
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML, stderr);

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com/page".parse().expect("valid test URL");
        let page = dl.fetch(&url).await.expect("fake obscura fetch");

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
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML, "");

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com/page".parse().expect("valid test URL");
        let page = dl.fetch(&url).await.expect("fake obscura fetch");

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
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let big_body = "x".repeat(256);
        let bin_path = write_fake_obscura(tmp.path(), &big_body, "");

        let dl = ObscuraDownloader::new(15, &bin_path).with_max_page_bytes(64);
        let url: Url = "https://example.com/big".parse().expect("valid test URL");
        let err = dl
            .fetch(&url)
            .await
            .expect_err("output past the cap must abort the fetch");
        assert!(
            matches!(err, DownloadError::BodyTooLarge { limit: 64 }),
            "expected BodyTooLarge {{ limit: 64 }}, got: {err:?}"
        );
    }
}
