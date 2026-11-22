use crate::domain::config::ConcurrencyConfig;
use crate::domain::options_spec::crawler as crawler_specs;
use crate::domain::post_load_wait::PostLoadWait;
use crate::domain::JsStrategy;
use crate::domain::ValidUrl;
use scraper::Selector;

/// Validate `--download-concurrency`: must be >= 1. A value of 0 would make
/// `buffer_unordered(0)` hang forever (deadlock, D1). Rejecting here satisfies
/// the "Zero Silent Loss" philosophy with a clear CLI error instead of a hang.
/// Bounds and messages come from the OptionsSpec (ADR-002) — the single
/// validation source.
pub(crate) fn parse_download_concurrency(s: &str) -> Result<usize, String> {
    let value = crawler_specs::DOWNLOAD_CONCURRENCY
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    usize::try_from(value).map_err(|_| {
        crawler_specs::DOWNLOAD_CONCURRENCY
            .parse_error(s)
            .to_string()
    })
}

/// Keywords `--rate-limit-burst` accepts as an explicit "use the derived
/// default", i.e. `Ok(None)` rather than an error.
///
/// The set is defined by the SIBLING parser, not invented here:
/// `ConcurrencyConfig::from_str` (`domain/config.rs:388-399`) and its
/// `From<&str>` twin (`domain/config.rs:374-386`) both trim, lowercase, and
/// treat exactly one word — `auto` — plus empty as the auto-detect request.
/// Every other string either parses as a number or is rejected. So the burst's
/// accepted-keyword set is exactly `["auto"]`.
///
/// Matching that set is deliberate in BOTH directions:
///
/// - Narrower would be a fail-closed regression. `WEBFANG_RATE_LIMIT_BURST=auto`
///   and `--rate-limit-burst auto` were accepted spellings on the base, so a
///   deployment that pinned one started aborting with exit 78 on upgrade — not
///   a silent wrong answer, but a hard break of a working configuration.
/// - Wider would silently re-admit the typos #1813 exists to reject
///   (`banana` must not become "not set").
///
/// Empty/whitespace is NOT in this list: it is handled earlier in
/// [`parse_rate_limit_burst`] and is not a keyword, it is the absence of a
/// value. The one-way link between the two sets is pinned by the
/// `burst_auto_keywords_match_the_concurrency_parser` test.
const RATE_LIMIT_BURST_AUTO_KEYWORDS: [&str; 1] = ["auto"];

