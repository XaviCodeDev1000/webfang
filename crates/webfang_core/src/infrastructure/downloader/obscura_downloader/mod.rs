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
use std::process::ExitStatus;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tracing::{debug, info, instrument, warn};
use url::Url;

use super::{DownloadError, Downloader, FetchedPage};

/// Shared sandbox resolution; the file is declared here so both browser
/// downloader layers consume the same mount order and skip rules.
#[path = "../downloader_factory.rs"]
pub mod sandbox;

use sandbox::{arapuca_args, resolve_executable, SandboxConfig};

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

/// Emit the "wrapper applied" decision into FileTraceLayer.
///
/// The tenant/run id is the existing per-run task id; the top-level `trace_id`
/// is added by FileTraceLayer from the surrounding fetch span. No metrics
/// backend, global registry or extra correlation mechanism is introduced.
pub fn trace_sandbox_applied(layer: u8, task_id: &str, binary: &str) {
    info!(
        event = "sandbox_applied",
        layer,
        sandbox = "arapuca",
        task_id,
        binary,
        "browser subprocess sandboxed"
    );
}

/// Emit the "wrapper skipped" decision with its reason and selected binary.
pub fn trace_sandbox_skipped(layer: u8, task_id: &str, reason: &str, binary: &str) {
    warn!(
        event = "sandbox_skipped",
        layer, reason, task_id, binary, "sandbox unavailable — running browser unsandboxed"
    );
}

/// Emit one structured event per arapuca seccomp denial found on child stderr.
///
/// The vendored baseline profile returns `ENOSYS` for `landlock_*` and
/// `memfd_create` and a dedicated `EPERM` tier for `ptrace`; arapuca's debug
/// handler prints `seccomp: blocked syscall N (name)` for trapped syscalls.
/// Mapping the name back to errno keeps the denial actionable without
/// changing the sandbox profile or its argv.
pub fn trace_sandbox_denials(stderr: &str, layer: u8, task_id: &str) {
    for line in stderr.lines() {
        let Some(rest) = line.strip_prefix("seccomp: blocked syscall ") else {
            continue;
        };
        let Some((number, tail)) = rest.split_once(' ') else {
            continue;
        };
        if number.parse::<u32>().is_err() {
            continue;
        }
        let syscall = tail.trim().trim_start_matches('(').trim_end_matches(')');
        if syscall.is_empty() {
            continue;
        }
        let errno = if syscall == "ptrace" {
            "EPERM"
        } else {
            "ENOSYS"
        };
        warn!(
            event = "sandbox_denial",
            errno,
            syscall,
            syscall_nr = number,
            layer,
            task_id,
            "sandbox denied a syscall"
        );
    }
}

