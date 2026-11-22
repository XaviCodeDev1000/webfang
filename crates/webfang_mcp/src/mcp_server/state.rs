//! MCP Server State — shared state with per-category backpressure
//!
//! McpState embeds the application Container for DI and adds
//! tokio::sync::Semaphore instances to limit concurrent operations
//! per tool category, protecting the 8GB RAM / HDD hardware.

//! This module is a semaphore registry: every shared handle is an `Arc` clone
//! or an atomically-read field, and the only `Mutex`es are the metrics and
//! session-result slots whose guards are dropped before any await. This deny
//! fails the build if a future edit ever holds a `std` lock guard across an
//! `.await` in this module (#1616 CC-S1).
#![deny(clippy::await_holding_lock)]

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::mcp_server::metrics::{MetricsSnapshot, ScrapeEvent, ScrapeMetrics};
use crate::mcp_server::path_gate;
use rmcp::ErrorData as McpError;
use webfang_core::adapters::downloader::Downloader;
use webfang_core::di::Container;
use webfang_core::domain::crawler_port::RobotsPort;
use webfang_core::domain::DomInspectorPort;

mod limits;

pub use limits::{CategoryLimits, CategorySemaphores};

/// Default byte ceiling for the results ONE session retains across its crawl
/// run and the export that consumes them (#1611, F7).
///
/// The buffer it bounds is a plain `Vec<ScrapedContent>` that a crawl run
/// REPLACES wholesale, so before this ceiling a single `crawl_site` call with
/// `max_pages` at its advertised ceiling (100 000) could park ~100 GB of page
/// content in the process for as long as the session lived — the finding F7
/// names as "accumulated session results", and the one of its three sites that
/// neither a request-body cap nor a per-line cap reaches.
///
/// 64 MiB is [`MAX_BLOB_LEN`] × 64, i.e. 640× the whole of a page at the
/// crate's largest legal blob size, and — the property that matters for
/// compatibility — far above any DEFAULT run: `crawl_site` defaults to 100
/// pages, which retains well under a megabyte of extracted markdown for a
/// typical site. The ceiling is therefore reached by a deliberately oversized
/// run, which is exactly the traffic the bound exists to refuse, and it is
/// reported as an error naming the budget rather than silently truncated
/// (see `within_session_budget` in `handlers/scraping.rs`).
///
/// [`MAX_BLOB_LEN`]: crate::mcp_server::validation::MAX_BLOB_LEN
pub const DEFAULT_SESSION_RESULT_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Shared state for the MCP server.
///
/// Embeds the Container for dependency injection and provides
/// per-category semaphores for backpressure control.
//
// `Clone` is manual: every field stays process-shared across clones
// (container, limits, semaphores, metrics, …) EXCEPT `session_results`,
// which each clone forks: the clone gets its own buffer holding a copy of
// the origin's current contents, and the two diverge afterwards. The MCP
// transport builds one handler per client session via `state.clone()` from
// the pristine template, so sessions start empty and one session's later
// `crawl_site` run can never overwrite or leak into another session's
// export (R4-session-results-global). A new session starts with nothing to
// export — the honest empty-session error — until its own `crawl_site` runs.
pub struct McpState {
    /// Application DI container (single source of truth)
    pub container: Arc<Container>,
    /// Per-category concurrency limits
    pub limits: Arc<CategoryLimits>,
    /// Semaphores for each category
    pub semaphores: Arc<CategorySemaphores>,
    /// Shared Downloader for connection pooling across MCP tool calls
    pub downloader: Option<Arc<Downloader>>,
    /// DOM inspector for CSS selector diagnostics (None = no diagnostics)
    pub inspector: Option<Arc<dyn DomInspectorPort>>,
    /// Shared robots.txt fetcher for the scrape tools (#697). Construction
    /// failure is non-fatal: `None` leaves enforcement disabled, mirroring
    /// the rate-limiter pattern in the core container.
    pub robots_fetcher: Option<Arc<dyn RobotsPort>>,
    /// Process-lifetime scrape metrics, shared across all per-session clones
    /// (REQ-06). Locked only in short synchronous sections (REQ-07).
    pub metrics: Arc<Mutex<ScrapeMetrics>>,
    /// Session result slot for the last completed crawl run of THIS session
    /// (#1290).
    ///
    /// `crawl_site` replaces the buffer after every completed run (the same
    /// enriched DTO the CLI exports in memory: checksum, timestamp,
    /// `word_count`, `metadata_version` are produced downstream by the
    /// shared `process_results` path), and the export tools consume it
    /// instead of the legacy server persistence. Each MCP client session
    /// owns its slot: `Clone` forks the current contents into the clone's
    /// own buffer (sessions forked from the pristine template start empty),
    /// so one session's run can never overwrite or leak into another's
    /// export (R4-session-results-global). The repository read was re-pointed
    /// without carrying its restart durability into this slice.
    ///
    /// The export read copies the buffer inside `spawn_blocking` to preserve
    /// the #1122 executor anti-starvation contract. The lock is still only
    /// held for the synchronous copy inside that blocking task, never across
    /// an `.await` (REQ-07). The cheaper Arc-swap snapshot is deferred to
    /// slice 4.
    pub session_results: Arc<Mutex<Vec<webfang_core::domain::ScrapedContent>>>,
    /// Byte ceiling on [`Self::session_results`] (#1611, F7); see
    /// [`DEFAULT_SESSION_RESULT_BUDGET_BYTES`].
    ///
    /// A [`NonZeroUsize`], like every count/byte limit in this crate: a zero
    /// budget would mean "no result is ever exportable", which is a
    /// misconfiguration rather than a mode, and [`Self::with_session_result_budget`]
    /// is the only way to set it.
    ///
    /// Process-shared across clones, because the buffer is NOT: each session
    /// gets its own buffer and therefore spends its own budget.
    pub session_result_budget_bytes: NonZeroUsize,
    /// Allowed root directories for absolute `output_dir` and `checkpoint_dir`
    /// paths (#696, #1588).
    ///
    /// Empty (default) = absolute values are REJECTED (fail-closed). When
    /// non-empty, an absolute path must be under one of these roots AFTER
    /// symlink/junction resolution of both sides (canonical containment, not
    /// the old purely lexical prefix match).
    pub allowed_export_roots: Arc<Vec<PathBuf>>,
    /// Hermetic Obsidian detection overrides for tests (#726): `(scan_root,
    /// registry_path)`. When `Some`, vault detection is injected with these
    /// paths and never reads the host's real Obsidian registry. Production
    /// leaves this `None` (real detection chain).
    pub obsidian_hermetic: Option<(PathBuf, PathBuf)>,
    /// Cancellation token for graceful shutdown propagation.
    pub cancel_token: CancellationToken,
}

