use crate::domain::options_spec::crawler as crawler_specs;
use crate::domain::post_load_wait::PostLoadWait;
use crate::domain::ValidUrl;
use scraper::Selector;

#[path = "crawler_args.rs"]
mod crawler_args;

pub use crawler_args::CrawlerArgs;

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

#[cfg(test)]
#[path = "crawler_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "crawler_spec_parity_tests.rs"]
mod spec_parity_tests;

#[cfg(test)]
#[path = "crawler_spec_deferred_surface_tests.rs"]
mod spec_deferred_surface_tests;