/// Emit the browser subprocess exit status as a structured event.
pub fn trace_browser_exit(status: Option<&ExitStatus>, layer: u8, task_id: &str) {
    let (exit_status, success) = match status {
        Some(status) => (status.to_string(), status.success()),
        None => ("unknown (wait failed)".to_string(), false),
    };
    info!(
        event = "browser_exit",
        exit_status = exit_status.as_str(),
        success,
        layer,
        task_id,
        "browser subprocess exited"
    );
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
    /// Sandbox prerequisites, resolved at build time and never mutated.
    sandbox: SandboxConfig,
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
            sandbox: SandboxConfig::resolve(),
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

/// Capped stdout, stderr and exit status of the child process.
type ChildOutput = (Vec<u8>, Vec<u8>, Option<ExitStatus>);

/// Shared child wiring: the handle is owned by the awaited future with
/// `kill_on_drop(true)`, so a timeout drops it and terminates the subprocess
/// instead of leaving an orphan (XP-S-05, #1608).
fn configure_child(mut command: Command) -> Command {
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command
}

impl ObscuraDownloader {
    /// Build the child command: wrap with `arapuca run` when the sandbox is
    /// available, otherwise emit a structured WARN and run obscura directly.
    fn spawn_command(&self, url_string: &str) -> Command {
        let mut inner = vec![
            "fetch".to_string(),
            "--dump".to_string(),
            "html".to_string(),
        ];
        // S1 (#2722 H5): the operator-pinned User-Agent travels to obscura's
        // argv; `None` passes no flag and obscura keeps its own default UA.
        if let Some(ua) = &self.user_agent {
            inner.push("--user-agent".to_string());
            inner.push(ua.clone());
        }
        inner.push(url_string.to_string());

        let binary = self.binary.to_string_lossy();
        if let Some(reason) = self.sandbox.skip_reason() {
            trace_sandbox_skipped(2, &self.sandbox.task_id, reason, &binary);
        } else if let Some(arapuca) = &self.sandbox.arapuca {
            // The mount must be the directory that holds the real executable.
            // A bare name (`obscura`, the default) has no parent, so resolve
            // it on PATH; an unresolved name skips the sandbox rather than
            // mounting the current directory.
            match resolve_executable(&self.binary) {
                Some(resolved) => {
                    trace_sandbox_applied(2, &self.sandbox.task_id, &binary);
                    let mut command = Command::new(arapuca);
                    command.args(arapuca_args(&resolved, None, &self.sandbox.task_id, &inner));
                    return configure_child(command);
                },
                None => trace_sandbox_skipped(
                    2,
                    &self.sandbox.task_id,
                    "obscura binary not found on PATH",
                    &binary,
                ),
            }
        }

        let mut command = Command::new(&self.binary);
        command.args(&inner);
        configure_child(command)
    }

    #[instrument(skip(self), fields(url = %url, binary = %self.binary.display()))]
    async fn fetch_inner(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        debug!(
            binary = %self.binary.display(),
            "Obscura fetch: {url}"
        );

        let url_string = url.to_string();

        // Defense in depth: a typed Url always carries a scheme (e.g. https://) so it can
        // never be mistaken for an obscura flag; reject defensively anyway.
        // LCOV_EXCL_START defensive: defense-in-depth — a typed Url always carries a scheme, never a leading '-'
        if url_string.starts_with('-') {
            return Err(DownloadError::Internal(
                "URL inválida: no puede comenzar con '-'".to_string(),
            ));
        }
        // LCOV_EXCL_STOP

        let child = self.spawn_command(&url_string).spawn().map_err(|e| {
            DownloadError::Internal(format!("obscura process failed to start: {e}"))
        })?;

        let max_page_bytes = self.max_page_bytes;
        let result = timeout(self.timeout, Self::collect_child(child, max_page_bytes)).await;
        if let Ok(Ok((_, err, status))) = &result {
            trace_sandbox_denials(&String::from_utf8_lossy(err), 2, &self.sandbox.task_id);
            trace_browser_exit(status.as_ref(), 2, &self.sandbox.task_id);
        }
        match result {
            Ok(Ok(output)) => self.build_page(output, url, max_page_bytes),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(DownloadError::timeout(self.timeout.as_secs())),
        }
    }

    /// Drain both pipes concurrently with `wait()` so a chatty child can
    /// never deadlock the pipes or inflate memory (fetch-guard stage 5).
    async fn collect_child(
        child: Child,
        max_page_bytes: u64,
    ) -> Result<ChildOutput, DownloadError> {
        let mut child = child;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DownloadError::Internal("obscura stdout pipe unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| DownloadError::Internal("obscura stderr pipe unavailable".into()))?;

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
    }

    /// Validate the child output and synthesize the page DTO.
    fn build_page(
        &self,
        (out_bytes, err_bytes, exit_status): ChildOutput,
        url: &Url,
        max_page_bytes: u64,
    ) -> Result<FetchedPage, DownloadError> {
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
        let final_url =
            parse_page_loaded_url(&String::from_utf8_lossy(&err_bytes)).unwrap_or_else(|| {
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

#[cfg(test)]
mod tests;