impl Clone for McpState {
    fn clone(&self) -> Self {
        Self {
            container: Arc::clone(&self.container),
            limits: Arc::clone(&self.limits),
            semaphores: Arc::clone(&self.semaphores),
            downloader: self.downloader.clone(),
            inspector: self.inspector.clone(),
            robots_fetcher: self.robots_fetcher.clone(),
            metrics: Arc::clone(&self.metrics),
            // Per-session isolation (R4-session-results-global): a clone
            // forks the current contents into its own buffer instead of
            // sharing the origin's Arc. Sessions forked from the pristine
            // template start empty; afterwards each session's runs diverge
            // — one session's `crawl_site` can never overwrite or leak into
            // another session's export. Everything else stays process-shared.
            session_results: Arc::new(Mutex::new(
                self.session_results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            )),
            session_result_budget_bytes: self.session_result_budget_bytes,
            allowed_export_roots: Arc::clone(&self.allowed_export_roots),
            obsidian_hermetic: self.obsidian_hermetic.clone(),
            cancel_token: self.cancel_token.clone(),
        }
    }
}

impl McpState {
    /// Create a new McpState with the given container and default limits.
    pub fn new(container: Container) -> Self {
        Self::from_parts(Arc::new(container), CategoryLimits::default())
    }

    /// Create with custom category limits.
    pub fn with_limits(container: Container, limits: CategoryLimits) -> Self {
        Self::from_parts(Arc::new(container), limits)
    }

    /// Share a pre-`Arc`'d container (#759).
    ///
    /// Unlike [`new`](Self::new)/[`with_limits`](Self::with_limits), which move
    /// an owned `Container` into a private `Arc`, this constructor adopts the
    /// given `Arc<Container>` as-is. The MCP binaries use it so a background
    /// task (lazy AI port wiring) and the server observe the SAME container —
    /// ports injected after construction become visible through the shared
    /// [`Container`] (lazy AI port wiring, #759).
    pub fn from_container(container: Arc<Container>) -> Self {
        Self::from_parts(container, CategoryLimits::default())
    }