/// Validate `--rate-limit-burst`: explicit rate-limiter burst override
/// (`WEBFANG_RATE_LIMIT_BURST`, budget model decision Q1/D1).
///
/// This function is the SINGLE validating authority for the burst. Every
/// non-empty value is either accepted or rejected — there is no arm that
/// degrades silently to the hardware-derived default:
///
/// - Numeric values that fit `u32` and are >= 1 are accepted (the derived
///   default never produces 0, so neither may the operator).
/// - `0` is rejected with a Spanish usage error (same "Zero Silent Loss"
///   philosophy as `--download-concurrency`).
/// - A value spelled as an integer but OUT of `u32` range is rejected.
/// - The keywords in [`RATE_LIMIT_BURST_AUTO_KEYWORDS`] — that is, `auto`,
///   case-insensitively and whitespace-trimmed — are accepted as
///   `Ok(None)`: "not set", so the derived default applies. This is the same
///   spelling the sibling `--concurrency` parser understands.
/// - Any other non-numeric value is a typo and is rejected.
/// - An EMPTY or whitespace-only value means "not set" (`Ok(None)`), so
///   `WEBFANG_RATE_LIMIT_BURST=""` keeps working as a neutraliser.
///
/// # Keywords vs typos — why `auto` is accepted and `banana` is not
///
/// `auto` was rejected for a while after #1813, which was a fail-CLOSED
/// regression: `WEBFANG_RATE_LIMIT_BURST=auto` and `--rate-limit-burst auto`
/// were accepted spellings on the base, so pinning deployments began aborting
/// with exit 78 on upgrade of the binary. It is not in the same class as
/// `banana`: `auto` is a word this CLI already defines, in
/// `--concurrency`'s parser, with the same meaning it has here ("derive it
/// from the hardware"). Rejecting it fails closed on a configuration that is
/// both intentional and understood.
///
/// The two failure modes stay separated, and the separation is the whole point
/// of keeping [`RATE_LIMIT_BURST_AUTO_KEYWORDS`] a named list rather than
/// matching on "not a number": a typo silently changes the request cadence the
/// operator asked for, which is the defect #1813 exists to close, while `auto`
/// asks for exactly the cadence that runs when the flag is absent.
///
/// # Why the `Err(String)` shape
///
/// `Err(String)` already routes to `CliExit::ConfigError` (exit 78) at the
/// preflight call site (`stage_budget_overrides`), so no new error type is
/// needed and the exit code matches the `0` and out-of-range rejections exactly.
///
/// Nothing ever overflowed `governor`: `s.parse::<u32>()` fails before the
/// value reaches `Quota::allow_burst`, so this is a semantic fix, not a
/// memory-safety one.
pub(crate) fn parse_rate_limit_burst(s: &str) -> Result<Option<u32>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    // A keyword the sibling `--concurrency` parser understands means "use the
    // derived default", never a typo. ASCII case-insensitive comparison is
    // equivalent to the sibling's `trim().to_lowercase()` for this pure-ASCII
    // keyword: no non-ASCII code point lowercases into `auto`, so the two
    // spellings agree on exactly the inputs the sibling accepts.
    if RATE_LIMIT_BURST_AUTO_KEYWORDS
        .iter()
        .any(|keyword| s.eq_ignore_ascii_case(keyword))
    {
        return Ok(None);
    }
    match s.parse::<u32>() {
        Ok(0) => Err(
            "--rate-limit-burst debe ser >= 1 (0 no permite ningún request en ráfaga)".to_string(),
        ),
        Ok(v) => Ok(Some(v)),
        // Spelled as a whole number but too wide for u32: an explicit request
        // we cannot honour, so fail rather than substitute a different value.
        Err(_) if looks_like_integer(s) => Err(format!(
            "--rate-limit-burst «{s}» está fuera del rango admitido (debe estar entre 1 y {})",
            u32::MAX
        )),
        // Not a number and not a keyword (#1813): a typo. Same rejection
        // shape as the out-of-range arm so the operator sees one consistent
        // Spanish message for every invalid burst.
        Err(_) => Err(format!(
            "--rate-limit-burst «{s}» no es un número (debe estar entre 1 y {})",
            u32::MAX
        )),
    }
}

