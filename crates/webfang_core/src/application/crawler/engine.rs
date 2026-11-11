//! Engine module — Crawl orchestration with JoinSet-based concurrency
//!
//! The Engine manages the crawl loop, spawning tasks via JoinSet
//! with backpressure and rate limiting. Each task fetches a URL,
//! extracts links, and pushes discovered URLs to the queue.
//!
//! # D6 lock-across-await audit (task 2.3, change stabilization-concurrency-budget)
//!
//! Functions rewired by commit f5114cd6 (rate-limiter construction):
//!
//! | Function | `.await` points | Guard discipline | Verdict |
//! |---|---|---|---|
//! | `rate_limiter_config` | none (sync fn) | no lock guards touched | PASS |
//! | `Engine::with_budget` | none (sync ctor) | `SharedRateLimiter::new` / `CrawlScheduler::new` consume args by value; no `std`/`RwLock`/`Mutex`/`DashMap` guard outlives its expression | PASS |
//!
//! Enforcement: `#![deny(clippy::await_holding_lock)]` below fails the build if
//! a future edit ever holds a `std` lock guard across an `.await` in this module.

#![deny(clippy::await_holding_lock)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::RwLock as AsyncRwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, Instrument};
use wreq_util::Profile;

use super::checkpoint::{BannedDomain, BincodeCheckpoint, CrawlCheckpoint};
use super::collector::ResultsCollector;
use super::concurrency_level::{ConcurrencyLevel, SharedConcurrencyLevel};
use super::content_sink::CrawlContentSink;
use super::crawl_scheduler::CrawlScheduler;
use super::session::CrawlSession;
use crate::application::crawler::ports::TerminationSource;
use crate::application::pipeline::PipelineExecutor;
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::budget::BudgetModel;
use crate::domain::cookie_bridge::CookieBridge;
use crate::domain::crawler_port::RobotsPort;
use crate::domain::downloader_factory::{DownloaderFactory, DownloaderSpec};
use crate::domain::downloader_port::{DownloadError, Downloader};
use crate::domain::ram_probe_port::RamProbePort;
use crate::domain::{post_load_wait::PostLoadWait, CorrelationId, CrawlerConfig, JsStrategy};

mod options;
pub use options::EngineOptions;

mod pause;
pub use pause::{PauseTracker, PauseVerdict, PauseWatchdog, WaitOutcome};

mod checkpoint;

mod lifecycle;

mod crawl_loop;

mod entry;
pub use entry::crawl_site_with_options;
#[allow(deprecated)]
pub use entry::{crawl_site, crawl_site_capturing};

mod build;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_autoscale;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_budget;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_checkpoint;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_common;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_pause;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_pool;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_span;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_stalled;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_termination;

/// Shared shutdown signal — set to `true` when SIGINT/SIGTERM received.
type ShutdownSignal = Arc<AtomicBool>;

/// What a termination-source wait means for the run's shutdown (#1835).
///
/// ONE decision, TWO authorities: the crawl engine ([`Engine`]) and the CLI
/// shutdown guard ([`crate::cli::shutdown`]) both map the wait's outcome
/// through this type, so a REJECTED registration cannot mean "stop the run"
/// in one site and "keep going" in the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminationOutcome {
    /// A termination event arrived — the run drains.
    Event(&'static str),
    /// The OS rejected the registration — the process has NO termination
    /// source at all. The run keeps going and waits for an explicit cancel
    /// (#509).
    RegistrationRejected,
}

impl TerminationOutcome {
    /// Classify the result of a termination-source wait.
    ///
    /// `None` means the OS refused the handler — never "nothing happened"
    /// (the same meaning the termination port gives it, #509).
    #[must_use]
    pub(crate) fn from_wait(result: Option<&'static str>) -> Self {
        match result {
            Some(name) => Self::Event(name),
            None => Self::RegistrationRejected,
        }
    }

    /// Whether this outcome must stop the run.
    ///
    /// A REJECTED registration does NOT: the process has no OS termination
    /// source, and stopping on a transient registration failure would
    /// silently end an unattended run (#1835). An explicit cancel still
    /// stops it.
    #[must_use]
    pub(crate) fn stops_run(self) -> bool {
        matches!(self, Self::Event(_))
    }
}