    /// Shared construction for [`new`](Self::new) / [`with_limits`](Self::with_limits)
    /// / [`from_container`](Self::from_container): builds the semaphores,
    /// metrics accumulator, cancellation token, and the non-fatal robots.txt
    /// fetcher ([`build_robots_fetcher`]) from the given container and limits.
    fn from_parts(container: Arc<Container>, limits: CategoryLimits) -> Self {
        let limits = Arc::new(limits);
        let semaphores = Arc::new(CategorySemaphores::from_limits(&limits));
        let robots_fetcher = build_robots_fetcher(&container);
        Self {
            container,
            limits,
            semaphores,
            downloader: None,
            inspector: None,
            robots_fetcher,
            metrics: Arc::new(Mutex::new(ScrapeMetrics::default())),
            session_results: Arc::new(Mutex::new(Vec::new())),
            session_result_budget_bytes: NonZeroUsize::new(DEFAULT_SESSION_RESULT_BUDGET_BYTES)
                .unwrap_or_else(|| unreachable!("DEFAULT_SESSION_RESULT_BUDGET_BYTES is non-zero")),
            allowed_export_roots: Arc::new(Vec::new()),
            obsidian_hermetic: None,
            cancel_token: CancellationToken::new(),
        }
    }

    /// Trigger graceful shutdown by cancelling the token.
    ///
    /// All clones of this state share the same token (via `Arc` interior
    /// mutability), so a single call propagates to all holders.
    pub fn shutdown_signal(&self) {
        self.cancel_token.cancel();
    }

    /// Set a shared Downloader for connection pooling across tool calls.
    #[must_use]
    pub fn with_downloader(mut self, downloader: Arc<Downloader>) -> Self {
        self.downloader = Some(downloader);
        self
    }

    /// Set the byte ceiling for this session's retained crawl results
    /// (#1611, F7).
    ///
    /// Parity with [`with_downloader`](Self::with_downloader): composition
    /// roots that retain more than [`DEFAULT_SESSION_RESULT_BUDGET_BYTES`]
    /// of extracted content per run raise it here, and tests lower it to a
    /// few bytes so the refusal is reachable without allocating 64 MiB.
    ///
    /// A zero budget is refused by the type ([`NonZeroUsize`]), not clamped:
    /// "retain nothing" is never what an operator means by this knob.
    #[must_use]
    pub fn with_session_result_budget(mut self, budget: NonZeroUsize) -> Self {
        self.session_result_budget_bytes = budget;
        self
    }

    /// Set a shared robots.txt fetcher for the scrape tools (#697).
    ///
    /// Parity with [`with_downloader`](Self::with_downloader) and
    /// [`with_inspector`](Self::with_inspector): lets tests and composition
    /// roots wire a deterministic or pre-built fetcher.
    #[must_use]
    pub fn with_robots_fetcher(mut self, fetcher: Arc<dyn RobotsPort>) -> Self {
        self.robots_fetcher = Some(fetcher);
        self
    }

    /// Set a DOM inspector for CSS selector diagnostics.
    ///
    /// When set, failed selector extractions produce a `SelectorDiagnostic`
    /// with DOM structure analysis and closest-match suggestions.
    /// When `None` (default), diagnostics are `null` in the response.
    #[must_use]
    pub fn with_inspector(mut self, inspector: Arc<dyn DomInspectorPort>) -> Self {
        self.inspector = Some(inspector);
        self
    }

    /// Set the allowed root directories for absolute `output_dir` and
    /// `checkpoint_dir` paths (#696, #1588).
    ///
    /// When empty (default), absolute `output_dir`/`checkpoint_dir` values are
    /// rejected. When non-empty, an absolute path must resolve (symlinks
    /// included) under one of these roots. Relative paths are always allowed
    /// (they resolve against CWD).
    ///
    /// #769: when roots are configured, also checks the container's own
    /// `output_dir` — the write target of `process_export_pipeline`, which
    /// is gated against the same roots at request time (XP-P-08/G-9, issue
    /// #1608). If it is rooted (absolute, or a Windows-style rooted form on
    /// a host that does not consider it absolute) and outside every root, a
    /// tracing warning reports the inconsistency (the operator declared a
    /// boundary the server's own pipeline would violate). Deliberately
    /// warn-only at startup: the request-time gate is the enforcement.
    #[must_use]
    pub fn with_export_roots(mut self, roots: Vec<PathBuf>) -> Self {
        if !roots.is_empty() {
            warn_if_configured_output_dir_outside_roots(
                &self.container.config().output_dir,
                &roots,
            );
        }
        self.allowed_export_roots = Arc::new(roots);
        self
    }

