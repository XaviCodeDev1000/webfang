//! Engine construction — the machinery builder and the session constructor,
//! extracted verbatim from `engine.rs` (file-size Ola 1, #2584). Inherent
//! methods of [`Engine`]: the child module reaches the engine's private
//! fields, and the module stays private — no re-export is needed because
//! nothing here is part of the crate's public surface.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::RwLock as AsyncRwLock;
use tokio_util::sync::CancellationToken;

use super::rate_limiter_config;
use super::Engine;
use crate::application::crawler::checkpoint::{BincodeCheckpoint, CheckpointPath};
use crate::application::crawler::collector::ResultsCollector;
use crate::application::crawler::crawl_scheduler::CrawlScheduler;
use crate::application::crawler::ports::TerminationSource;
use crate::application::crawler::session::CrawlSession;
use crate::application::rate_limiter::SharedRateLimiter;
use crate::domain::budget::BudgetModel;
use crate::domain::cookie_bridge::CookieBridge;
use crate::domain::ram_probe_port::system_default;
use crate::domain::session_port::SessionPoolConfig;
use crate::domain::{CorrelationId, CrawlError, CrawlerConfig, JsStrategy};

impl Engine {
    /// Build the engine's execution machinery from a config.
    ///
    /// Private and constructor-internal by design (P6-2 slice 4): the only
    /// caller-facing constructor is [`Engine::from_session`], which fills the
    /// session slot before the engine escapes this function. The operator-level
    /// [`BudgetOverrides`](crate::domain::budget::BudgetOverrides) carried on
    /// the config feed `BudgetModel::build`, so
    /// an explicit `--concurrency` / `--rate-limit-burst` reaches the scheduler
    /// spawn bound and rate-limiter burst instead of being silently replaced by
    /// the auto table (Bug R2-1, design D4: ONE budget derivation per run).
    fn build_machinery(config: CrawlerConfig) -> Result<Self, CrawlError> {
        let overrides = config.budget_overrides;
        let config = Arc::new(config);
        let config_clone = Arc::clone(&config);

        // ONE budget derivation per run — every tier below reads from this
        // immutable snapshot.
        let budget =
            BudgetModel::build(overrides, &crate::domain::budget::detector::SystemDetector);

        // Create rate limiter using SharedRateLimiter (single source of truth).
        // Burst derives from the model (Q1 DECOUPLE); delay_ms unchanged.
        let rate_limiter_config = rate_limiter_config(&config_clone, &budget);
        let rate_limiter = match SharedRateLimiter::new(&rate_limiter_config) {
            Ok(limiter) => limiter,
            // LCOV_EXCL_LINE defensive: rate-limiter-config — SharedRateLimiter::new fails only on invalid config, an invariant
            Err(e) => return Err(CrawlError::Internal(e.to_string())),
        };

        // Scheduling policy owns the visited set, its checkpoint string mirror,
        // and the shared discovery queue (Arc-shared with the per-page tasks).
        // The spawn bound derives from the model's Operation.crawl tier, not
        // from `CrawlerConfig.concurrency` (task 2.2b).
        let scheduler = CrawlScheduler::new(budget.crawl());

        // Results collector via mpsc channel
        let collector = ResultsCollector::new(config_clone.max_pages, Some(config_clone.max_pages));
        let error_count = Arc::new(AtomicUsize::new(0));
        let error_breakdown = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
        let pages_crawled = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        // Robots.txt port — shares the crawl's TLS fingerprint so the
        // robots.txt request is indistinguishable from a page fetch (#337).
        // Built through the composition-root helper (trait in domain,
        // concrete in infra — ADR-0012-B post-narrow robots slice).
        let robots_fetcher = crate::application::container::build_robots_fetcher(
            config_clone.tls_emulation,
            config_clone.timeout_secs,
        )
        // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
        .map_err(|e| CrawlError::Internal(e.to_string()))?;

        Ok(Self {
            config,
            correlation_id: CorrelationId::new(),
            collector,
            scheduler,
            rate_limiter,
            budget,
            error_count,
            error_breakdown,
            checkpoint_store: BincodeCheckpoint::new(),
            checkpoint_state: None,
            checkpoint_path: None,
            checkpoint_interval: 100,
            robots_fetcher,
            pages_crawled,
            shutdown,
            cancel_token: CancellationToken::new(),
            js_strategy: JsStrategy::default(),
            fetch_router: None,
            downloader_factory: None,
            cookie_bridge: Arc::new(AsyncRwLock::new(CookieBridge::new())),
            banned_domains: Arc::new(RwLock::new(Vec::new())),
            content_sink: None,
            pipeline: None,
            // Filled by `from_session` before the engine escapes the
            // private machinery builder — no engine ever runs session-less.
            session: None,
            signal_handle: None,
            // Filled by `with_autoscale`; `shutdown` ends it.
            autoscale_handle: None,
            // Default to the sysinfo-backed probe so the autoscale loop is
            // wired without any extra setup. Tests inject a fake via
            // `Engine::with_ram_probe` (no real sysinfo reads in unit tests).
            // The factory is domain-side: `application` must not name the
            // infrastructure concrete (ADR-0012-B cheap win).
            ram_probe: system_default(),
            // Ctrl+C only until the composition root injects the real source:
            // `build_machinery` is application-local and cannot know the
            // platform, so the default must be the host-agnostic floor.
            termination_source: TerminationSource::default(),
        })
    }

