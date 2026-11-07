//! Crawl entry points of the engine — the `crawl_site*` free functions that
//! form the crate's public crawl surface, extracted verbatim from
//! `engine.rs` (file-size Ola 1, #2584). Free functions, not inherent
//! methods of [`Engine`]: `engine.rs` re-exports the three `pub` entries so
//! the historical paths (`crawler::engine::*`, `crawler::*` and
//! `application::*`) keep resolving unchanged, and the module stays private
//! — the session-builder and inner helpers have zero callers outside it.

use std::sync::Arc;

use tracing::{info, instrument};

use super::{CrawlContentSink, Engine, EngineOptions};
use crate::application::crawler::session::{
    CrawlIdentity, CrawlPorts, CrawlSession, TransportPolicy,
};
use crate::domain::downloader_factory::DownloaderFactory;
use crate::domain::persistence::{CheckpointCfg, PersistenceMode};
use crate::domain::{
    post_load_wait::PostLoadWait, CorrelationId, CrawlError, CrawlResult, CrawlerConfig, JsStrategy,
};
use crate::infrastructure::observability::log_scrape_error;

/// Crawl a website starting from the seed URL
///
/// Thin wrapper that creates an Engine, runs the crawl loop, and shuts down.
///
/// Following **async-no-lock-across-await**: Uses JoinSet for concurrency control
/// without redundant Semaphore (JoinSet already limits via tasks.len()).
/// Following **async-clone-before-await**: Clones config before async operations.
///
/// # Arguments
///
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(CrawlResult)` - Crawl result with discovered URLs
/// * `Err(CrawlError)` - Error during crawling
///
/// # Examples
///
/// The example targets the replacement entry ([`crawl_site_with_options`]):
/// unlike this shim, it keeps every engine knob visible at the call site.
///
/// ```no_run
/// use webfang_core::{domain::CrawlerConfig, application::crawl_site_with_options};
/// use webfang_core::application::crawler::engine::EngineOptions;
/// use webfang_core::domain::CorrelationId;
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::builder(seed)
///     .max_depth(2)
///     .max_pages(50)
///     .build();
///
/// let correlation = CorrelationId::new();
/// let result = crawl_site_with_options(config, EngineOptions::default(), &correlation).await?;
/// println!("Crawled {} pages", result.total_pages);
/// # Ok(())
/// # }
/// ```
///
/// # Deprecation (since 2.2.0, #1369)
///
/// This entry silently uses default engine options — checkpointing, the
/// session pool, the JS strategy, robots handling and content capture all
/// stay invisible at the call site. Use [`crawl_site_with_options`] instead.
/// The shim keeps working exactly as before until removal.
///
/// Run-root contract (#1439): this is a STANDALONE entry — a caller of this
/// function performs the whole operation here, so the shim mints its own
/// run-root at the boundary (like `scrape_with_readability`). Production
/// routes must NOT use it: they own a root and propagate it through
/// [`crawl_site_with_options`].
#[deprecated(
    since = "2.2.0",
    note = "Use crawl_site_with_options instead: EngineOptions lets callers set checkpoint, session pool, js_strategy, robots and content sink explicitly instead of inheriting silent defaults (#1369)."
)]
pub async fn crawl_site(config: CrawlerConfig) -> Result<CrawlResult, CrawlError> {
    crawl_site_inner(config, CorrelationId::new(), None).await
}

/// Crawl a website and capture every fetched page body into `sink`.
///
/// Same crawl semantics as [`crawl_site`], but the raw body of each fetched
/// page is handed to the sink before link extraction discards it.
/// [`CrawlResult`] carries metadata only, so this is the only way for a caller
/// to obtain crawl content without a second HTTP round-trip — the gap that
/// made `--batch` write zero files (#631).
///
/// # Errors
///
/// Returns [`CrawlError`] when the engine cannot be constructed or the crawl
/// loop fails, exactly as [`crawl_site`] does.
///
/// # Deprecation (since 2.2.0, #1369)
///
/// The sink is expressible as [`EngineOptions::content_sink`], and every
/// other knob the sibling [`crawl_site`] inherits silently rides on the same
/// struct — the `content_sink` doc commits to a single capture entry
/// ("checkpoint-only, capture-only, and both all converge in
/// [`crawl_site_with_options`]; no second entry function"). Use that entry
/// with `options.content_sink = Some(sink)` instead. The shim keeps working
/// exactly as before until removal.
#[deprecated(
    since = "2.2.0",
    note = "Use crawl_site_with_options with EngineOptions::content_sink instead: one explicit entry for capture, checkpoint, session pool, js_strategy and robots (#1369)."
)]
pub async fn crawl_site_capturing(
    config: CrawlerConfig,
    sink: Arc<dyn CrawlContentSink>,
) -> Result<CrawlResult, CrawlError> {
    crawl_site_inner(config, CorrelationId::new(), Some(sink)).await
}