/// The single warning EVERY shutdown authority emits when the OS rejects the
/// termination-source registration (#1835, #2193).
///
/// One definition, THREE call sites: the crawl engine ([`Engine`]), the CLI
/// shutdown guard ([`crate::cli::shutdown`]) and the MCP server answer a
/// rejected registration the SAME way (the shared `TerminationOutcome`
/// decision), so they must also SAY the same thing — a shared string has to
/// describe shared behaviour.
///
/// `pub` because the third site lives in ANOTHER crate: `webfang_mcp` cannot
/// reach a `pub(crate)` item of `webfang_core`, so while this constant was
/// crate-private the MCP server kept a wording of its own for the same fact —
/// three texts for one event, and no way for a reader to tell that the other
/// two were saying the same thing (#2193).
pub const TERMINATION_REGISTRATION_REJECTED: &str =
    "interrupt handler registration failed — graceful shutdown will only respond to an explicit cancel";

/// Grace added to the longest legitimate worker wait when bounding joins (#509).
///
/// The longest legitimate wait is one fetch — a connect phase plus a request
/// phase, each carrying its own timeout — or a rate-limit token
/// (`config.delay_ms`). Anything beyond that plus this grace is treated as a
/// hung worker.
const SHUTDOWN_GRACE_SECS: u64 = 10;

/// Drain grace after a FAILED loop (#2338).
///
/// A failed loop already ran `cancel_run()` (#2313) and carries its reason:
/// responsive workers are unwinding and finish within this grace, and the
/// wedged one was already declared hung — draining it again at the full
/// [`worker_wait_bound`] doubles the failure latency for zero information.
/// The failure is reported at detection + this grace, not detection + bound.
const FAILED_RUN_DRAIN_GRACE: Duration = Duration::from_secs(1);

/// Timeout-carrying phases a single worker pays before it can finish (#2313).
///
/// A fetch is not one timeout: the connect has its own cap (10s in
/// `create_rate_limited_client`) and the request has `timeout_secs` on top. The
/// bound must cover the SUM, not the larger of the two — measured on
/// `test_crawl_with_sitemap_response_excludes_external_and_forbidden_urls`, an
/// unreachable host took 47s end to end with a 30s request timeout, and a bound
/// of `30 + grace` aborted the whole crawl over one slow address.
const FETCH_TIMEOUT_PHASES: u64 = 2;