    /// Build an Engine from a validated [`CrawlSession`] (P6-2) — the ONLY
    /// constructor since slice 4: a session-built run is the only way to
    /// obtain a crawl engine.
    ///
    /// Run facts (config, identity, policies, ports) come from the session;
    /// execution machinery is built by the private `build_machinery` from
    /// exactly the config the session was validated with. The session is
    /// stored so [`Engine::run`] derives the shared context from it and
    /// closes the run through [`CrawlSession::finish`].
    ///
    /// # Errors
    ///
    /// Returns [`CrawlError`] when the engine or its fetch router cannot
    /// be constructed — same contract as the `with_*` chain it replaced.
    pub(crate) fn from_session(mut session: CrawlSession) -> Result<Self, CrawlError> {
        let transport = session.transport.clone();
        let config = session.config().as_ref().clone();
        let mut engine = Engine::build_machinery(config)?;
        // Platform-aware termination source (XP-S-02, #1808): built by the
        // composition root, which is the one `application → infrastructure`
        // edge this crate allowlists. Injected here — at the ONLY constructor
        // — so every production path that starts a crawl (crawl, sitemap
        // crawl, batch, CLI discovery, MCP crawl tool, benchmark) drains on
        // the console close/logoff/shutdown events `ctrl_c()` never sees.
        engine = engine
            .with_termination_source(crate::application::container::build_termination_source());
        // Adopt the session's single-minted identity: every page shares
        // its trace_id by construction (P6-2 identity requirement).
        engine.correlation_id = session.identity().root.clone();
        // Adopt the session's single cancellation authority (#509).
        engine.cancel_token = session.cancel_token();
        // P6-2 wiring fix (#1291): `session.ports.session_pool` is the single
        // injection point the derived task ctx reads (`CrawlSession::task_ctx`).
        // A `session_pool_enabled` transport flag with no injected pool derives
        // one into the ports BEFORE the session is stored — the pre-seam
        // semantics (pool reaches workers) that the direct `Engine.session_pool`
        // mirror silently lost for session-built runs. Slot count derives from
        // the model's Domain tier (task 2.2c); the 2s cooldown is the backoff
        // base delay — the same shape the pre-session entry derived.
        if session.ports.session_pool.is_none() && transport.session_pool_enabled {
            let pool_cfg = SessionPoolConfig {
                base_delay: Duration::from_secs(2),
                pool_size: engine.budget.domain(),
                ..SessionPoolConfig::default()
            };
            session.ports.session_pool = Some(
                crate::application::container::build_crawl_session_pool(pool_cfg),
            );
        }
        if let Some(factory) = session.ports.downloader_factory.clone() {
            engine = engine.with_downloader_factory(factory);
        }
        if let Some(sink) = session.ports.content_sink.clone() {
            engine = engine.with_content_sink(sink);
        }
        engine.pipeline = session.ports.pipeline.clone();
        // Checkpoint wiring from the session policy (mirrors
        // `with_checkpoint`, but reuses the state `begin()` loaded
        // instead of hitting disk a second time).
        match &session.persistence.mode {
            crate::domain::persistence::PersistenceMode::Checkpoint { cfg }
            | crate::domain::persistence::PersistenceMode::Full {
                checkpoint: cfg, ..
            } => {
                let scoped =
                    CheckpointPath::new(&cfg.dir).file_for_seed(engine.config.seed_url.as_str());
                engine.checkpoint_path = Some(scoped);
                engine.checkpoint_interval = cfg.interval;
                engine.checkpoint_state = session.persistence.loaded.clone();
            },
            crate::domain::persistence::PersistenceMode::Disabled
            | crate::domain::persistence::PersistenceMode::Resume { .. } => {},
        }
        if transport.autoscale_enabled {
            engine = engine.with_autoscale();
        }
        engine = engine.with_js_strategy(
            transport.js_strategy,
            transport.tls_emulation,
            transport.ignore_waf,
            transport.max_retries,
            transport.backoff_base_ms,
            transport.backoff_max_ms,
            transport.obscura_binary.clone(),
            transport.post_load_wait,
            transport.chrome_binary.clone(),
        )?;
        engine.session = Some(session);
        Ok(engine)
    }
}