/// Session-builder helper (T7 slice-6, #1881): owns the shared
/// `content_sink`/`pipeline` ports → `.identity(...)` →
/// `.build()` → `map_err` chain of the crawl-path entries.
///
/// The two crawl paths differ in persistence, transport, and injected ports,
/// so those arrive as parameters — nothing is silently unified. Session
/// contents, error mapping, and the `log_scrape_error` url/stage/correlation
/// contexts stay identical at every call site. The `#[cfg(test)]` session
/// helpers keep their own chain on purpose: they fail with `.expect()` and
/// mint a fresh root instead of reporting through `log_scrape_error`, so they
/// are a different cluster, not this helper's.
///
/// Sync with no lock guards, so there is nothing to hold across an `.await`
/// (`#![deny(clippy::await_holding_lock)]` above keeps enforcing that).
fn build_crawl_session(
    config: CrawlerConfig,
    persistence: PersistenceMode,
    transport: TransportPolicy,
    content_sink: Option<Arc<dyn CrawlContentSink>>,
    downloader_factory: Option<Arc<dyn DownloaderFactory>>,
    correlation_id: CorrelationId,
) -> Result<CrawlSession, CrawlError> {
    let run_label = config.seed_url.host_str().unwrap_or("seed").to_string();
    let seed_url = config.seed_url.as_str().to_string();
    // P6-2 slice 2: build the validated run object first; the engine
    // executes from it. A build failure (exotic seed, inconsistent
    // transport) fails the run before any worker spawns — no silent
    // legacy fallback (matrix rows 31/33).
    CrawlSession::builder()
        .config(config)
        .persistence(persistence)
        .transport(transport)
        .ports(CrawlPorts {
            session_pool: None,
            downloader_factory,
            content_sink,
            pipeline: None,
        })
        .identity(CrawlIdentity {
            root: correlation_id.clone(),
            run_label,
        })
        .build()
        .map_err(|err| {
            log_scrape_error(
                &err,
                &seed_url,
                "session",
                Some(&correlation_id),
                "session build failed — refusing to run (no legacy fallback)",
            );
            CrawlError::from(err)
        })
}

/// Inner implementation of [`crawl_site`].
///
/// The `#[instrument]` span declares the run-root identity (`correlation_id`,
/// `trace_id`) AT CREATION time (#501) — they are arguments. What it cannot
/// see is how the run ENDED, so `outcome` is declared `Empty` and recorded by
/// [`Engine::record_run_outcome`] once the engine returns (#1610, OBS-H7); the
/// FileTraceLayer merges late records into the same snapshot (`on_record`).
/// The instrumented span lifecycle is async-safe — no `enter()` guard crosses
/// an `.await` (#519).
#[instrument(
    name = "crawl_site",
    skip(config, correlation_id, content_sink),
    fields(
        correlation_id = %correlation_id,
        trace_id = %correlation_id.trace_id(),
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages,
        delay_ms = config.delay_ms,
        concurrency = config.concurrency.get(),
        outcome = tracing::field::Empty
    )
)]
async fn crawl_site_inner(
    config: CrawlerConfig,
    correlation_id: CorrelationId,
    content_sink: Option<Arc<dyn CrawlContentSink>>,
) -> Result<CrawlResult, CrawlError> {
    info!(
        "Starting crawl from {} with max_depth={} max_pages={}",
        config.seed_url, config.max_depth, config.max_pages
    );

    let ignore_robots = config.ignore_robots;
    let mut session = build_crawl_session(
        config,
        PersistenceMode::Disabled,
        TransportPolicy {
            js_strategy: JsStrategy::Static,
            tls_emulation: wreq_util::Profile::Chrome145,
            ignore_waf: false,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            obscura_binary: crate::domain::downloader_factory::DEFAULT_OBSCURA_BINARY.to_string(),
            chrome_binary: None,
            post_load_wait: PostLoadWait::Idle,
            session_pool_enabled: false,
            autoscale_enabled: false,
            ignore_robots,
        },
        content_sink,
        None,
        correlation_id,
    )?;
    session.begin();
    let mut engine = Engine::from_session(session)?;
    let result = engine.run().await;
    // Record BEFORE `shutdown()`: shutdown cancels the engine's own token, so
    // a verdict read afterwards could not tell an operator-triggered stop from
    // a self-inflicted one.
    engine.record_run_outcome(&result);
    engine.shutdown().await;
    result
}