/// Crawl engine — orchestrates URL fetching with concurrency control
///
/// Uses `JoinSet` for task management (no redundant Semaphore).
/// Rate limiting via `SharedRateLimiter`. Deduplication via lock-free
/// `UrlDeduplicator`. Results collected via mpsc channel.
pub struct Engine {
    config: Arc<CrawlerConfig>,
    /// Root correlation ID for this crawl — all pages share its `trace_id`.
    correlation_id: CorrelationId,
    collector: ResultsCollector,
    /// Scheduling policy — visited dedup, pending-work buffer, concurrency limits.
    scheduler: CrawlScheduler,
    rate_limiter: SharedRateLimiter,
    error_count: Arc<AtomicUsize>,
    /// Per-category error counters indexed by `CrawlErrorCategory::index()` (issue #374).
    error_breakdown: Arc<[AtomicUsize; 8]>,
    /// Checkpoint store for persistence (stateless — always available).
    checkpoint_store: BincodeCheckpoint,
    /// Loaded checkpoint state for crash recovery (`None` = fresh start).
    checkpoint_state: Option<CrawlCheckpoint>,
    /// Path to save checkpoint files.
    checkpoint_path: Option<PathBuf>,
    /// Pages between automatic checkpoint saves (0 = disabled).
    checkpoint_interval: u64,
    /// Shared robots.txt port for the crawl session (TLS-fingerprinted, #337).
    /// The concrete `RobotsFetcher` is built by the composition-root helper
    /// `application::container::build_robots_fetcher` (ADR-0012-B post-narrow).
    robots_fetcher: Arc<dyn RobotsPort>,
    /// Atomic counter for total pages crawled (used by checkpoint and signal handler).
    pages_crawled: Arc<AtomicU64>,
    /// Shared shutdown signal for graceful termination.
    shutdown: ShutdownSignal,
    /// Engine-wide cancellation token (#509) — fired on shutdown so workers
    /// blocked on rate-limit or resource-governor waits abort promptly.
    cancel_token: CancellationToken,
    /// JavaScript rendering strategy.
    js_strategy: JsStrategy,
    /// Optional fetch downloader for hybrid/full JS rendering.
    ///
    /// Built through the injected [`DownloaderFactory`] — never constructed
    /// here. `None` means "no factory was injected", which is a fully
    /// supported state: [`ports::ProductionPageFetcher`](crate::application::crawler::ports::ProductionPageFetcher) then falls back to
    /// the composition-root-injected [`StaticFetchPort`](crate::domain::crawler_port::StaticFetchPort),
    /// whose infrastructure implementation delegates to the static `fetch_url`
    /// free fn.
    fetch_router: Option<Arc<dyn Downloader>>,
    /// Factory that builds [`Self::fetch_router`]. `None` disables the
    /// dynamic fetch path for this engine.
    downloader_factory: Option<Arc<dyn DownloaderFactory>>,
    /// Cookie bridge for extracting and injecting cookies.
    cookie_bridge: Arc<AsyncRwLock<CookieBridge>>,
    /// Domains currently banned due to WAF or rate limiting.
    banned_domains: Arc<RwLock<Vec<BannedDomain>>>,
    /// Optional sink capturing every fetched page body (#631).
    content_sink: Option<Arc<dyn CrawlContentSink>>,
    /// Optional item pipeline for processing scraped content.
    pipeline: Option<Arc<PipelineExecutor>>,
    /// Run owner (P6-2): the validated session this engine was built from.
    /// `Engine::from_session` is the only constructor, so an engine always
    /// owns its session until [`Engine::run`] consumes it to derive the task
    /// context and render the close verdict (slice 4 — the direct-
    /// construction state is gone).
    session: Option<CrawlSession>,
    /// Optional handle for the signal handler task — aborted on shutdown
    /// to prevent the tokio runtime from hanging waiting for it.
    signal_handle: Option<tokio::task::JoinHandle<()>>,
    /// Optional handle for the RAM autoscale probe task (#1941) — aborted on
    /// shutdown for the same reason as `signal_handle`.
    ///
    /// The autoscale loop never terminates on its own (it ticks every 5s for
    /// the whole run), so discarding this handle left a live task that nothing
    /// owned and nothing stopped: it kept polling `ram_probe` after the crawl
    /// had finished.
    autoscale_handle: Option<tokio::task::JoinHandle<()>>,
    /// Immutable budget snapshot built once at entry; every derived tier
    /// (burst, crawl, domain) reads from it.
    budget: BudgetModel,
    /// System RAM-usage probe — domain port read by the autoscale loop
    /// (`with_autoscale`) to throttle crawl permits under memory pressure.
    /// Defaults to the sysinfo-backed production impl; tests inject a fake.
    /// ADR-0012 sub-slice 3.B-1c.
    ram_probe: Arc<dyn RamProbePort>,
    /// Process-level termination source this run drains on (#1808).
    ///
    /// Injected, never named: the concrete source (the Win32 console-event
    /// fan-out) is infrastructure, and `application` may only reach inward, so
    /// the composition root builds it
    /// (`application::container::build_termination_source`) and it arrives
    /// here through the [`TerminationSource`] port. Defaults to Ctrl+C only,
    /// so a caller that asks for no platform plumbing keeps today's
    /// semantics.
    termination_source: TerminationSource,
}

/// Rate-limiter burst source derived from the budget model (design D4/D1):
/// the burst is an INDEPENDENT tier — it must never re-read
/// `CrawlerConfig.concurrency`, so raising the crawler concurrency leaves
/// the token-bucket burst unchanged.
fn rate_limiter_config(config: &CrawlerConfig, budget: &BudgetModel) -> RateLimiterConfig {
    RateLimiterConfig::new(config.delay_ms, budget.burst().get())
}

