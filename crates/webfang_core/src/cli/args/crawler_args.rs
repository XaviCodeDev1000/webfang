//! The `CrawlerArgs` clap surface: struct and its hand-written
//! `clap::FromArgMatches` / `clap::Args` impls. Split from `crawler.rs`
//! (which keeps the value parsers); the public path
//! `crate::cli::args::crawler::CrawlerArgs` is preserved by the re-export
//! there, so spec_command, options_spec and the parity tests keep resolving
//! it unchanged.

use crate::domain::config::ConcurrencyConfig;
use crate::domain::post_load_wait::PostLoadWait;
use crate::domain::JsStrategy;
use crate::domain::ValidUrl;

/// Crawler and discovery configuration arguments.
///
/// Parsing stays derive-driven (`FromArgMatches`); command assembly is
/// spec-built (ADR-002 slice 3); see `cli::spec_command`.
#[derive(Debug, Default)]
pub struct CrawlerArgs {
    // ========== Target ==========
    /// URL to scrape (required unless using a subcommand), parsed into a
    /// hardened [`ValidUrl`] at the argv boundary by `parse_seed_url` (#1239).
    pub url: Option<ValidUrl>,

    /// CSS selector for content extraction
    pub selector: String,

    // ========== Discovery ==========
    /// Delay between requests in milliseconds
    pub delay_ms: u64,

    /// Random addition to each paced interval, in milliseconds (#2610).
    ///
    /// The effective interval is uniform over
    /// `[delay_ms, delay_ms + jitter_ms]`. `0` — the default — keeps the
    /// fixed cadence exactly as it is today.
    pub jitter_ms: u64,

    /// Maximum pages to scrape
    pub max_pages: usize,

    /// Concurrency level (auto or number)
    pub concurrency: ConcurrencyConfig,

    /// Explicit rate-limiter burst permits (token-bucket capacity).
    ///
    /// Overrides the hardware-derived budget-model default (Q1: burst is
    /// decoupled from crawl concurrency). Raw string here ON PURPOSE:
    /// validation/conversion happens once in preflight staging via
    /// `parse_rate_limit_burst` so CLI, env, and programmatic input all
    /// share one accept / reject-0 / warn-and-default semantic.
    pub rate_limit_burst: Option<String>,

    /// Use sitemap for URL discovery
    /// NOTE: HTTP redirects (301/302) are resolved at scrape-time, not parse-time.
    /// This avoids redundant HEAD requests during sitemap parsing for better performance.
    pub use_sitemap: bool,

    /// Explicit sitemap URL
    pub sitemap_url: Option<String>,

    // ========== Behavior ==========
    /// Scrape only the seed URL without discovery or crawling (batch mode always scrapes one page per URL)
    pub single_page: bool,

    /// Resume mode - skip URLs already processed
    pub resume: bool,

    /// Custom state directory for resume mode
    pub state_dir: Option<std::path::PathBuf>,

    /// Download images from the page
    pub download_images: bool,

    /// Download documents from the page
    pub download_documents: bool,

    /// Download all assets (images + documents) from the page
    pub download_assets: bool,

    /// Record extraction failure fingerprints in SQLite and attach them to
    /// low-quality extraction hints (#792). Repeated low-score extractions on
    /// the same site/selector pair accumulate a failure count surfaced in the
    /// hint, instead of degrading silently.
    pub extraction_fingerprint: bool,

    /// Use AI-powered semantic cleaning for better RAG output
    pub clean_ai: bool,

    /// Enable adaptive CSS selector repair (2-tier cascade)
    #[cfg(feature = "adaptive-selectors")]
    pub adaptive_selectors: bool,

    /// Feature flag placeholder when adaptive-selectors is not enabled
    #[cfg(not(feature = "adaptive-selectors"))]
    pub adaptive_selectors: bool,

    // ========== Display ==========
    /// Verbosity level: -v (INFO), -vv (DEBUG), -vvv (TRACE)
    pub verbose: u8,

    /// Quiet mode — suppress info/debug output
    pub quiet: bool,

    /// Dry-run mode — discover URLs and print without scraping
    pub dry_run: bool,

