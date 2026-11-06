//! Engine-level crawl options — controls Engine internals beyond `CrawlerConfig`.
//!
//! Extracted from `engine.rs` to keep that module within the file-size
//! ratchet. Re-exported from `engine` so `crawler::EngineOptions` and the
//! public `engine::EngineOptions` path stay intact.

use std::path::PathBuf;
use std::sync::Arc;

use wreq_util::Profile;

use crate::application::crawler::content_sink::CrawlContentSink;
use crate::application::rate_limiter::SharedRateLimiter;
use crate::domain::downloader_factory::{DownloaderFactory, DEFAULT_OBSCURA_BINARY};
use crate::domain::post_load_wait::PostLoadWait;
use crate::domain::JsStrategy;

/// Engine-level crawl options — controls Engine internals beyond CrawlerConfig.
///
/// While `CrawlerConfig` defines *what* to crawl (seed, depth, patterns),
/// `EngineOptions` controls *how* the Engine operates (checkpointing,
/// session pooling, robots.txt enforcement).
///
/// Transitional view (P6-2, #1291): after slice 4 the engine itself is built
/// only from a validated `CrawlSession`; this
/// struct remains as the public options DTO for [`crawl_site_with_options`],
/// consumed via `TransportPolicy::from(&options)`. Its deprecation is the
/// declared destination but a separate act — the signed design requires its
/// own `type:breaking-change` decision plus a caller migration (CLI discovery,
/// benchmark), so it is deliberately kept here.
///
/// Named `EngineOptions` (not `CrawlOptions`) to avoid collision with
/// `application::crawl_options::CrawlOptions`, which is the CLI-level
/// configuration struct.
///
/// `Clone` without a derived `Debug`: the run-scoped
/// [`SharedRateLimiter`]
/// carries a governor bucket with no `Debug` impl, so the manual impl below
/// renders every field and the limiter as presence-only.
#[derive(Clone)]
pub struct EngineOptions {
    /// Path to save checkpoint files. `None` disables checkpointing.
    pub checkpoint_path: Option<PathBuf>,
    /// Pages between checkpoint saves (0 = disabled, but `checkpoint_path` None already disables).
    /// Defaults to 100 for backward compat; `PersistenceMode` overrides via `checkpoint_interval`.
    pub checkpoint_interval: u64,
    /// Enable the domain session pool for per-domain rate limiting.
    pub session_pool_enabled: bool,
    /// Skip robots.txt enforcement.
    pub ignore_robots: bool,
    /// JavaScript rendering strategy.
    pub js_strategy: JsStrategy,
    /// Obscura binary name or path for the Hybrid strategy's Layer 2 (#787).
    ///
    /// A path is invoked as given; a bare name is resolved from `PATH`.
    /// Defaults to `obscura`.
    pub obscura_binary: String,
    /// Post-load settlement wait for the chromium render path (F-52-b).
    /// Defaults to [`PostLoadWait::Idle`]; populated from
    /// `CrawlOptions.network.post_load_wait` by the CLI composition root.
    pub post_load_wait: PostLoadWait,
    /// Preflight-resolved Chrome/Chromium binary for the chromium render
    /// path (F-52-c, #1278).
    ///
    /// `Some(path)` pins every chromium launch to the gate-certified
    /// binary; `None` (default, MCP/benchmark/test paths) keeps launcher
    /// auto-detection. Populated from `CrawlOptions.network.chrome_binary`
    /// by the CLI composition root.
    pub chrome_binary: Option<PathBuf>,
    /// Enable autoscaled concurrency based on system RAM.
    pub autoscale_enabled: bool,
    /// TLS/HTTP2 fingerprint profile applied to the wreq fetch layer.
    ///
    /// Defaults to [`Profile::Chrome145`], the historical hardcoded fingerprint.
    pub tls_emulation: Profile,
    /// Bypass WAF classification on the hybrid spa-detection path (REQ-WAF-07).
    ///
    /// Defaults to `false`. When `true`, a genuine T1 challenge is treated per
    /// normal spa/static logic instead of aborting the fetch.
    pub ignore_waf: bool,
    /// Maximum number of retry attempts for failed fetches.
    pub max_retries: u32,
    /// Base delay for exponential backoff (ms).
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff (ms).
    pub backoff_max_ms: u64,
    /// Factory that builds the fetch downloader for `js_strategy`.
    ///
    /// `None` (the default) means the engine records the strategy but builds
    /// no downloader, so the crawl uses the static `fetch_url()` path. Any
    /// caller that needs `Hybrid` or `Full` rendering MUST inject a factory —
    /// the only production implementation lives in
    /// [`crate::infrastructure::downloader::fetch_router::DefaultDownloaderFactory`],
    /// so wiring it is a `cli`/composition-root responsibility (`cli` is
    /// exempt from the ADR-0010 direction gate).
    pub downloader_factory: Option<Arc<dyn DownloaderFactory>>,
    /// Optional sink capturing every fetched page body (F-05, #1229).
    ///
    /// `None` (the default) keeps the metadata-only crawl: bodies are
    /// discarded after link extraction. `Some` wires the sink through the
    /// SAME engine path — checkpoint-only, capture-only, and both all
    /// converge in [`crawl_site_with_options`]; no second entry function.
    pub content_sink: Option<Arc<dyn CrawlContentSink>>,
    /// Run-scoped token bucket shared by every engine of one run (#1428).
    ///
    /// `None` (the default) keeps the historical behavior: `build_machinery`
    /// mints a fresh limiter per engine. `Some` makes
    /// [`crawl_site_with_options`] adopt it instead, so `--delay-ms` paces
    /// across engines — batch runs one engine per URL, and a per-engine
    /// bucket restores a full burst per URL, making cross-URL spacing
    /// structurally impossible. Carried by `Clone` (one `Arc` clone per
    /// task), exactly like `content_sink`.
    pub rate_limiter: Option<SharedRateLimiter>,
}