/// Crawl a website with fine-grained engine options.
///
/// This is the advanced entry point for callers that need checkpointing,
/// session pooling, or explicit robots.txt control beyond what
/// `CrawlerConfig.ignore_robots` provides.
///
/// # Run-root identity (#1439)
///
/// `correlation` is the calling operation's run-root [`CorrelationId`]
/// (#501). This entry NO LONGER mints one: every production route (CLI
/// discovery, batch, MCP, benchmark) owns a root at its operation boundary
/// and must propagate it here, so the engine spans and the caller's events
/// share one `trace_id` and the whole run reconstructs from the announced
/// identity. A genuinely standalone caller for which this call IS the
/// operation mints its own root at the call site
/// (`CorrelationId::new()`), documenting that contract — the silent
/// mint-on-behalf is what re-broke causability after #1232.
///
/// # Arguments
///
/// * `config` - Crawler configuration (seed, depth, patterns, etc.)
/// * `options` - Engine-level options (checkpoint, session pool, robots)
/// * `correlation` - The operation's run-root correlation identity (#1439)
///
/// # Returns
///
/// * `Ok(CrawlResult)` - Crawl result with discovered URLs
/// * `Err(CrawlError)` - Error during crawling
///
/// # Examples
///
/// ```no_run
/// use webfang_core::{domain::CrawlerConfig, application::crawl_site_with_options};
/// use webfang_core::application::crawler::engine::EngineOptions;
/// use webfang_core::domain::{CorrelationId, JsStrategy};
/// use std::time::Duration;
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::builder(seed)
///     .max_depth(2)
///     .max_pages(50)
///     .build();
///
/// let options = EngineOptions {
///     checkpoint_path: Some(std::path::PathBuf::from("/tmp/checkpoint")),
///     session_pool_enabled: true,
///     js_strategy: JsStrategy::Static,
///     autoscale_enabled: true,
///     ..Default::default()
/// };
///
/// // The caller owns the run-root: this call is the whole operation.
/// let correlation = CorrelationId::new();
/// let result = crawl_site_with_options(config, options, &correlation).await?;
/// println!("Crawled {} pages", result.total_pages);
/// # Ok(())
/// # }
/// ```
pub async fn crawl_site_with_options(
    config: CrawlerConfig,
    options: EngineOptions,
    correlation: &CorrelationId,
) -> Result<CrawlResult, CrawlError> {
    crawl_site_with_options_inner(config, options, correlation.clone()).await
}

/// Inner implementation of [`crawl_site_with_options`].
///
/// The `#[instrument]` span declares the run-root identity (`correlation_id`,
/// `trace_id`) AT CREATION time (#501) — they are arguments. The run verdict
/// is not, so `outcome` is declared `Empty` and recorded by
/// [`Engine::record_run_outcome`] after the engine returns (#1610, OBS-H7).
/// The instrumented span lifecycle is async-safe — no `enter()` guard crosses
/// an `.await` (#519).
#[instrument(
    name = "crawl_site_with_options",
    skip(config, options, correlation_id),
    fields(
        correlation_id = %correlation_id,
        trace_id = %correlation_id.trace_id(),
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages,
        checkpoint_enabled = options.checkpoint_path.is_some(),
        session_pool = options.session_pool_enabled,
        ignore_robots = options.ignore_robots,
        capture_enabled = options.content_sink.is_some(),
        shared_limiter = options.rate_limiter.is_some(),
        outcome = tracing::field::Empty
    )
)]
async fn crawl_site_with_options_inner(
    config: CrawlerConfig,
    options: EngineOptions,
    correlation_id: CorrelationId,
) -> Result<CrawlResult, CrawlError> {
    info!(
        "Starting crawl from {} with max_depth={} max_pages={} (checkpoint={}, session_pool={}, ignore_robots={})",
        config.seed_url,
        config.max_depth,
        config.max_pages,
        options.checkpoint_path.is_some(),
        options.session_pool_enabled,
        options.ignore_robots
    );

    // P6-2 slice 1: the run object carries what `EngineOptions` carried.
    // Checkpoint mode comes from the same (path, interval) pair the
    // `with_checkpoint` chain it replaced consumed. Ports the options cannot
    // prebuild (pool needs the budget tier) stay unset — `from_session`
    // assembles them exactly as the pre-session entry body did.
    let mode = match &options.checkpoint_path {
        Some(dir) => PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: dir.clone(),
                interval: options.checkpoint_interval,
            },
        },
        None => PersistenceMode::Disabled,
    };
    let mut session = build_crawl_session(
        config,
        mode,
        TransportPolicy::from(&options),
        options.content_sink.clone(),
        options.downloader_factory.clone(),
        correlation_id,
    )?;

    session.begin();
    let mut engine = Engine::from_session(session)?;
    // #1428 RUN-scope limiter: a shared bucket carried on the options
    // replaces the per-engine bucket `build_machinery` minted (via
    // `from_session`), so `--delay-ms` paces across the engines of one
    // run instead of granting every engine a fresh burst. The swap happens
    // before `run()` spawns any task, so no worker ever observes the
    // minted bucket. `None` keeps the historical per-engine behavior.
    if let Some(limiter) = options.rate_limiter.clone() {
        engine.rate_limiter = limiter;
    }
    let result = engine.run().await;
    // Record BEFORE `shutdown()` for the same reason as `crawl_site_inner`.
    engine.record_run_outcome(&result);
    engine.shutdown().await;
    result
}