    /// Path to write OTel spans as JSONL for offline debugging
    pub trace_file: Option<std::path::PathBuf>,

    // ========== Crawler Settings ==========
    /// Maximum depth to crawl (0 = only seed URL)
    pub max_depth: u8,

    /// Request timeout in seconds
    pub timeout_secs: u64,

    /// Whether `timeout_secs` came from an operator (argv or
    /// `WEBFANG_TIMEOUT_SECS`) rather than from the spec's default (#2490).
    ///
    /// Carried because the VALUE alone cannot tell an explicit
    /// `--timeout-secs 30` from the default 30, and the sitemap fetch client
    /// must inherit the operator's deadline only when the operator expressed
    /// one — inheriting the spec default would silently replace the parser's
    /// own default on the no-flag path. Same `value_source` distinction as
    /// [`AiArgs::max_chars_explicit`](crate::cli::args::AiArgs::max_chars_explicit).
    pub timeout_secs_explicit: bool,

    /// URL patterns to include (glob-style). Three modes:
    ///
    /// * Path: starts with `/` → matched against URL path, e.g. `/pricing`, `/admin/*`
    /// * Path glob: starts with `*/` → matched against URL path, e.g. `*/api/*`
    /// * Host (default): matched against hostname, e.g. `example.com`, `*.example.com`
    ///
    /// Example: to exclude a path, use `--exclude-pattern "/admin/*"`, not `*admin*`
    pub include_patterns: Vec<String>,

    /// URL patterns to exclude (glob-style, same three modes as --include-pattern).
    /// Deny takes precedence over allow.
    pub exclude_patterns: Vec<String>,

    /// Estrategia de nombre de archivo para assets descargados: hash (default), slug, content-disposition
    pub asset_naming: String,

    /// Maximum concurrent asset downloads per page (omit = auto from budget model)
    pub download_concurrency: Option<usize>,

    // ========== HTTP Client Settings ==========
    /// Maximum number of retry attempts
    pub max_retries: u32,

    /// Base delay for exponential backoff (ms)
    pub backoff_base_ms: u64,

    /// Maximum delay for exponential backoff (ms)
    pub backoff_max_ms: u64,

    /// Accept-Language header value
    pub accept_language: String,

    /// Custom User-Agent header value (overrides Chrome 145 default)
    pub user_agent: Option<String>,

    /// Inject a custom HTTP header as `Name: Value` (repeatable).
    ///
    /// Overrides any default header with the same (case-insensitive) name.
    /// Example: `-H "Authorization: Bearer TOKEN"`.
    pub headers: Vec<String>,

    /// Inject a custom cookie as `name=value` (repeatable).
    ///
    /// Seeded into the cookie jar before the first request so authenticated
    /// crawls work without a prior login round-trip.
    /// Example: `--cookie "session=abc123"`.
    pub cookies: Vec<String>,

    // ========== Download Settings ==========
    /// Maximum file size to download in bytes (default: 50MB)
    pub max_file_size: u64,

    /// Timeout for individual asset downloads in seconds
    pub download_timeout: u64,

    // ========== Sitemap Settings ==========
    /// Maximum recursion depth for sitemap indexes
    pub sitemap_depth: u8,

    // ========== Competitive Features Phase 1 ==========
    /// Pages between automatic checkpoint saves (0 = disabled) — unified via PersistenceMode with --resume (Checkpoint/Full when enabled)
    pub checkpoint_interval: u64,

    /// Disable checkpoint persistence entirely — PersistenceMode disables checkpoint (Resume only when combined with --resume)
    pub no_checkpoint: bool,

    /// Skip robots.txt enforcement
    pub ignore_robots: bool,

    /// Skip WAF/CAPTCHA classification (REQ-WAF-07): challenged responses are reported
    /// as plain HTTP errors instead of WAF blocks. It does not rescue the fetch — a
    /// challenge page is never scraped as content (F-11).
    pub ignore_waf: bool,

    /// Enable autoscaled concurrency — dynamically adjusts task concurrency based on RAM usage
    pub autoscale: bool,

    /// Disable session pool health checks
    pub no_session_health: bool,