    /// Inject hermetic Obsidian detection paths for tests (#726).
    ///
    /// When set, Obsidian vault detection runs through
    /// [`detect_vault_hermetic`](webfang_core::infrastructure::obsidian::vault_detector::detect_vault_hermetic)
    /// rooted at `root` with the registry read from `registry_path`, so tests
    /// never touch the host's real Obsidian registry. Production leaves this
    /// `None` and keeps the default detection chain.
    #[must_use]
    pub fn with_obsidian_hermetic(
        mut self,
        root: impl Into<PathBuf>,
        registry_path: impl Into<PathBuf>,
    ) -> Self {
        self.obsidian_hermetic = Some((root.into(), registry_path.into()));
        self
    }

    /// Validate that `dir` is an allowed export destination (#696, #1588).
    ///
    /// Delegates to the shared `path_gate::confine` gate: relative paths are
    /// always allowed (existing behavior — they resolve against the server's
    /// CWD). Absolute paths are resolved through the
    /// gate (symlinks/junctions canonicalized on both sides) and must be
    /// under one of [`allowed_export_roots`](Self::allowed_export_roots);
    /// when no roots are configured, absolute paths are rejected
    /// (fail-closed).
    ///
    /// # Errors
    /// Returns `McpError::invalid_params` when the path is empty/oversize,
    /// rooted non-absolute (`C:foo`, `\foo`), contains `..`, or — when
    /// absolute — no roots are configured or the resolved path lies outside
    /// every configured root.
    pub fn validate_export_dir(&self, dir: &Path) -> Result<(), McpError> {
        path_gate::confine(
            "output_dir",
            &dir.to_string_lossy(),
            self.allowed_export_roots.as_ref(),
        )
    }

    /// Validate that `dir` is an allowed checkpoint destination (#1588).
    ///
    /// `crawl_site`'s `checkpoint_dir` is a filesystem write target exactly
    /// like `output_dir`, so it runs through the SAME shared gate with the
    /// same configured roots: relative paths pass (server-CWD contract),
    /// absolute paths must resolve under
    /// [`allowed_export_roots`](Self::allowed_export_roots) (fail-closed with
    /// no roots).
    ///
    /// # Errors
    /// Same rejection set as [`validate_export_dir`](Self::validate_export_dir),
    /// reported for the `checkpoint_dir` field.
    pub fn validate_checkpoint_dir(&self, dir: &Path) -> Result<(), McpError> {
        path_gate::confine(
            "checkpoint_dir",
            &dir.to_string_lossy(),
            self.allowed_export_roots.as_ref(),
        )
    }

    /// Record a scrape event into the shared accumulator.
    ///
    /// REQ-07: short synchronous critical section — the lock is acquired and
    /// dropped entirely within this call (the tracing event emitted inside
    /// `record` is synchronous), so it is never held across an `.await`.
    /// REQ-10: a poisoned mutex is recovered via `into_inner`, never a panic.
    pub fn record_scrape(&self, event: ScrapeEvent) {
        self.metrics
            .lock()
            // LCOV_EXCL_LINE defensive: lock-poisoning — poison is recovered via into_inner, never a panic
            .unwrap_or_else(|e| e.into_inner())
            .record(event);
    }

    /// Shared robots.txt gate for tools that fetch content directly (#749).
    ///
    /// Delegates to the single policy source
    /// ([`enforce_robots_policy`](webfang_core::application::scraper_service::enforce_robots_policy)):
    /// fail-open when the fetcher is absent, `WafBlocked(url, "robots.txt")`
    /// on a genuine rules denial, `Network` carrying the guard's cause on a
    /// policy refusal (#1301 — never a phantom WAF). Callers receive the
    /// denial as an error payload and report it through their tool error
    /// envelope.
    pub async fn robots_denied_for(
        &self,
        url: &url::Url,
    ) -> Option<webfang_core::error::ScraperError> {
        let robots = self.robots_fetcher.as_deref();
        webfang_core::application::scraper_service::enforce_robots_policy(url, robots, false)
            .await
            .err()
    }