/// True when `s` is spelled as a whole number — an optional sign followed by at
/// least one ASCII digit — regardless of whether it fits any integer width.
///
/// Deliberately width-independent: the caller uses it to tell "the operator
/// typed a number" apart from "the operator typed a keyword", and a 40-digit
/// burst is still a number even though no integer type can hold it.
fn looks_like_integer(s: &str) -> bool {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Validate `--timeout-secs`: must be >= 1. A value of 0 makes wreq apply
/// `Duration::from_secs(0)` as the request timeout, so every request fails
/// instantly with "operation timed out". Rejecting here gives a clear CLI
/// error instead of a crawl where every page fails. Bounds and messages come
/// from the OptionsSpec (ADR-002).
/// Validate `--selector`: must parse as a CSS selector via `scraper`. Invalid
/// selectors currently surface only at scrape time (sometimes as a 30s network
/// timeout), so reject them up front with a clear Spanish usage error (exit 64
/// once `parse_args` maps the clap error). See issue #797.
pub(crate) fn parse_selector(s: &str) -> Result<String, String> {
    Selector::parse(s).map_err(|e| format!("selector CSS inválido «{s}»: {e}"))?;
    Ok(s.to_string())
}

/// Validate the seed URL (`--url` / positional URL) at the argv boundary
/// (#1239). The value is parsed into a hardened [`ValidUrl`] — the #675-2
/// http(s) scheme allow-list and the #675-5 credential strip apply HERE,
/// before anything can reach `--trace-file` spans, logs, or exports.
///
/// Invalid input surfaces as a Spanish clap usage error (exit 64); valid input
/// is stored already-sanitized, so downstream code can never observe
/// credentials through `CrawlOptions::url`.
pub(crate) fn parse_seed_url(s: &str) -> Result<ValidUrl, String> {
    // The ScraperError display is already the Spanish user-facing message
    // ("URL inválida: …") and clap names the offending value itself, so no
    // extra prefix is added (which would duplicate "URL inválida").
    ValidUrl::parse(s).map_err(|e| e.to_string())
}

/// Parse `--js-wait` (F-52-b, #1277): `idle` | `none` | `<ms 1..=30000>`
/// via [`PostLoadWait`](crate::domain::post_load_wait::PostLoadWait)::from_str. Invalid input surfaces as a Spanish
/// clap usage error (exit 64) naming the value and the valid forms.
pub(crate) fn parse_js_wait(s: &str) -> Result<PostLoadWait, String> {
    s.parse::<PostLoadWait>()
}

pub(crate) fn parse_timeout_secs(s: &str) -> Result<u64, String> {
    crawler_specs::TIMEOUT_SECS
        .parse_uint(s)
        .map_err(|e| e.to_string())
}

/// Validate `--download-timeout`: must be >= 1. A value of 0 makes wreq
/// apply `Duration::from_secs(0)` to every asset download, so each one
/// fails instantly. Same "Zero Silent Loss" boundary as `--timeout-secs`
/// (P4-2 of #1296). Bounds and messages come from the OptionsSpec
/// (ADR-002).
pub(crate) fn parse_download_timeout(s: &str) -> Result<u64, String> {
    crawler_specs::DOWNLOAD_TIMEOUT
        .parse_uint(s)
        .map_err(|e| e.to_string())
}

/// Validate `--max-pages`: must be >= 1. A value of 0 would panic
/// `tokio::sync::mpsc::channel(0)` inside `ResultsCollector::new`
/// (SIGABRT, #780 — the MCP path already rejects this; #598/#611 only
/// covered MCP). Rejecting here gives a clear usage error (exit 64)
/// instead of a runtime panic (exit 134). Bounds and messages come from the
/// OptionsSpec (ADR-002).
pub(crate) fn parse_max_pages(s: &str) -> Result<usize, String> {
    let value = crawler_specs::MAX_PAGES
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    usize::try_from(value).map_err(|_| crawler_specs::MAX_PAGES.parse_error(s).to_string())
}

/// Validate `--max-depth`: `0` is valid (only the seed URL is scraped);
/// values above 10 are rejected. Bounds and messages come from the
/// OptionsSpec (ADR-002 slice 4, #940) — the single validation source,
/// shared with the MCP tool schema.
pub(crate) fn parse_max_depth(s: &str) -> Result<u8, String> {
    let value = crawler_specs::MAX_DEPTH
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    u8::try_from(value).map_err(|_| crawler_specs::MAX_DEPTH.parse_error(s).to_string())
}

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
    /// [`AiArgs::max_chars_explicit`](super::AiArgs::max_chars_explicit).
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
            timeout_secs_explicit: super::chosen_by_operator(m, "timeout_secs"),
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

#[cfg(test)]
#[path = "crawler_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "crawler_spec_parity_tests.rs"]
mod spec_parity_tests;

#[cfg(test)]
#[path = "crawler_spec_deferred_surface_tests.rs"]
mod spec_deferred_surface_tests;