impl std::fmt::Debug for EngineOptions {
    /// Manual `Debug`: `SharedRateLimiter` carries a governor bucket with
    /// no `Debug` impl, so the struct cannot derive it. Every field is
    /// rendered; the limiter as presence-only (token counts are runtime
    /// state, not configuration).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineOptions")
            .field("checkpoint_path", &self.checkpoint_path)
            .field("checkpoint_interval", &self.checkpoint_interval)
            .field("session_pool_enabled", &self.session_pool_enabled)
            .field("ignore_robots", &self.ignore_robots)
            .field("js_strategy", &self.js_strategy)
            .field("obscura_binary", &self.obscura_binary)
            .field("post_load_wait", &self.post_load_wait)
            .field("chrome_binary", &self.chrome_binary)
            .field("autoscale_enabled", &self.autoscale_enabled)
            .field("tls_emulation", &self.tls_emulation)
            .field("ignore_waf", &self.ignore_waf)
            .field("max_retries", &self.max_retries)
            .field("backoff_base_ms", &self.backoff_base_ms)
            .field("backoff_max_ms", &self.backoff_max_ms)
            .field("downloader_factory", &self.downloader_factory)
            .field("content_sink", &self.content_sink)
            .field("rate_limiter_shared", &self.rate_limiter.is_some())
            .finish()
    }
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            checkpoint_path: None,
            checkpoint_interval: 100,
            session_pool_enabled: false,
            ignore_robots: false,
            js_strategy: JsStrategy::default(),
            // #787: keep today's `obscura`-on-PATH behavior for callers that
            // do not configure the binary.
            obscura_binary: DEFAULT_OBSCURA_BINARY.to_string(),
            // F-52-b: idle by default; the CLI gate path resolves it.
            post_load_wait: PostLoadWait::Idle,
            // F-52-c: unresolved by default; the CLI gate resolves it.
            chrome_binary: None,
            autoscale_enabled: false,
            tls_emulation: Profile::Chrome145,
            ignore_waf: false,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            downloader_factory: None,
            content_sink: None,
            rate_limiter: None,
        }
    }
}