    /// Record a scrape event stamped with the tool call's run-root identity.
    ///
    /// The sole call-side helper for identified scrape metrics (#698): it owns
    /// the [`ScrapeEvent::identified`] construction so each MCP tool handler is
    /// a one-line record call. `start` is the operation's start instant and is
    /// converted to an elapsed [`Duration`](std::time::Duration) here, keeping
    /// timing bookkeeping in one place.
    pub fn record_scrape_identity(
        &self,
        tool: &'static str,
        domain: String,
        outcome: crate::mcp_server::metrics::Outcome,
        count: usize,
        start: std::time::Instant,
        correlation: &webfang_core::domain::CorrelationId,
    ) {
        let event =
            ScrapeEvent::identified(tool, domain, outcome, count, start.elapsed(), correlation);
        self.record_scrape(event);
    }

    /// Produce a point-in-time metrics snapshot from the shared accumulator.
    ///
    /// REQ-07: lock → clone snapshot → release. REQ-10: poison recovery, never
    /// a panic.
    #[must_use]
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.metrics
            .lock()
            // LCOV_EXCL_LINE defensive: lock-poisoning — poison is recovered via into_inner, never a panic
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }
}

/// Warn at startup when the container's configured `output_dir` lies outside
/// the declared export roots (#769).
///
/// `process_export_pipeline` exports to `container.scraper_config.output_dir`
/// (which now DOES go through [`McpState::validate_export_dir`] at request
/// time — XP-P-08/G-9, issue #1608), so this startup check is the early,
/// operator-facing signal for the same inconsistency.
///
/// A relative `output_dir` (the `ScraperConfig::default()` `"output"`) is
/// skipped: like [`McpState::validate_export_dir`], it resolves against the server's
/// CWD and the server's own boundary does not apply. The shape decision goes
/// through [`path_gate::classify`](crate::mcp_server::path_gate::classify)
/// instead of the old platform-blind `Path::is_absolute()` (G-8, issue
/// #1608): a configured `C:\exports` is not absolute on POSIX, and the #769
/// warn used to silently skip it — a rooted form the request gate would
/// reject must still surface at startup. Warn-only by design — #769
/// semantics.
///
/// Containment uses the shared #1588 helper
/// [`resolved_within_roots`](crate::mcp_server::path_gate::resolved_within_roots)
/// (symlinks resolved on both sides), so the startup check and the request
/// gate never disagree about the same path.
fn warn_if_configured_output_dir_outside_roots(output_dir: &Path, roots: &[PathBuf]) {
    if matches!(
        path_gate::classify(&output_dir.to_string_lossy()),
        path_gate::PathShape::Relative
    ) {
        return;
    }
    if path_gate::resolved_within_roots(output_dir, roots) {
        return;
    }
    tracing::warn!(
        output_dir = %output_dir.display(),
        roots = ?roots,
        "configured output_dir is outside the configured export roots: the server's own \
         export pipeline (process_export_pipeline) writes there without going through the \
         #696 export-root gate, violating the boundary the operator declared"
    );
}

/// Build the shared robots.txt fetcher for the scrape tools (#697).
///
/// Single graph (#1149): built through the `Container` default robots seam
/// (same `Chrome145` fingerprint + cache policy as every other robots
/// fetcher), so the MCP crate no longer names the infrastructure concrete.
/// Uses the container's configured download timeout. Construction failure is
/// NON-FATAL — the fetcher stays `None` and robots enforcement degrades to
/// "no fetcher available", mirroring the rate-limiter pattern in the core
/// container.
fn build_robots_fetcher(container: &Container) -> Option<Arc<dyn RobotsPort>> {
    match Container::build_default_robots_fetcher(container.config().download_timeout_secs) {
        Ok(fetcher) => Some(fetcher),
        Err(e) => {
            tracing::warn!(error = %e, "robots_fetcher_init_failed_non_fatal");
            None
        },
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "state_export_roots_tests.rs"]
mod export_roots_tests;