impl Engine {
    /// Inject the factory that builds the fetch downloader.
    ///
    /// Must be called before [`Self::with_js_strategy`], which is where the
    /// factory is actually invoked. Without it `with_js_strategy` records the
    /// strategy but leaves the downloader unset, and the crawl falls back to
    /// the static `fetch_url()` path — hybrid/full rendering silently
    /// degrades to static. Callers that need JS rendering own this wiring:
    /// `application` cannot name a concrete factory without re-introducing
    /// the `application → infrastructure` edge this port removes, so the
    /// composition happens in the gate-exempt `cli` layer.
    #[must_use]
    pub fn with_downloader_factory(mut self, factory: Arc<dyn DownloaderFactory>) -> Self {
        self.downloader_factory = Some(factory);
        self
    }

    /// Set the JavaScript rendering strategy.
    ///
    /// `tls_emulation` is the TLS/HTTP2 fingerprint profile applied to the wreq
    /// layer of the fetch router. `ignore_waf` bypasses WAF classification on
    /// the hybrid spa-detection path (REQ-WAF-07). `obscura_binary` is the
    /// Hybrid Layer 2 binary — a path is invoked as given, a bare name is
    /// resolved from `PATH` (#787).
    ///
    /// The downloader itself is built by the factory injected through
    /// [`Self::with_downloader_factory`]. When no factory was injected the
    /// strategy is still recorded but no downloader is built: there is no
    /// built-in fallback, because building one would mean `application`
    /// constructing infrastructure concretes again.
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Internal`] if the wreq client cannot be built.
    // 10 params: the strategy's full dependency set (profile, WAF, retry
    // backoff, obscura binary, resolved chrome binary, post-load wait).
    // Bundling them would only move the same
    // wiring one level up (same pattern as DownloaderSpec).
    #[allow(clippy::too_many_arguments)]
    pub fn with_js_strategy(
        mut self,
        strategy: JsStrategy,
        tls_emulation: Profile,
        ignore_waf: bool,
        max_retries: u32,
        backoff_base_ms: u64,
        backoff_max_ms: u64,
        obscura_binary: String,
        post_load_wait: PostLoadWait,
        chrome_binary: Option<PathBuf>,
    ) -> Result<Self, DownloadError> {
        let timeout = self.config.timeout_secs;
        // No factory injected => no downloader built. `None` is a supported
        // state: ProductionPageFetcher falls back to the static fetch_url().
        let router = self
            .downloader_factory
            .as_ref()
            .map(|factory| {
                factory.build(
                    &DownloaderSpec {
                        strategy,
                        timeout_secs: timeout,
                        tls_emulation,
                        ignore_waf,
                        // #503: the Engine path keeps today's rotating default
                        // behavior — `CrawlerConfig.user_agent` is a separate
                        // dead field, out of scope.
                        user_agent: None,
                        // #890: operator headers/cookies are wired on the scrape
                        // path (cli/scrape_flow.rs). The Engine path keeps
                        // profile-default behavior — same out-of-scope precedent
                        // as the UA above.
                        custom_headers: Vec::new(),
                        accept_language: None,
                        initial_cookie_jar: None,
                        max_retries,
                        backoff_base_ms,
                        backoff_max_ms,
                        obscura_binary,
                        // F-52-b: wait mode for the chromium path.
                        post_load_wait,
                        // F-52-c: the gate-certified binary (or None = auto-detect).
                        chrome_binary,
                        // FIX-1 (#1231 F-12): the historical 50 MiB cap.
                        // Plumbing an engine-side operator flag for it is a
                        // follow-up (CrawlerConfig change, gate3-pinned).
                        max_page_bytes: Some(
                            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
                        ),
                    },
                    // #509: the Full strategy's governor shares the engine token
                    // so permit waits abort on shutdown.
                    Arc::clone(&self.cookie_bridge),
                    self.cancel_token.clone(),
                )
            })
            .transpose()?;
        self.js_strategy = strategy;
        self.fetch_router = router;
        Ok(self)
    }

    /// Enable autoscaled concurrency based on system RAM.
    ///
    /// Spawns a background task that polls the injected [`RamProbePort`]
    /// every 5 seconds and adjusts the shared concurrency level accordingly.
    /// The engine's spawn loop reads this level to compute effective concurrency.
    ///
    /// The loop never ends on its own, so its [`tokio::task::JoinHandle`] is kept on the
    /// engine and aborted in [`Self::shutdown`] — same lifecycle as
    /// `signal_handle` (#1941).
    // Supervised: the JoinHandle is stored in `self.autoscale_handle` and
    // aborted in `Self::shutdown` (#1941 lifecycle).
    #[allow(clippy::disallowed_methods)]
    pub fn with_autoscale(mut self) -> Self {
        let level = Arc::new(SharedConcurrencyLevel::new());
        let level_clone = Arc::clone(&level);
        let probe = Arc::clone(&self.ram_probe);

        self.autoscale_handle = Some(
            tokio::spawn(
                async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(5));
                    interval.tick().await; // skip first immediate tick
                    loop {
                        interval.tick().await;
                        let usage = probe.ram_usage_percent().as_percent();
                        let new_level = if usage >= f32::from(crate::domain::budget::derivation::RamThresholds::DEFAULT_CRITICAL_PERCENT) {
                        ConcurrencyLevel::Critical
                    } else if usage >= f32::from(crate::domain::budget::derivation::RamThresholds::DEFAULT_WARNING_PERCENT) {
                        ConcurrencyLevel::Reduced
                    } else {
                        ConcurrencyLevel::Normal
                    };
                        if level_clone.get() != new_level {
                            info!(
                                "Autoscale: RAM {usage:.2}% → concurrency level {:?}",
                                new_level
                            );
                            level_clone.set(new_level);
                        }
                    }
                }
                .in_current_span(),
            ),
        );

        self.scheduler.set_autoscale(level);
        self
    }

    /// Override the RAM-usage probe used by [`Self::with_autoscale`].
    ///
    /// Tests inject a deterministic fake so the autoscale loop's threshold
    /// branches can be exercised without real sysinfo reads. Production
    /// code can leave the default
    /// ([`SystemRamProbe`](crate::domain::ram_probe_port::SystemRamProbe), built
    /// by [`system_default`](crate::domain::ram_probe_port::system_default)) in
    /// place.
    #[must_use]
    pub fn with_ram_probe(mut self, probe: Arc<dyn RamProbePort>) -> Self {
        self.ram_probe = probe;
        self
    }

    /// Restore banned domains from a checkpoint.
    pub fn with_banned_domains(self, domains: Vec<BannedDomain>) -> Self {
        if let Ok(mut banned) = self.banned_domains.write() {
            *banned = domains;
        }
        self
    }

    /// Capture every fetched page body into `sink` (#631).
    ///
    /// [`crate::domain::CrawlResult`] is metadata only; without a sink the crawl discards the
    /// bodies after link extraction and callers (batch mode) have nothing to
    /// export. The sink is shared, so several engines can feed one collection.
    #[must_use]
    pub fn with_content_sink(mut self, sink: Arc<dyn CrawlContentSink>) -> Self {
        self.content_sink = Some(sink);
        self
    }

    /// Set the item pipeline executor for processing scraped content.
    pub fn with_pipeline(mut self, executor: PipelineExecutor) -> Self {
        self.pipeline = Some(Arc::new(executor));
        self
    }

    /// Override the process-level termination source this run drains on
    /// (#1808, XP-S-02).
    ///
    /// Tests inject a source that fires on demand so the drain path is
    /// exercised without delivering an OS signal. Production wiring injects
    /// the platform source through the composition root
    /// ([`Engine::from_session`]); the default stays Ctrl+C only, which is
    /// why every construction site that never calls this keeps compiling and
    /// keeps today's semantics.
    pub(crate) fn with_termination_source(mut self, source: TerminationSource) -> Self {
        self.termination_source = source;
        self
    }
}
