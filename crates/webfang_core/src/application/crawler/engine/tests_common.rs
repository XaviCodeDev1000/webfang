use super::*;
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::application::crawler::session::CrawlExec;
use crate::domain::downloader_factory::DEFAULT_OBSCURA_BINARY;
use crate::domain::persistence::PersistenceMode;
use crate::domain::session_port::SessionPort;
use crate::domain::CrawlError;

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