    /// TLS/HTTP2 profile name (default: Chrome145)
    pub h2_profile: String,

    /// JavaScript rendering strategy: static (wreq only), hybrid (3-layer), full (Chromiumoxide only)
    pub js_strategy: JsStrategy,

    /// Post-load settlement wait for the chromium render path (F-52-b):
    /// idle (default), fixed ms, or none.
    pub post_load_wait: PostLoadWait,

    /// Path to the obscura binary (default: "obscura")
    pub obscura_binary: String,

    /// Enable DOM pre-pruning before Readability (removes invisible/empty wrappers).
    /// Default: enabled (true). Set to false via --dom-preprune=false or WEBFANG_DOM_PREPRUNE=false.
    pub dom_preprune: bool,
}

impl clap::FromArgMatches for CrawlerArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use crate::cli::spec_command::extract;
        Ok(Self {
            url: extract::opt(m, "url"),
            selector: extract::value(m, "selector")?,
            delay_ms: extract::value(m, "delay_ms")?,
            jitter_ms: extract::value(m, "jitter_ms")?,
            concurrency: extract::value(m, "concurrency")?,
            max_pages: extract::value(m, "max_pages")?,
            rate_limit_burst: extract::opt(m, "rate_limit_burst"),
            use_sitemap: m.get_flag("use_sitemap"),
            sitemap_url: extract::opt(m, "sitemap_url"),
            single_page: m.get_flag("single_page"),
            resume: m.get_flag("resume"),
            state_dir: extract::opt(m, "state_dir"),
            download_images: m.get_flag("download_images"),
            download_documents: m.get_flag("download_documents"),
            download_assets: m.get_flag("download_assets"),
            extraction_fingerprint: m.get_flag("extraction_fingerprint"),
            clean_ai: m.get_flag("clean_ai"),
            adaptive_selectors: m.get_flag("adaptive_selectors"),
            verbose: m.get_count("verbose"),
            quiet: m.get_flag("quiet"),
            dry_run: m.get_flag("dry_run"),
            trace_file: extract::opt(m, "trace_file"),
            max_depth: extract::value(m, "max_depth")?,
            timeout_secs: extract::value(m, "timeout_secs")?,
            timeout_secs_explicit: crate::cli::args::chosen_by_operator(m, "timeout_secs"),
            include_patterns: extract::many(m, "include_patterns"),
            exclude_patterns: extract::many(m, "exclude_patterns"),
            asset_naming: extract::value(m, "asset_naming")?,
            download_concurrency: extract::opt(m, "download_concurrency"),
            max_retries: extract::value(m, "max_retries")?,
            backoff_base_ms: extract::value(m, "backoff_base_ms")?,
            backoff_max_ms: extract::value(m, "backoff_max_ms")?,
            accept_language: extract::value(m, "accept_language")?,
            user_agent: extract::opt(m, "user_agent"),
            headers: extract::many(m, "headers"),
            cookies: extract::many(m, "cookies"),
            max_file_size: extract::value(m, "max_file_size")?,
            download_timeout: extract::value(m, "download_timeout")?,
            sitemap_depth: extract::value(m, "sitemap_depth")?,
            checkpoint_interval: extract::value(m, "checkpoint_interval")?,
            no_checkpoint: m.get_flag("no_checkpoint"),
            ignore_robots: m.get_flag("ignore_robots"),
            ignore_waf: m.get_flag("ignore_waf"),
            autoscale: m.get_flag("autoscale"),
            no_session_health: m.get_flag("no_session_health"),
            h2_profile: extract::value(m, "h2_profile")?,
            js_strategy: extract::value(m, "js_strategy")?,
            post_load_wait: extract::value(m, "js_wait")?,
            obscura_binary: extract::value(m, "obscura_binary")?,
            dom_preprune: m.get_flag("dom_preprune"),
        })
    }

    fn update_from_arg_matches(&mut self, m: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(m)?;
        Ok(())
    }
}

impl clap::Args for CrawlerArgs {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.args(crate::cli::spec_command::crawler_args(
            crate::cli::spec_command::Headings::Applied,
        ))
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}
