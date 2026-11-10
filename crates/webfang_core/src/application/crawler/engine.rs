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
mod tests_checkpoint;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_pause;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_pool;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_stalled;

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

#[cfg(test)]
#[cfg(not(miri))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests {
    /// #1615 (F11 / G-3 / G-4 / G-5): these tests drive real fetches against
    /// wiremock's 127.0.0.1, which the literal-IP entry guard now refuses.
    /// `EnvGuard::entry_guard_off()` disarms the guard for its lifetime — the
    /// named constructor is the repo's rule, so each call site stays one line.
    pub(super) fn entry_off() -> webfang_test_utils::EnvGuard {
        webfang_test_utils::EnvGuard::entry_guard_off()
    }
    /// Test helper: non-zero literal for `CrawlerConfig::concurrency` (#1132).
    pub(super) fn nz(n: usize) -> std::num::NonZeroUsize {
        std::num::NonZeroUsize::new(n).expect("test literal is non-zero")
    }

    /// Watchdog for the loop-level tests: thresholds far beyond any test
    /// duration, so only an injected millisecond watchdog decides verdicts.
    /// Run-time values live on `CrawlerConfig` (#2338).
    pub(super) const TEST_WATCHDOG: PauseWatchdog = PauseWatchdog {
        alert_after: Duration::from_secs(300),
        give_up_after: Duration::from_secs(1800),
    };

    /// Test helper: a session-built engine — the only construction path
    /// since P6-2 slice 4. Mirrors the former direct `(config, true)`
    /// construction exactly: robots ignored via the transport, static
    /// strategy, default transport knobs, no injected ports.
    pub(super) fn session_engine(config: CrawlerConfig) -> Engine {
        Engine::from_session(pool_session(config, None, false, None)).expect("engine must build")
    }

    use super::*;
    use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
    use crate::application::crawler::session::CrawlExec;
    use crate::domain::budget::detector::FixedDetector;
    use crate::domain::budget::tiers::{BurstPermits, CrawlConcurrency};
    use crate::domain::budget::{BudgetModel, BudgetOverrides};
    use crate::domain::downloader_factory::DEFAULT_OBSCURA_BINARY;
    use crate::domain::persistence::PersistenceMode;
    use crate::domain::session_port::SessionPort;
    use crate::domain::CrawlError;
    use tracing_subscriber::layer::SubscriberExt;
    use url::Url;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Spec scenario (Q1 DECOUPLE): raising the configured crawler
    /// concurrency must NOT move the rate-limiter burst — the burst is a
    /// budget-model tier derived from the detector seam, independent of
    /// `CrawlerConfig.concurrency`.
    #[tokio::test]
    async fn raising_crawler_concurrency_leaves_rate_limiter_burst_unchanged() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let low = CrawlerConfig::builder(seed.clone())
            .concurrency(nz(1))
            .build();
        let high = CrawlerConfig::builder(seed).concurrency(nz(16)).build();

        let engine_low = session_engine(low);
        let engine_high = session_engine(high);

        assert_eq!(
            engine_low.budget.burst().get(),
            engine_high.budget.burst().get(),
            "burst must not track crawler concurrency"
        );
        assert!(engine_high.budget.burst().get() > 0);
    }

    /// The RateLimiterConfig construction point derives its burst from the
    /// model: with a FixedDetector of 4 cores the auto table yields crawl
    /// (= default burst) 3, regardless of any crawler concurrency value.
    #[test]
    fn rate_limiter_config_burst_comes_from_budget_model() {
        let detector =
            FixedDetector::with_detection(std::num::NonZeroUsize::new(4).expect("non-zero"), None);
        let budget = BudgetModel::build(BudgetOverrides::default(), &detector);
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).concurrency(nz(16)).build();

        let rl_config = rate_limiter_config(&config, &budget);
        assert_eq!(rl_config.delay_ms, config.delay_ms);
        assert_eq!(
            rl_config.concurrency,
            budget.burst().get(),
            "burst must come from the model, not config.concurrency"
        );
        assert_eq!(rl_config.concurrency, 3, "4-core auto table value");
    }

    /// Triangulation sweep: across the whole 1..=32 core range the
    /// construction point emits exactly the model's burst tier, whatever
    /// the configured crawler concurrency claims.
    #[test]
    fn rate_limiter_burst_sweep_matches_model_across_core_counts() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        for cores in 1..=32usize {
            let detector = FixedDetector::with_detection(
                std::num::NonZeroUsize::new(cores).expect("cores non-zero"),
                None,
            );
            let budget = BudgetModel::build(BudgetOverrides::default(), &detector);
            for concurrency in [1usize, 8, 16] {
                let config = CrawlerConfig::builder(seed.clone())
                    .concurrency(nz(concurrency))
                    .build();
                assert_eq!(
                    rate_limiter_config(&config, &budget).concurrency,
                    budget.burst().get(),
                    "burst diverged from model at cores={cores} concurrency={concurrency}"
                );
            }
        }
    }

    /// Bug R2-1: an explicit operator override carried on `CrawlerConfig`
    /// must reach the Engine — `build_machinery` (consumed by
    /// `from_session`) feeds `config.budget_overrides`
    /// into `BudgetModel::build`, so the scheduler spawn bound (Operation.crawl)
    /// and rate-limiter burst reflect the override instead of the auto table.
    #[tokio::test]
    async fn session_engine_consumes_config_budget_overrides() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let overrides = BudgetOverrides {
            crawl: CrawlConcurrency::new(7).ok(),
            rate_burst: BurstPermits::new(9).ok(),
            ..BudgetOverrides::default()
        };
        let config = CrawlerConfig::builder(seed)
            .concurrency(nz(1))
            .budget_overrides(overrides)
            .build();

        let engine = session_engine(config);

        assert_eq!(
            engine.budget.crawl().get(),
            7,
            "explicit --concurrency must reach the Engine scheduler bound"
        );
        assert_eq!(
            engine.budget.burst().get(),
            9,
            "explicit --rate-limit-burst must reach the Engine rate limiter"
        );
    }

    /// Triangulation: with default (no-op) overrides the Engine reproduces
    /// exactly the model derived from those same default overrides through
    /// the canonical detector seam — the channel changes nothing when unset.
    #[tokio::test]
    async fn session_engine_default_overrides_keep_auto_derivation() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let expected = BudgetModel::build(
            BudgetOverrides::default(),
            &crate::domain::budget::detector::SystemDetector,
        );
        let config = CrawlerConfig::builder(seed).concurrency(nz(16)).build();

        let engine = session_engine(config);

        assert_eq!(
            engine.budget.crawl().get(),
            expected.crawl().get(),
            "default overrides must keep the auto-derived crawl tier"
        );
        assert_eq!(
            engine.budget.burst().get(),
            expected.burst().get(),
            "default overrides must keep the auto-derived burst tier"
        );
    }

    // ——— ADR-0012-B sub-slice 3.F: session pool is the domain port (#1075) ———

    /// In-crate fake port with per-instance counters — proves the SAME
    /// trait object reaches the spawned workers (ban/cooldown state must be
    /// visible to every consumer, #1075; wiring fix coverage, #1291).
    #[derive(Debug, Default)]
    pub(super) struct FakeSessionPort {
        acquires: std::sync::atomic::AtomicUsize,
        successes: std::sync::atomic::AtomicUsize,
    }

    impl FakeSessionPort {
        pub(super) fn acquire_count(&self) -> usize {
            self.acquires.load(std::sync::atomic::Ordering::SeqCst)
        }
        pub(super) fn success_count(&self) -> usize {
            self.successes.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl SessionPort for FakeSessionPort {
        fn acquire(&self, _domain: &str) -> Option<crate::domain::session_port::SessionId> {
            self.acquires
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(crate::domain::session_port::SessionId(0))
        }
        fn report_success(&self, _domain: &str, _session: crate::domain::session_port::SessionId) {
            self.successes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn report_failure(
            &self,
            _domain: &str,
            _session: crate::domain::session_port::SessionId,
            _status: u16,
        ) {
        }
    }

    /// Test helper: a validated session for pool-wiring proofs — static
    /// strategy, robots ignored (mirrors the pre-session direct constructor
    /// the 3.F tests used). `pool` injects through the ports, the single
    /// production injection point since the #1291 wiring fix.
    pub(super) fn pool_session(
        config: CrawlerConfig,
        pool: Option<Arc<FakeSessionPort>>,
        session_pool_enabled: bool,
        pipeline: Option<Arc<PipelineExecutor>>,
    ) -> CrawlSession {
        // #1439: identity is a REQUIRED builder part (no silent mint), so
        // the pool-wiring proof supplies its own standalone root. The label
        // is read BEFORE `.config(...)` moves the config into the builder.
        let run_label = config.seed_url.host_str().unwrap_or("seed").to_string();
        CrawlSession::builder()
            .config(config)
            .persistence(PersistenceMode::Disabled)
            .transport(crate::application::crawler::session::TransportPolicy {
                js_strategy: JsStrategy::Static,
                tls_emulation: Profile::Chrome145,
                ignore_waf: false,
                max_retries: 3,
                backoff_base_ms: 1000,
                backoff_max_ms: 10000,
                obscura_binary: DEFAULT_OBSCURA_BINARY.to_string(),
                chrome_binary: None,
                post_load_wait: PostLoadWait::Idle,
                session_pool_enabled,
                autoscale_enabled: false,
                ignore_robots: true,
            })
            .ports(crate::application::crawler::session::CrawlPorts {
                session_pool: pool.map(|p| p as Arc<dyn SessionPort>),
                downloader_factory: None,
                content_sink: None,
                pipeline,
            })
            .identity(crate::application::crawler::session::CrawlIdentity {
                root: CorrelationId::new(),
                run_label,
            })
            .build()
            .expect("test session must build")
    }

    /// ADR-0012 sub-slice 3.B-1c — the autoscale loop MUST read RAM via the
    /// injected [`RamProbePort`], not via a hardcoded `ResourceGovernor` static
    /// call. This test injects a high-pressure reading, runs the autoscale
    /// background task, and asserts the shared concurrency level reaches
    /// `Critical` without ever touching real sysinfo.
    #[tokio::test]
    async fn with_autoscale_uses_injected_ram_probe() {
        #[derive(Debug)]
        struct HighPressureProbe;
        impl crate::domain::ram_probe_port::RamProbePort for HighPressureProbe {
            fn ram_usage_percent(&self) -> crate::domain::ram_probe_port::RamUsagePercent {
                crate::domain::ram_probe_port::RamUsagePercent::new_clamped(95.0)
            }
        }
        impl crate::domain::ram_probe_port::Sealed for HighPressureProbe {}

        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).build();
        let engine = session_engine(config)
            .with_ram_probe(Arc::new(HighPressureProbe))
            .with_autoscale();

        // The probe returns 95% on every poll. The autoscale background loop
        // ticks at 5s; we wait long enough for at least one post-skip tick
        // to land, then verify the shared level moved to Critical. The
        // probe is `Arc<dyn RamProbePort>` so it MUST be the one polled —
        // not the production `SystemRamProbe`.
        let level = engine
            .scheduler
            .autoscale_level()
            .expect("with_autoscale must install a level")
            .clone();
        // Sleep slightly longer than one 5s tick so the spawn'd loop runs
        // at least once past the initial skip-tick.
        tokio::time::sleep(Duration::from_millis(5_100)).await;
        assert_ne!(
            level.get(),
            crate::application::crawler::concurrency_level::ConcurrencyLevel::Normal,
            "autoscale loop must have moved off Normal under 95% injected RAM pressure",
        );
        assert_eq!(
            level.get(),
            crate::application::crawler::concurrency_level::ConcurrencyLevel::Critical,
            "autoscale loop must reach Critical at 95% (>= 90% threshold)",
        );

        // End the 5s poll loop this test started (#1941) instead of leaking it
        // into the rest of the suite.
        engine.shutdown().await;
    }

    /// #1941: the RAM autoscale loop is an unbounded poll task (it loops on a
    /// 5s tick forever). `with_autoscale` used to `tokio::spawn` it and drop
    /// the `JoinHandle`, so nothing owned it — not the engine, not shutdown —
    /// and the task outlived the crawl that created it, keeping the runtime
    /// alive and polling a probe no one reads. The engine must hold the handle
    /// and end it in `shutdown`, exactly like `signal_handle`.
    #[tokio::test]
    async fn autoscale_probe_handle_is_owned_and_aborted_by_shutdown() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).build();
        let engine = session_engine(config)
            .with_ram_probe(Arc::new(CountingProbe(Arc::new(
                std::sync::atomic::AtomicUsize::new(0),
            ))))
            .with_autoscale();

        let abort = engine
            .autoscale_handle
            .as_ref()
            .map(tokio::task::JoinHandle::abort_handle)
            .expect("with_autoscale must record the spawned probe handle");
        assert!(
            !abort.is_finished(),
            "the probe task must be live before shutdown, else this proves nothing"
        );

        engine.shutdown().await;

        // `abort()` marks the task; the runtime observes it on the next poll
        // pass, so yield a bounded number of times before reading the verdict.
        for _ in 0..32 {
            if abort.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            abort.is_finished(),
            "shutdown must abort the autoscale probe task instead of leaking it"
        );
    }

    /// Counter used by the test above; the value is never read — only the
    /// fact that the task exists and stops is under test.
    #[derive(Debug)]
    struct CountingProbe(Arc<std::sync::atomic::AtomicUsize>);

    impl RamProbePort for CountingProbe {
        fn ram_usage_percent(&self) -> crate::domain::ram_probe_port::RamUsagePercent {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::domain::ram_probe_port::RamUsagePercent::new_clamped(10.0)
        }
    }

    impl crate::domain::ram_probe_port::Sealed for CountingProbe {}

    /// Remaining-budget edges (issue #1599, strict-TDD RED): the dispatch
    /// cap must block spawns exactly when the budget is exhausted while
    /// leaving tier-bound dispatch untouched when budget is ample.
    #[test]
    fn remaining_budget_zero_when_collector_full() {
        assert_eq!(
            Engine::remaining_budget(3, 3, 0),
            0,
            "remaining == 0 spawns nothing"
        );
    }

    #[test]
    fn remaining_budget_zero_when_collected_plus_in_flight_reaches_max() {
        assert_eq!(
            Engine::remaining_budget(3, 2, 1),
            0,
            "collected + in_flight == max_pages blocks dispatch"
        );
    }

    #[test]
    fn remaining_budget_leaves_ample_budget_untouched() {
        assert_eq!(
            Engine::remaining_budget(10, 2, 2),
            6,
            "ample budget must not throttle tier-bound dispatch"
        );
    }

    #[test]
    fn remaining_budget_saturates_without_underflow() {
        assert_eq!(
            Engine::remaining_budget(3, 4, 2),
            0,
            "collected + in_flight > max_pages saturates to zero"
        );
    }

    /// Break-path parity (issue #1599, strict-TDD RED): with the collector
    /// already full before completions are processed, the first
    /// (pre-processing) budget-exit path in `crawl_loop` must invoke run
    /// cancellation exactly as the second (post-processing) path does — no
    /// spawned worker may outlive the budget decision beyond the bounded
    /// drain. Pre-fix the first path breaks without `cancel_run`, so this
    /// fails; post-fix both paths cancel.
    #[tokio::test]
    async fn first_budget_exit_break_path_cancels_in_flight_work() {
        use crate::application::crawler::collector::CrawlMessage;
        use crate::domain::DiscoveredUrl;

        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed.clone())
            .max_pages(1)
            .concurrency(nz(2))
            .build();
        let mut engine = session_engine(config);
        // Pending work so the loop body runs; the collector is already full
        // so the FIRST budget-exit path is taken before any spawn.
        engine.scheduler.seed(&engine.config.seed_url).await;
        engine
            .collector
            .send(CrawlMessage::success(DiscoveredUrl::html(
                seed.clone(),
                0,
                seed.clone(),
            )))
            .await
            .expect("collector send must succeed");
        let (task_ctx, mut tasks) = loop_harness(&mut engine);
        engine
            .crawl_loop(
                &mut tasks,
                &task_ctx,
                std::time::Instant::now(),
                Duration::from_secs(1),
                TEST_WATCHDOG,
            )
            .await
            .expect("a budget exit is not a stall");

        assert!(
            engine.cancel_token.is_cancelled(),
            "first budget-exit break path must cancel in-flight work like the second path"
        );
        assert!(
            tasks.is_empty(),
            "no worker may be spawned after the budget decision"
        );
    }

    /// Test helper: the task context plus an empty in-flight set that
    /// `crawl_loop` needs, assembled exactly like `Engine::run` does.
    ///
    /// The loop never uses the context itself — it only hands it to spawned
    /// workers — so these tests drive the loop without a real fetch.
    pub(super) fn loop_harness(
        engine: &mut Engine,
    ) -> (
        Arc<CrawlTaskCtx>,
        tokio::task::JoinSet<Result<(), CrawlError>>,
    ) {
        let session = engine.session.take().expect("session present");
        let task_ctx = session.task_ctx(CrawlExec {
            queue: engine.scheduler.queue(),
            rate_limiter: engine.rate_limiter.clone(),
            pages_crawled: Arc::clone(&engine.pages_crawled),
            error_count: Arc::clone(&engine.error_count),
            error_breakdown: Arc::clone(&engine.error_breakdown),
            collector: engine.collector.clone(),
            cookie_bridge: Arc::clone(&engine.cookie_bridge),
            banned_domains: Arc::clone(&engine.banned_domains),
            robots_fetcher: Arc::clone(&engine.robots_fetcher),
            fetch_router: engine.fetch_router.clone(),
        });
        (task_ctx, tokio::task::JoinSet::new())
    }

    /// #2338: the partial payload is the summary a completed run would have
    /// returned, truncated to what the collector already held — URLs, error
    /// count and breakdown, with the rendered failure preserved verbatim.
    #[tokio::test]
    async fn fail_run_with_partials_carries_collected_urls() {
        use crate::application::crawler::collector::CrawlMessage;
        use crate::domain::DiscoveredUrl;

        let seed = Url::parse("https://example.com").expect("valid seed URL");
        let mut engine = session_engine(CrawlerConfig::builder(seed).build());
        let collected = DiscoveredUrl::html(
            Url::parse("https://example.com/page").expect("valid page URL"),
            0,
            Url::parse("https://example.com/").expect("valid parent URL"),
        );
        engine
            .collector
            .send(CrawlMessage::success(collected))
            .await
            .expect("collector channel must be open");

        let err = engine
            .fail_run_with_partials(CrawlError::Internal("crawl stalled: test".to_string()))
            .await;

        let CrawlError::PartialRun { message, partial } = err else {
            panic!("the failure must carry the partials, got: {err:?}");
        };
        assert!(
            message.contains("crawl stalled"),
            "the rendered reason must be preserved, got: {message}"
        );
        assert_eq!(partial.total_pages, 1, "the collected page must survive");
        assert_eq!(partial.urls.len(), 1);
    }

    // ——— #1610 crawl run outcome + duration contract (OBS-H7) ———

    /// #1610 (OBS-H7): the crawl entry span must say how the run ENDED, and
    /// the `crawl completed` event must carry a millisecond duration.
    ///
    /// Before the outcome contract the span closed with configuration only,
    /// and `duration_secs` rounded every sub-second run to 0 — the crawl cost
    /// was recoverable only from the `span_close` record, which no summary
    /// query can aggregate. The crawl runs against a wiremock seed (the
    /// proven shape of the sibling entry tests) under a real FileTraceLayer,
    /// so both records are read back from the parsed JSONL.
    #[test]
    fn crawl_site_with_options_span_records_outcome_and_summary_duration_ms() {
        // #1615 (F11 / G-3 / G-4 / G-5): this path applies the literal-IP
        // entry guard, which refuses wiremock's 127.0.0.1.
        let _entry_off = webfang_test_utils::EnvGuard::entry_guard_off();

        let dir = tempfile::tempdir().expect("temp dir");
        let trace_path = dir.path().join("trace.jsonl");
        let layer = crate::infrastructure::observability::FileTraceLayer::new(trace_path.clone())
            .expect("trace layer");
        let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
        // Multi-threaded, like the sibling entry tests: the crawl spawns
        // workers. Only the entry span and the summary event are asserted —
        // both are produced on THIS thread, where the layer is installed.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");

        tracing::dispatcher::with_default(&dispatch, || {
            runtime.block_on(async {
                let server = MockServer::start().await;
                let port = server.address().port();
                Mock::given(path("/"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_string("<html><body>seed</body></html>"),
                    )
                    .mount(&server)
                    .await;

                let seed =
                    Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
                let config = CrawlerConfig::builder(seed)
                    .max_depth(0)
                    .max_pages(10)
                    .ignore_robots(true)
                    .build();
                let result = crawl_site_with_options(
                    config,
                    EngineOptions {
                        ignore_robots: true,
                        ..Default::default()
                    },
                    &CorrelationId::new(),
                )
                .await
                .expect("crawl must succeed");
                assert_eq!(result.total_pages, 1, "the seed page is crawled");
            });
        });
        drop(runtime);
        // Flush before reading: the layer buffers and only drains on drop.
        drop(dispatch);

        let records: Vec<serde_json::Value> = std::fs::read_to_string(&trace_path)
            .expect("trace file")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
            .collect();

        let closes: Vec<_> = records
            .iter()
            .filter(|r| r["record"] == "span_close" && r["span"] == "crawl_site_with_options")
            .collect();
        assert_eq!(closes.len(), 1, "exactly one crawl entry span closes");
        let span_fields = closes[0]["span_fields"].as_object().expect("span_fields");
        assert_eq!(
            span_fields.get("outcome").and_then(|v| v.as_str()),
            Some("ok"),
            "a completed crawl must close its span with a verdict"
        );
        assert!(
            span_fields.contains_key("correlation_id"),
            "the run-root identity must survive the late outcome record"
        );

        let summaries: Vec<_> = records
            .iter()
            .filter(|r| r["message"] == "crawl completed")
            .collect();
        assert_eq!(summaries.len(), 1, "exactly one crawl summary");
        let summary_fields = summaries[0]["fields"].as_object().expect("summary fields");
        let duration_ms = summary_fields
            .get("duration_ms")
            .and_then(serde_json::Value::as_u64)
            .expect("the summary must report milliseconds, not whole seconds only");
        assert!(
            summary_fields.contains_key("duration_secs"),
            "the historical field must survive — other tooling reads it"
        );
        let span_ms = closes[0]["span_duration_ms"]
            .as_u64()
            .expect("the span reports its own lifetime");
        assert!(
            duration_ms <= span_ms,
            "the summary is emitted inside the span, so its elapsed time ({duration_ms}ms) cannot exceed the span lifetime ({span_ms}ms)"
        );
    }

    /// Source that resolves immediately with a fixed event name.
    ///
    /// Stands in for the platform console source without touching the OS, so
    /// the assertion is about the engine's contract (drain on the INJECTED
    /// source) rather than about a signal a test may not deliver.
    fn immediate_source(name: &'static str) -> TerminationSource {
        TerminationSource::new(Arc::new(move || {
            let name: &'static str = name;
            Box::pin(async move { Some(name) })
        }))
    }

    /// Source that never resolves — the shape a degraded/absent platform
    /// source has.
    fn pending_source() -> TerminationSource {
        TerminationSource::new(Arc::new(|| Box::pin(std::future::pending())))
    }

    /// Source whose registration the OS REJECTED: the wait resolves to
    /// `None` instead of an event name (#509's degradation shape).
    fn rejected_source() -> TerminationSource {
        TerminationSource::new(Arc::new(|| Box::pin(async { None::<&'static str> })))
    }

    /// #1808: the engine drains on the source it was given, not on one it
    /// reaches for itself. Before the port existed, `spawn_signal_handler`
    /// named the infrastructure console source directly, so an injected source
    /// could not be observed at all — and `application` reached outward.
    #[tokio::test]
    async fn an_injected_termination_source_drives_the_engine_shutdown() {
        let shutdown: ShutdownSignal = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = CancellationToken::new();
        let handle = Engine::spawn_signal_handler(
            Arc::clone(&shutdown),
            cancel.clone(),
            immediate_source("TEST_TERMINATION_EVENT"),
        );

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the handler must return once the injected source fires")
            .expect("the handler task must not panic");

        assert!(
            shutdown.load(std::sync::atomic::Ordering::SeqCst),
            "the injected source must set the run's shutdown flag"
        );
        assert!(
            cancel.is_cancelled(),
            "the injected source must fire the run's single cancellation authority (ADR-0016)"
        );
    }

    /// Negative case for the same contract: a source that never delivers
    /// leaves the run alone. This is what a rejected Windows console-event
    /// registration degrades to, and it must NOT be read as "shutting down
    /// now" by the handler itself.
    #[tokio::test]
    async fn a_never_delivering_source_leaves_the_run_alone() {
        let shutdown: ShutdownSignal = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = CancellationToken::new();
        let handle =
            Engine::spawn_signal_handler(Arc::clone(&shutdown), cancel.clone(), pending_source());
        assert!(!handle.is_finished(), "a pending source must not resolve");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !shutdown.load(std::sync::atomic::Ordering::SeqCst),
            "an undelivered event must not set the shutdown flag"
        );
        assert!(
            !cancel.is_cancelled(),
            "an undelivered event must not cancel the run"
        );
        handle.abort();
    }

    /// The default is the Ctrl+C-only source: awaiting it installs nothing
    /// that panics and stays cancellable, which is what every construction
    /// site that never injects a platform source relies on.
    #[tokio::test]
    async fn the_default_source_is_a_cancellable_ctrl_c_wait() {
        let waiter = tokio::spawn(TerminationSource::default().wait());
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "no test may deliver Ctrl+C, so the default source must stay pending"
        );
        waiter.abort();
        assert!(waiter.await.is_err(), "abort must release the wait");
    }

    /// #1835: a REJECTED registration is not a termination event. The engine
    /// must leave the run alone — the shutdown flag unset and the
    /// cancellation token live — so the run keeps going and waits for an
    /// explicit cancel. The CLI shutdown guard answers the same way, and both
    /// say `TERMINATION_REGISTRATION_REJECTED`.
    #[tokio::test]
    async fn a_rejected_registration_leaves_the_run_alone() {
        let shutdown: ShutdownSignal = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = CancellationToken::new();
        let handle =
            Engine::spawn_signal_handler(Arc::clone(&shutdown), cancel.clone(), rejected_source());

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the handler must return once the rejected source reports itself")
            .expect("the handler task must not panic");

        assert!(
            !shutdown.load(std::sync::atomic::Ordering::SeqCst),
            "a rejected registration must not set the shutdown flag (#1835)"
        );
        assert!(
            !cancel.is_cancelled(),
            "a rejected registration must not cancel the run (#1835)"
        );
    }
}
