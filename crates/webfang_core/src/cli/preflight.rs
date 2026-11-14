//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::warn;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::config::ConfigDefaults;
use crate::domain::budget::{BudgetOverrides, BurstPermits};
use crate::domain::config_value::{ConfigSource, ConfigValue};
use crate::domain::JsStrategy;
use crate::infrastructure::observability::log_scrape_error;
use crate::{Args, CliExit, ConcurrencyConfig, ExportFormat, OutputFormat};
use std::collections::BTreeMap;
use tracing::{info, instrument};

mod field_book;
pub(crate) use field_book::FieldBook;
pub use field_book::{ArgSources, NormalizedConfig};

fn try_write<T: Clone>(
    slot: &mut ConfigValue<T>,
    incoming: T,
    incoming_source: ConfigSource,
    field: &str,
) -> bool {
    if slot.outranked_by(incoming_source) {
        info!(field = %field, winner = ?incoming_source, loser = ?slot.source, "config_field_overridden");
        *slot = ConfigValue::new(incoming, incoming_source);
        true
    } else {
        false
    }
}

#[instrument(skip_all, fields(fields_written))]
fn stage_defaults() -> FieldBook {
    FieldBook {
        max_pages: ConfigValue::new(10, ConfigSource::Default),
        max_depth: ConfigValue::new(2, ConfigSource::Default),
        sitemap_depth: ConfigValue::new(1, ConfigSource::Default),
        sitemap_url: ConfigValue::new(None, ConfigSource::Default),
        use_sitemap: ConfigValue::new(false, ConfigSource::Default),
        selector: ConfigValue::new("body".to_string(), ConfigSource::Default),
        delay_ms: ConfigValue::new(1000, ConfigSource::Default),
        jitter_ms: ConfigValue::new(0, ConfigSource::Default),
        timeout_secs: ConfigValue::new(30, ConfigSource::Default),
        concurrency: ConfigValue::new(ConcurrencyConfig::default(), ConfigSource::Default),
        output: ConfigValue::new(PathBuf::from("output"), ConfigSource::Default),
        format: ConfigValue::new(OutputFormat::Markdown, ConfigSource::Default),
        export_format: ConfigValue::new(ExportFormat::Jsonl, ConfigSource::Default),
        obsidian_tags: ConfigValue::new(Vec::new(), ConfigSource::Default),
        obsidian_wiki_links: ConfigValue::new(false, ConfigSource::Default),
        obsidian_relative_assets: ConfigValue::new(false, ConfigSource::Default),
        obsidian_rich_metadata: ConfigValue::new(false, ConfigSource::Default),
        vault: ConfigValue::new(None, ConfigSource::Default),
        quick_save: ConfigValue::new(false, ConfigSource::Default),
        ignore_waf: ConfigValue::new(false, ConfigSource::Default),
        budget_overrides: ConfigValue::new(BudgetOverrides::default(), ConfigSource::Default),
    }
}

#[instrument(skip_all, fields(fields_written))]
fn stage_config_file(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    n += stage_config_file_crawl(book, config);
    n += stage_config_file_output(book, config);
    n += stage_config_file_obsidian(book, config);
    n
}

fn stage_config_file_crawl(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(v) = config.max_pages {
        if try_write(
            &mut book.max_pages,
            v,
            ConfigSource::ConfigFile,
            "max_pages",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.delay_ms {
        if try_write(&mut book.delay_ms, v, ConfigSource::ConfigFile, "delay_ms") {
            n += 1;
        }
    }
    if let Some(v) = config.jitter_ms {
        if try_write(
            &mut book.jitter_ms,
            v,
            ConfigSource::ConfigFile,
            "jitter_ms",
        ) {
            n += 1;
        }
    }
    if let Some(ref s) = config.selector {
        if try_write(
            &mut book.selector,
            s.clone(),
            ConfigSource::ConfigFile,
            "selector",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.use_sitemap {
        if try_write(
            &mut book.use_sitemap,
            v,
            ConfigSource::ConfigFile,
            "use_sitemap",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.ignore_waf {
        if try_write(
            &mut book.ignore_waf,
            v,
            ConfigSource::ConfigFile,
            "ignore_waf",
        ) {
            n += 1;
        }
    }
    // concurrency via string tag (mirrors apply_config_defaults)
    if let Some(ref c) = config.concurrency {
        let target = ConcurrencyConfig::from(c.as_str());
        // enact rank guard even when target equals default 'auto'
        if try_write(
            &mut book.concurrency,
            target,
            ConfigSource::ConfigFile,
            "concurrency",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_config_file_output(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(ref fmt) = config.format {
        let target = match fmt.to_lowercase().as_str() {
            "json" => OutputFormat::Json,
            "text" => OutputFormat::Text,
            _ => OutputFormat::Markdown,
        };
        if try_write(&mut book.format, target, ConfigSource::ConfigFile, "format") {
            n += 1;
        }
    }
    if let Some(ref fmt) = config.export_format {
        let target = match fmt.to_lowercase().as_str() {
            "vector" => ExportFormat::Vector,
            "auto" => ExportFormat::Auto,
            _ => ExportFormat::Jsonl,
        };
        if try_write(
            &mut book.export_format,
            target,
            ConfigSource::ConfigFile,
            "export_format",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_config_file_obsidian(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(v) = config.obsidian_wiki_links {
        if try_write(
            &mut book.obsidian_wiki_links,
            v,
            ConfigSource::ConfigFile,
            "obsidian_wiki_links",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.obsidian_relative_assets {
        if try_write(
            &mut book.obsidian_relative_assets,
            v,
            ConfigSource::ConfigFile,
            "obsidian_relative_assets",
        ) {
            n += 1;
        }
    }
    if let Some(ref vault) = config.vault_path {
        if try_write(
            &mut book.vault,
            Some(PathBuf::from(vault)),
            ConfigSource::ConfigFile,
            "vault",
        ) {
            n += 1;
        }
    }
    // Rank-guarded by `try_write` alone: at this fixed pipeline position the
    // slot can only be `Default`-sourced, so value-emptiness must not gate
    // the write (spec R2 forbids value-equality merge logic).
    if let Some(ref tags_str) = config.obsidian_tags {
        let parsed: Vec<String> = tags_str
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if try_write(
            &mut book.obsidian_tags,
            parsed,
            ConfigSource::ConfigFile,
            "obsidian_tags",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_env_cli_crawl(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("max_pages") {
        if try_write(
            &mut book.max_pages,
            args.crawler.max_pages,
            src,
            "max_pages",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("max_depth") {
        if try_write(
            &mut book.max_depth,
            args.crawler.max_depth,
            src,
            "max_depth",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("delay_ms") {
        if try_write(&mut book.delay_ms, args.crawler.delay_ms, src, "delay_ms") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("jitter_ms") {
        if try_write(
            &mut book.jitter_ms,
            args.crawler.jitter_ms,
            src,
            "jitter_ms",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("timeout_secs") {
        if try_write(
            &mut book.timeout_secs,
            args.crawler.timeout_secs,
            src,
            "timeout_secs",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("selector") {
        if try_write(
            &mut book.selector,
            args.crawler.selector.clone(),
            src,
            "selector",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("concurrency") {
        if try_write(
            &mut book.concurrency,
            args.crawler.concurrency.clone(),
            src,
            "concurrency",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("sitemap_url") {
        let val = args.crawler.sitemap_url.clone();
        if try_write(&mut book.sitemap_url, val, src, "sitemap_url") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("use_sitemap") {
        if try_write(
            &mut book.use_sitemap,
            args.crawler.use_sitemap,
            src,
            "use_sitemap",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_env_cli_output(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("output") {
        if try_write(&mut book.output, args.export.output.clone(), src, "output") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("format") {
        if try_write(&mut book.format, args.export.format, src, "format") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("export_format") {
        if try_write(
            &mut book.export_format,
            args.export.export_format,
            src,
            "export_format",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_env_cli_obsidian(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("obsidian_tags") {
        if try_write(
            &mut book.obsidian_tags,
            args.obsidian.obsidian_tags.clone().unwrap_or_default(),
            src,
            "obsidian_tags",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("obsidian_wiki_links") {
        if try_write(
            &mut book.obsidian_wiki_links,
            args.obsidian.obsidian_wiki_links,
            src,
            "obsidian_wiki_links",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("obsidian_relative_assets") {
        if try_write(
            &mut book.obsidian_relative_assets,
            args.obsidian.obsidian_relative_assets,
            src,
            "obsidian_relative_assets",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("vault") {
        if try_write(&mut book.vault, args.obsidian.vault.clone(), src, "vault") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("quick_save") {
        if try_write(
            &mut book.quick_save,
            args.obsidian.quick_save,
            src,
            "quick_save",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("ignore_waf") {
        if try_write(
            &mut book.ignore_waf,
            args.crawler.ignore_waf,
            src,
            "ignore_waf",
        ) {
            n += 1;
        }
    }
    n
}

#[instrument(skip_all, fields(fields_written))]
fn stage_env_cli(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    n += stage_env_cli_crawl(book, args, sources);
    n += stage_env_cli_output(book, args, sources);
    n += stage_env_cli_obsidian(book, args, sources);
    n
}

/// Field-wise merge of budget overrides at the binary's final assembly
/// point (#897 item 1).
///
/// * `cli_capture` — the CLI-explicit knobs captured from `From<Args>`
///   BEFORE provenance projection (unranked by construction).
/// * `staged` — the pipeline-projected overrides (`into_crawl_options`),
///   carrying the WINNING source per the `Default < ConfigFile <
///   Environment < Cli` resolution upstream.
///
/// Tier rules:
/// - `crawl`: **staged wins; the CLI capture only fills the gap.** Both
///   sides project the SAME user knob (`--concurrency` / TOML), but
///   only `staged` is rank-resolved. A blind `cli.or(staged)` here let a
///   plain `--concurrency` stomp a rank-resolved staged value while
///   `network.concurrency` kept the ranked winner — a silent
///   contradiction inside one struct (adversarial review M1 of PR #925).
///   Preferring `staged` keeps `budget_overrides.crawl` consistent with
///   `network.concurrency` BY CONSTRUCTION.
/// - `rate_burst`: since #1813 `From<Args>` contributes `None`, so on the
///   shipped binary's path the staged value is the only possible source —
///   `stage_budget_overrides` writes it at ConfigFile/Env/CommandLine rank.
///   The `cli_capture` side remains reachable for programmatic `Args`
///   callers that populate a capture themselves; `cli_capture` wins there.
/// - `batch` / `asset`: CLI-explicit wins where present; the staged value
///   fills the rest (nothing stages them today).
#[must_use]
pub fn merge_budget_overrides(
    cli_capture: crate::domain::budget::BudgetOverrides,
    staged: crate::domain::budget::BudgetOverrides,
) -> crate::domain::budget::BudgetOverrides {
    crate::domain::budget::BudgetOverrides {
        crawl: staged.crawl.or(cli_capture.crawl),
        rate_burst: cli_capture.rate_burst.or(staged.rate_burst),
        batch: cli_capture.batch.or(staged.batch),
        asset: cli_capture.asset.or(staged.asset),
    }
}

/// Stage operator budget overrides (task 2.1, design D4).
///
/// Runs AFTER [`apply_cross_field_rules`] and BEFORE [`validate_stage`] so the
/// existing rank-guarded stages keep their stage ordering untouched. TOML is
/// staged first at `ConfigFile` rank, then env/cli via the provenance map.
#[instrument(skip_all, fields(fields_written))]
fn stage_budget_overrides(
    book: &mut FieldBook,
    args: &Args,
    sources: &ArgSources,
    config: &ConfigDefaults,
) -> Result<usize, CliExit> {
    let mut n = 0;
    // TOML tier first (lowest explicit rank).
    if let Some(v) = config.rate_limit_burst {
        if try_write(
            &mut book.budget_overrides,
            budget_override(v)?,
            ConfigSource::ConfigFile,
            "rate_limit_burst",
        ) {
            n += 1;
        }
    }
    // env/cli tier via the provenance map. The raw string is parsed here so
    // CLI, env, and programmatic input share ONE accept / reject semantic
    // (`parse_rate_limit_burst`). This is the ONLY place the burst is
    // validated: `From<Args>` no longer parses it (#1813), so an invalid
    // value from argv, the env var, or TOML stops here with exit 78 before
    // the conversion runs.
    if let Some(src) = sources.source_of("rate_limit_burst") {
        if let Some(raw) = args.crawler.rate_limit_burst.as_deref() {
            match crate::cli::args::crawler::parse_rate_limit_burst(raw) {
                Ok(Some(v)) => {
                    if try_write(
                        &mut book.budget_overrides,
                        budget_override(v)?,
                        src,
                        "rate_limit_burst",
                    ) {
                        n += 1;
                    }
                },
                // Empty / whitespace-only, or the `auto` keyword: the
                // operator neutralised the flag (`WEBFANG_RATE_LIMIT_BURST=""`
                // or `=auto`), so it means "not set" and the derived default
                // applies. Never a silent degrade of an invalid value —
                // those are rejected below (#1813).
                Ok(None) => {},
                Err(msg) => return Err(CliExit::ConfigError(msg)),
            }
        }
    }
    Ok(n)
}

/// Wrap a raw burst value into [`BudgetOverrides`], rejecting 0 with the same
/// Spanish boundary error the clap value parser uses (defense for programmatic
/// `Args` construction and TOML values, which bypass the CLI parser).
fn budget_override(v: u32) -> Result<BudgetOverrides, CliExit> {
    let rate_burst = BurstPermits::new(v).map_err(|_| {
        CliExit::ConfigError(
            "--rate-limit-burst debe ser >= 1 (0 no permite ningún request en ráfaga)".to_string(),
        )
    })?;
    Ok(BudgetOverrides {
        rate_burst: Some(rate_burst),
        crawl: None,
        batch: None,
        asset: None,
    })
}
#[instrument(skip_all, fields(fields_written))]
fn apply_cross_field_rules(book: &mut FieldBook) -> usize {
    let mut n = 0;
    // --sitemap-url implies --use-sitemap (#491): preserved verbatim
    if book.sitemap_url.value.is_some() && !book.use_sitemap.value {
        // keep the higher of the two provenances for auditability
        let src = std::cmp::max(book.sitemap_url.source, book.use_sitemap.source);
        let loser = book.use_sitemap.source;
        book.use_sitemap = ConfigValue::new(true, src);
        info!(field = "use_sitemap", winner = ?src, loser = ?loser, "config_field_overridden");
        n += 1;
    }
    // Obsidian tag trim/dedup preserved verbatim inside apply_cross_field_rules
    // Trim whitespace from each tag, drop empties, preserve order, dedup?
    let tags = &mut book.obsidian_tags.value;
    for tag in tags.iter_mut() {
        *tag = tag.trim().to_string();
    }
    tags.retain(|t| !t.is_empty());
    // byte-parity: original code did not dedup across retain, but spec says
    // dedup. We preserve order and dedup via first-occurrence retention to match
    // the spec's "byte-parity" expectation while staying deterministic.
    {
        let mut seen = std::collections::BTreeSet::new();
        let mut deduped = Vec::new();
        for t in tags.drain(..) {
            if seen.insert(t.clone()) {
                deduped.push(t);
            }
        }
        *tags = deduped;
    }
    n
}

#[instrument(skip_all, fields(fields_written))]
fn validate_stage(book: &FieldBook) -> Result<(), CliExit> {
    if book.max_pages.value == 0 {
        let msg = "max_pages debe ser mayor que 0".to_string();
        log_scrape_error(&msg, "", "validate_stage", None, "validation failed");
        return Err(CliExit::ConfigError(msg));
    }
    // Additional per-field validation could be added here without growing this
    // function's complexity beyond the ratchet (keep <30).
    Ok(())
}

/// Normalize configuration through the single rank-guarded pipeline (design D3).
///
/// Fixed stage order: defaults → config file → env/cli → cross-field →
/// budget overrides → field validation → capability gates → `NormalizedConfig`.
///
/// # Capability gates are part of the pipeline, not a caller's job
///
/// The last step is the `--adaptive-selectors` capability gate (#1813,
/// `check_adaptive_selectors_capability`). It is HERE, at the end, rather than
/// in the CLI binary's step list, for the reason `stage_budget_overrides` is
/// here: a check a caller must remember to invoke is a check some front door
/// will skip. Every route into this function — argv,
/// `WEBFANG_ADAPTIVE_SELECTORS`, or a programmatically built `Args` —
/// passes through it, so there is exactly one place where "this build cannot
/// do what you asked" is decided.
///
/// It runs LAST on purpose: `validate_stage` keeps the field errors it has
/// always reported, so a run that is already misconfigured for another reason
/// still sees that other reason first. The gate itself is still far ahead of
/// anything that matters — `main.rs` calls `normalize` at step 6b, before
/// logging, before any engine construction, and long before `build_and_run`
/// opens a socket.
///
/// # Errors
///
/// Returns `CliExit::ConfigError` with a Spanish message when validation
/// blocks the result.
#[allow(missing_docs)]
#[instrument(skip_all, fields(fields_written = tracing::field::Empty))]
pub fn normalize(
    args: &Args,
    sources: &ArgSources,
    config: &ConfigDefaults,
) -> Result<NormalizedConfig, CliExit> {
    let mut book = stage_defaults();
    let mut total = 0usize;
    total += stage_config_file(&mut book, config);
    total += stage_env_cli(&mut book, args, sources);
    total += apply_cross_field_rules(&mut book);
    total += stage_budget_overrides(&mut book, args, sources, config)?;
    validate_stage(&book)?;
    check_adaptive_selectors_capability(args)?;
    tracing::Span::current().record("fields_written", total);
    info!(fields_written = total, "normalization_complete");
    Ok(NormalizedConfig::from_book(book))
}

// ============================================================================
// JS strategy dependency preflight (#685)
// ============================================================================

/// Chrome binary candidates probed for `--js-strategy full` (#685).
///
/// The audit spec named `google-chrome` only, but Linux distributions ship
/// the engine under several names, so one missing distro binary must not
/// mask an installed Chrome. Order matters: the first binary that reports a
/// version wins.
///
/// Windows/macOS use their own candidate sets ([`default_chrome_candidates`],
/// XP-S-04 #1608), so this list is only compiled on the platforms it serves.
#[cfg(not(any(windows, target_os = "macos")))]
const DEFAULT_CHROME_CANDIDATES: [&str; 4] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium-browser",
    "chromium",
];

/// Chrome/Chromium candidates for the current platform (XP-S-04, #1608).
///
/// Order matters: the first candidate that resolves to an existing file
/// reporting a `--version` wins.
///
/// - Linux: bare names resolved through `PATH` (list unchanged from #685).
/// - Windows: the standard `chrome.exe` install locations first (Program
///   Files, Program Files (x86), then the per-user `%LOCALAPPDATA%`
///   install), then the bare `chrome` name through `PATH`/`PATHEXT`.
/// - macOS: the `.app` bundle executables, plus bare `chromium` for
///   Homebrew-style installs.
///
/// Injectability is preserved: [`resolve_chrome_binary_with`] and
/// [`check_js_dependencies_with`] still take the candidate list as a
/// parameter — only the production default is platform-conditional.
fn default_chrome_candidates() -> Vec<String> {
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        DEFAULT_CHROME_CANDIDATES
            .iter()
            .map(|s| (*s).into())
            .collect()
    }
    #[cfg(windows)]
    {
        let mut candidates: Vec<String> = Vec::new();
        for base in [
            r"C:\Program Files\Google\Chrome\Application",
            r"C:\Program Files (x86)\Google\Chrome\Application",
        ] {
            candidates.push(format!(r"{base}\chrome.exe"));
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let per_user = std::path::PathBuf::from(local_app_data)
                .join(r"Google\Chrome\Application\chrome.exe");
            candidates.push(per_user.to_string_lossy().into_owned());
        }
        // Bare name last: Chrome rarely registers on PATH, but the
        // PATH/PATHEXT search is free to try.
        candidates.push("chrome".to_string());
        candidates
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
            "chromium".to_string(),
        ]
    }
}

/// Resolve the Chrome/Chromium binary the gate certified (F-52-c, #1278).
///
/// Returns the first candidate (in order) that resolves to an existing
/// *file* and whose `--version` probe exits successfully. Bare names are
/// resolved through the injected `PATH` exactly like the OS would
/// ([`resolve_executable_in_path`]); explicit paths must exist as files.
/// The `is_file` requirement (not `exists`) excludes the
/// `/opt/google/chrome` *directory* trap that chromiumoxide's own
/// `get_by_path` falls into via `exists()`.
///
/// Pure and injectable like the rest of the gate (#787): tests pass
/// controlled candidates + `PATH` instead of touching process-global env.
pub(crate) fn resolve_chrome_binary_with(candidates: &[&str], path_value: &str) -> Option<PathBuf> {
    candidates.iter().find_map(|binary| {
        let resolved = if has_path_separator(binary) {
            let path = PathBuf::from(binary);
            path.is_file().then_some(path)
        } else {
            resolve_executable_in_path(binary, path_value)
        };
        resolved.filter(|path| binary_path_reports_version(path))
    })
}

/// Process-PATH entry point for `resolve_chrome_binary_with`: the
/// production resolution over the platform default candidates
/// the platform default candidates (`default_chrome_candidates`). Called once
/// after the gate passes
/// (`main.rs` 6c); the result travels in `CrawlOptions.network.chrome_binary`
/// so the launcher runs exactly the certified binary.
pub fn resolve_chrome_binary() -> Option<PathBuf> {
    let path_value =
        std::env::var_os("PATH").map_or_else(String::new, |v| v.to_string_lossy().into_owned());
    let candidates = default_chrome_candidates();
    let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    resolve_chrome_binary_with(&candidate_refs, &path_value)
}

/// Upper bound for one `--version` probe (XP-S-06, #1608).
///
/// The probe runs synchronously, pre-crawl; a wedged or interactive binary
/// must not hang the CLI forever. On expiry the child is killed and the
/// probe reports failure (the Full gate then goes red; the Hybrid Obscura
/// check degrades to "unknown version" with a warning).
const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Poll interval while waiting for a `--version` probe to exit.
const VERSION_PROBE_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// Run `<path> --version` silently and bounded (XP-S-06, #1608).
///
/// Returns the process `Output` (status + captured stdout/stderr) or `None`
/// when the spawn fails, the wait errors, or the deadline elapses first
/// (the child is killed and reaped before returning). The probe probes the
/// exact file the launcher will execute, so gate-certified and launched are
/// the same binary by construction; output is captured because the Obscura
/// version check parses it.
fn run_version_probe_with_timeout(
    path: &Path,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                use std::io::Read as _;
                // The child has exited: the pipes are closed on its side, so
                // read_to_end drains the buffered output without blocking.
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_end(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_end(&mut stderr);
                }
                return Some(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            },
            Ok(None) => {},
            Err(_) => return None,
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(VERSION_PROBE_POLL);
    }
}

/// Silent `--version` probe against an already-resolved file path, bounded
/// by [`VERSION_PROBE_TIMEOUT`].
///
/// Output is discarded — only the exit status matters.
fn binary_path_reports_version(path: &Path) -> bool {
    matches!(
        run_version_probe_with_timeout(path, VERSION_PROBE_TIMEOUT),
        Some(output) if output.status.success()
    )
}

/// Preflight: verify the local environment can satisfy the configured JS
/// strategy before any crawl starts (#685, #758, #787).
///
/// Strategy-specific checks:
///
/// - [`Static`](crate::domain::JsStrategy::Static): no external binary is
///   needed — returns immediately without touching disk or spawning
///   processes.
/// - [`Hybrid`](crate::domain::JsStrategy::Hybrid) (#787, #793): Layer 2 shells
///   out to Obscura (`ObscuraDownloader`), so the configured
///   `--obscura-binary` (default `obscura`) must exist. A value with a path
///   separator must exist as a file; a bare name must resolve via `PATH`.
///   This turns a missing binary into a clean config error (exit 78)
///   instead of a silent Layer-2 fallback mid-crawl. Once resolved, the
///   binary must report a version of at least `MINIMUM_OBSCURA_VERSION`
///   (#793): an older dump format would silently feed the wrong content
///   shape to Layer 2. A failing or unreadable `--version` probe degrades
///   to a warning (best-effort for unknown builds).
/// - [`Full`](crate::domain::JsStrategy::Full) renders pages through
///   Chromiumoxide, which spawns a real Chrome/Chromium binary. Two
///   independent preconditions are checked, in order:
///
///   1. **Compile-time capability** (#758): the binary must have been built
///      with the `chromium` feature. Without it, `ChromiumoxideDownloader`
///      is a stub that fails mid-crawl with an instruction the CLI binary
///      cannot follow. This is a build-configuration error, checked first
///      because probing the PATH is pointless when the binary cannot render
///      JS at all.
///   2. **Runtime environment**: probing the installed candidates turns a
///      missing browser into a clean config error (exit 78) instead of a
///      confusing mid-crawl browser launch failure.
///
/// Runs once, in a synchronous context, before crawl start — a brief
/// blocking `--version` probe (Full and Hybrid) is acceptable there.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when the strategy is
/// [`Hybrid`](crate::domain::JsStrategy::Hybrid) and the configured Obscura
/// binary does not exist (neither as a path nor on `PATH`) or reports a
/// version older than `MINIMUM_OBSCURA_VERSION` (#793), or when it is
/// [`Full`](crate::domain::JsStrategy::Full) and either the binary was
/// built without the `chromium` feature, or no Chrome/Chromium candidate
/// on `PATH` reports a version.
pub fn check_js_dependencies(opts: &CrawlOptions) -> Result<(), CliExit> {
    // The PATH value is injected so the core check stays pure and testable —
    // tests pass a controlled PATH instead of mutating process-global env.
    let path_value =
        std::env::var_os("PATH").map_or_else(String::new, |v| v.to_string_lossy().into_owned());
    let candidates = default_chrome_candidates();
    let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    check_js_dependencies_with(
        &candidate_refs,
        cfg!(feature = "chromium"),
        &path_value,
        opts,
    )
}

/// Candidate-, feature-, and PATH-injectable core of
/// [`check_js_dependencies`] — Full/Chrome tests use fake executables in a
/// controlled PATH dir (F-52-c), Hybrid tests do the same for obscura.
/// inject the feature flag because `cfg!` cannot be toggled per test, and
/// inject the `PATH` value so the Hybrid Obscura lookup never races with
/// concurrent tests over the process-global environment (#787).
fn check_js_dependencies_with(
    candidates: &[&str],
    chromium_enabled: bool,
    path_value: &str,
    opts: &CrawlOptions,
) -> Result<(), CliExit> {
    match opts.network.js_strategy {
        // Static crawls with wreq only — no external binary to check.
        JsStrategy::Static => Ok(()),
        // Hybrid escalates through Obscura: fail fast when the binary is
        // missing instead of letting Layer 2 fail in the middle of the crawl
        // (#787).
        JsStrategy::Hybrid => check_obscura_binary(&opts.network.obscura_binary, path_value),
        JsStrategy::Full => check_chrome_binary(candidates, chromium_enabled, path_value),
    }
}

/// Full-strategy check (#685, #758, F-52-c #1278): `chromium` feature + a
/// Chrome/Chromium candidate that reports a version. The gate now resolves
/// through [`resolve_chrome_binary_with`] — the same resolution the launcher
/// will use — so a green gate means the certified binary is launchable.
fn check_chrome_binary(
    candidates: &[&str],
    chromium_enabled: bool,
    path_value: &str,
) -> Result<(), CliExit> {
    if !chromium_enabled {
        return Err(CliExit::ConfigError(
            "--js-strategy full requiere un binario compilado con la feature `chromium`; \
                     recompilá con --features chromium o usá --js-strategy hybrid"
                .into(),
        ));
    }

    let chrome_present = resolve_chrome_binary_with(candidates, path_value).is_some();
    if chrome_present {
        tracing::info!(strategy = "full", "chrome_dependency_checked");
        return Ok(());
    }

    Err(CliExit::ConfigError(
        "--js-strategy full requiere Google Chrome instalado".into(),
    ))
}

/// Whether `binary` names an explicit path (absolute or relative) rather
/// than a bare executable name resolved from `PATH`.
fn has_path_separator(binary: &str) -> bool {
    binary.contains('/') || binary.contains('\\')
}

/// Semicolon-separated extension list tried after the bare name on Windows
/// (XP-S-04, #1608): the cmd.exe default `PATHEXT` value.
#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Parse a `PATHEXT`-style semicolon-separated extension list into ordered,
/// normalized extensions. Pure — unit-tested without process env.
///
/// Preserves order, drops empty entries, lowercases, and ensures each entry
/// starts with a dot (cmd.exe accepts both `EXE` and `.EXE` spellings).
fn parse_pathext(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            if ext.starts_with('.') {
                ext
            } else {
                format!(".{ext}")
            }
        })
        .collect()
}

/// The platform's `PATHEXT` value, when the platform uses one.
///
/// Windows: the `PATHEXT` env var (or the cmd.exe default when unset).
/// Everywhere else: `None` — a bare name is tried exactly as given.
#[cfg(windows)]
fn platform_pathext() -> Option<String> {
    Some(std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_string()))
}

#[cfg(not(windows))]
fn platform_pathext() -> Option<String> {
    None
}

/// Candidate file names for a bare executable name, in resolution order.
///
/// The exact name first, then — on Windows only — the name with each
/// `PATHEXT` extension appended, in `PATHEXT` order (XP-S-04, #1608). Pure
/// and unit-testable on every platform: unix callers pass `None`.
fn bare_name_candidates(name: &str, pathext: Option<&str>) -> Vec<String> {
    let mut names = vec![name.to_string()];
    if let Some(exts) = pathext {
        names.extend(
            parse_pathext(exts)
                .into_iter()
                .map(|ext| format!("{name}{ext}")),
        );
    }
    names
}

/// First existing *file* among `dir/name` combinations, dir-major order
/// (every candidate name is tried within a directory before moving to the
/// next — the same order the OS uses). Pure given the inputs; the only
/// effect is the `is_file` probe, unit-tested on Linux.
fn first_existing_in_dirs(
    mut dirs: impl Iterator<Item = PathBuf>,
    names: &[String],
) -> Option<PathBuf> {
    dirs.find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Scan `PATH` entries (in order) for a file named `name` — the same lookup
/// the OS performs for a bare executable name (#787).
///
/// Pure: takes the `PATH` value as input so tests control the search space
/// without touching the process-global environment. On Windows the bare
/// name is expanded with the platform's `PATHEXT` order (XP-S-04, #1608).
fn resolve_executable_in_path(name: &str, path_value: &str) -> Option<PathBuf> {
    if name.is_empty() || has_path_separator(name) {
        return None;
    }
    let names = bare_name_candidates(name, platform_pathext().as_deref());
    first_existing_in_dirs(std::env::split_paths(path_value), &names)
}

/// Resolve the configured Obscura binary to an existing file.
///
/// A value with a path separator must exist as a file exactly as given; a
/// bare name must resolve through `PATH` (#787).
fn resolve_obscura_binary(binary: &str, path_value: &str) -> Option<PathBuf> {
    if has_path_separator(binary) {
        PathBuf::from(binary)
            .is_file()
            .then(|| PathBuf::from(binary))
    } else {
        resolve_executable_in_path(binary, path_value)
    }
}

/// Hybrid-strategy check (#787): the configured Obscura binary exists.
fn check_obscura_binary(binary: &str, path_value: &str) -> Result<(), CliExit> {
    match resolve_obscura_binary(binary, path_value) {
        Some(resolved) => check_obscura_version(binary, &resolved),
        None if has_path_separator(binary) => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere el binario obscura: la ruta \"{binary}\" no existe \
             o no es un archivo; verificá --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
        None => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere el binario \"{binary}\": no se encontró en PATH; \
             instalalo o configurá una ruta con --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
    }
}

/// Minimum Obscura version for the Layer 2 contract (#793, #1710).
///
/// Two floors, one constant. The dump-format contract: `obscura fetch --dump
/// html` was verified in 0.2.0 and did not change through 0.2.3, so an older
/// binary may change dump semantics and silently feed the wrong content shape
/// to Layer 2. And the upstream security fixes a scraper that fetches
/// adversarial URLs cannot sit below: 0.2.2 closed SSRF gaps (IPv6 embedded /
/// 6to4 / NAT64, CGNAT, IANA ranges) and 0.2.3 hardened cookies (host-only,
/// public-suffix, SameSite), stripped credentials from cross-origin redirects
/// and updated rustls (RUSTSEC-2026-0285).
///
/// The floor is 0.2.5, the current stable upstream release.
const MINIMUM_OBSCURA_VERSION: (u64, u64, u64) = (0, 2, 5);

/// User-facing spelling of [`MINIMUM_OBSCURA_VERSION`] for Spanish errors.
const MINIMUM_OBSCURA_VERSION_STR: &str = "0.2.5";

/// Verdict of the Obscura version assessment (#793).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionVerdict {
    /// Parsed version is >= the minimum.
    Meets,
    /// Parsed version is below the minimum.
    TooOld,
    /// Probe failed or output was unparseable — degrade, do not block.
    Unknown,
}

/// Parse a pure MAJOR.MINOR.PATCH token into a semantic triple.
///
/// Tolerates a leading `v`/`V`, a `-`/`+` suffix on the patch segment
/// (`0.2.0-rc.1`, `0.2.0+build`), and extra dotted segments (ignored).
fn parse_version_token(token: &str) -> Option<(u64, u64, u64)> {
    let cleaned = token.strip_prefix(['v', 'V']).unwrap_or(token);
    let mut parts = cleaned.splitn(4, '.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?.split(['-', '+']).next()?;
    Some((
        major.parse().ok()?,
        minor.parse().ok()?,
        patch.parse().ok()?,
    ))
}

/// Extract the first semver-like version from raw `--version` output (#793).
///
/// Pure: no process or environment access — unit-tested without mutation.
fn parse_obscura_version(output: &str) -> Option<(u64, u64, u64)> {
    output.split_whitespace().find_map(parse_version_token)
}

/// Classify an optional parsed version against the minimum (#793). Pure.
fn assess_obscura_version(parsed: Option<(u64, u64, u64)>) -> VersionVerdict {
    match parsed {
        Some(version) if version >= MINIMUM_OBSCURA_VERSION => VersionVerdict::Meets,
        Some(_) => VersionVerdict::TooOld,
        None => VersionVerdict::Unknown,
    }
}

/// Run `<resolved> --version` once and parse its output (#793), bounded by
/// [`VERSION_PROBE_TIMEOUT`] (XP-S-06, #1608).
///
/// Returns `None` when the probe cannot run, is killed at the deadline,
/// exits non-zero, or prints no semver-like token on stdout or stderr.
/// Blocking spawn: acceptable once, pre-crawl (same silent-probe shape the
/// Full gate uses per resolved file).
fn probe_obscura_version(resolved: &std::path::Path) -> Option<(u64, u64, u64)> {
    let output = run_version_probe_with_timeout(resolved, VERSION_PROBE_TIMEOUT)?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_obscura_version(&stdout)
        .or_else(|| parse_obscura_version(&String::from_utf8_lossy(&output.stderr)))
}

/// Spanish operator-facing text of the `Unknown` Obscura version verdict
/// (#1710).
///
/// A verdict that says only "the version could not be determined" is one the
/// operator cannot act on: the floor the guard enforces
/// ([`MINIMUM_OBSCURA_VERSION_STR`]) is the whole point of the check, so the
/// warning has to NAME it. The text is derived from the constant, so the
/// message can never drift from the floor the guard actually applies.
///
/// The verdict itself does not change: an unreadable build stays a
/// DEGRADATION — the crawl and the release flow do not stop. Only the
/// CONTENT of the warning changes.
fn unknown_obscura_version_warning() -> String {
    format!(
        "no se pudo determinar la versión de obscura; se esperaba al menos \
         {MINIMUM_OBSCURA_VERSION_STR} (la sonda --version falló o su salida no es \
         interpretable) — se continúa en modo best-effort"
    )
}

/// Version half of the Hybrid check (#793): enforce the minimum contract on
/// a resolved binary. An unreadable build degrades to a warning (unknown
/// custom builds must not be hard-blocked); a parseable older version fails
/// fast with exit 78.
fn check_obscura_version(binary: &str, resolved: &std::path::Path) -> Result<(), CliExit> {
    let parsed = probe_obscura_version(resolved);
    let version = parsed.map_or_else(
        || "unknown".to_string(),
        |(major, minor, patch)| format!("{major}.{minor}.{patch}"),
    );

    match assess_obscura_version(parsed) {
        VersionVerdict::Meets => {
            tracing::info!(
                strategy = "hybrid",
                binary = %binary,
                resolved = %resolved.display(),
                version = %version,
                "obscura_dependency_checked"
            );
            Ok(())
        },
        VersionVerdict::Unknown => {
            warn!(
                strategy = "hybrid",
                binary = %binary,
                resolved = %resolved.display(),
                expected_version = MINIMUM_OBSCURA_VERSION_STR,
                user_message = %unknown_obscura_version_warning(),
                "obscura_version_unreadable: --version probe failed or unparseable — \
                 continuing best-effort"
            );
            Ok(())
        },
        VersionVerdict::TooOld => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere obscura {MINIMUM_OBSCURA_VERSION_STR} o superior: \
             el binario \"{binary}\" reporta la versión {version}; actualizalo o cambiá la \
             ruta con --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
    }
}

/// Preflight: `--elastic` must have at least one wirable vector sink (#695).
///
/// `--elastic` and `--output-vectors` are orthogonal vector destinations
/// (#636): the SQLite sink only exists under the `persistence` feature,
/// while the JSONL stream sink is available in every build. Without
/// `persistence` AND without `--output-vectors`, `--elastic` would wire no
/// sink at all and the run would silently report success with no artifact.
///
/// Fail fast instead: an explicit request the binary cannot honor is a
/// configuration error (exit 78), never a silent no-op.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when `--elastic` is
/// enabled, no `--output-vectors` path was given, and the binary was built
/// without the `persistence` feature.
pub fn check_elastic_sink(opts: &CrawlOptions) -> Result<(), CliExit> {
    if !opts.elastic.enabled {
        return Ok(());
    }
    if opts.elastic.output_vectors.is_some() {
        return Ok(());
    }
    if cfg!(feature = "persistence") {
        return Ok(());
    }
    Err(CliExit::ConfigError(
        "--elastic requiere un destino de vectores: este binario fue compilado sin la \
             feature `persistence` (sink SQLite); use --output-vectors <ruta> o un binario \
             compilado con persistencia"
            .into(),
    ))
}

/// Orphan-flag guard (#799): `--db-path` is only consumed by the elastic
/// ingestion pipeline (`build_elastic_ingestion`), so without `--elastic` it is a
/// silent no-op -- the run "succeeds" but persists nothing. Reject it up front
/// with a clear message instead of letting the user lose data unknowingly.
pub fn check_db_path_requires_elastic(opts: &CrawlOptions) -> Result<(), CliExit> {
    if opts.elastic.db_path.is_some() && !opts.elastic.enabled {
        return Err(CliExit::ConfigError(
            "--db-path solo tiene efecto junto con --elastic; ejecuta con --elastic para \
                 persistir en la base de datos SQLite, o quita --db-path"
                .into(),
        ));
    }
    Ok(())
}

/// The single Spanish message every `--adaptive-selectors` capability
/// rejection emits (#1813).
///
/// It names both the flag and the feature, because the actionable fix is a
/// recompile with `--features adaptive-selectors`, not a different argument.
/// Shared by both call shapes so the two can never drift into telling the
/// operator two different things about the same missing capability.
const ADAPTIVE_SELECTORS_UNAVAILABLE: &str = "--adaptive-selectors requiere un binario compilado con la feature `adaptive-selectors`; recompilá con --features adaptive-selectors";

/// The ONE semantic behind every `--adaptive-selectors` capability check
/// (#1813): a requested capability the build cannot honour is a
/// build-configuration problem (exit 78), never a silent no-op.
///
/// Both entry shapes funnel here — [`check_adaptive_selectors_capability_with`]
/// reads the request off `Args` during [`normalize`], and
/// [`check_adaptive_selectors_feature_with`] reads it off `CrawlOptions` for a
/// programmatic builder — so the accept/reject decision is stated once and the
/// only thing a caller chooses is where the flag lives.
fn adaptive_selectors_unavailable(adaptive_enabled: bool, requested: bool) -> Result<(), CliExit> {
    if !requested || adaptive_enabled {
        return Ok(());
    }
    Err(CliExit::ConfigError(
        ADAPTIVE_SELECTORS_UNAVAILABLE.to_string(),
    ))
}

/// Preflight: `--adaptive-selectors` on a binary built WITHOUT the
/// `adaptive-selectors` feature must fail before any network request (#1813).
///
/// Direct sibling of the `--elastic`/`persistence` gate
/// ([`check_elastic_sink`]): an explicitly
/// requested capability the binary was not compiled to honor is a
/// build-configuration problem, never a silent no-op. Before this gate the
/// flag survived argv parsing as the hidden compatibility placeholder built in
/// `cli::spec_command`, `From<Args>` carried `true` into
/// [`CrawlOptions::adaptive_selectors`], and `build_adaptive_engine`
/// (`webfang_cli`, `#[cfg(feature = "adaptive-selectors")]`) was compiled out
/// entirely — so the run reached the network and exited 0 having done nothing.
///
/// The placeholder is deliberately KEPT: it is the parse half of this
/// two-part gate. Deleting it would turn a named, actionable build error into
/// clap's "unexpected argument" (exit 64) with no mention of the feature.
///
/// The [`CrawlOptions`]-shaped half is kept for a programmatic caller that
/// builds options directly. The shipped CLI does NOT use it — it goes through
/// `check_adaptive_selectors_capability`, which runs inside [`normalize`].
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when
/// `adaptive_selectors` is requested and the `adaptive-selectors` feature is
/// not compiled in.
pub fn check_adaptive_selectors_feature(opts: &CrawlOptions) -> Result<(), CliExit> {
    check_adaptive_selectors_feature_with(cfg!(feature = "adaptive-selectors"), opts)
}

/// Feature-injectable core of [`check_adaptive_selectors_feature`] — `cfg!`
/// is a compile-time constant and cannot be toggled per test.
fn check_adaptive_selectors_feature_with(
    adaptive_enabled: bool,
    opts: &CrawlOptions,
) -> Result<(), CliExit> {
    adaptive_selectors_unavailable(adaptive_enabled, opts.adaptive_selectors)
}

/// #1813 — the SAME capability check, reached from the SHARED staging
/// pipeline instead of from one call site in the CLI binary.
///
/// ## Why it moved out of `main.rs`
///
/// T1 established the rule this follows: a validation that only one front door
/// runs is not a validation, it is a suggestion. `--rate-limit-burst` went into
/// [`stage_budget_overrides`] precisely so argv, env, TOML and programmatic
/// `Args` all share ONE accept/reject semantic. This gate was wired straight
/// into `webfang_cli/src/main.rs` step 6e2 instead, so it was reachable from
/// exactly one caller — and `main.rs:116` (`normalize`) runs long before
/// `main.rs:220` (the gate), which is the ordering proof that every `Args`
/// front door passes through the staging pipeline on its way to the binary.
///
/// ## Why `Args`, not `CrawlOptions`
///
/// `normalize` stages [`Args`]; `CrawlOptions` does not exist yet at that point
/// (it is built from `Args` afterwards, `main.rs:121`). The request itself is
/// unambiguous — `From<Args>` copies `args.crawler.adaptive_selectors` verbatim
/// into [`CrawlOptions::adaptive_selectors`], so the two can never disagree —
/// and `ConfigDefaults::adaptive_selectors` (`domain/config.rs:98`) is declared
/// but read by NOTHING, so TOML is not a fourth front door for this flag.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when the request is
/// present and the feature is not compiled in — the same value the
/// `CrawlOptions`-shaped check returns, from
/// [`ADAPTIVE_SELECTORS_UNAVAILABLE`].
///
/// # Deliberately independent of [`ArgSources`]
///
/// Every field the `FieldBook` stages is rank-guarded: it lands only when
/// provenance recorded it, so a value present in `Args` without a recorded
/// source never reaches the book and the derived default wins. This check is
/// NOT rank-guarded and deliberately reads `Args` directly. An unranked
/// boolean that arrives from argv or from the env var carries no ambiguity to
/// resolve — there is no lower-ranked value it could lose to — and making the
/// gate depend on a provenance entry would mean a front door that forgot to
/// record one silently regains the fail-open this is meant to close.
fn check_adaptive_selectors_capability(args: &Args) -> Result<(), CliExit> {
    check_adaptive_selectors_capability_with(cfg!(feature = "adaptive-selectors"), args)
}

/// Feature-injectable core of [`check_adaptive_selectors_capability`] — the
/// #761/#796 injection seam, so the negative case is runnable in the
/// `--all-features` lane that CI uses.
fn check_adaptive_selectors_capability_with(
    adaptive_enabled: bool,
    args: &Args,
) -> Result<(), CliExit> {
    adaptive_selectors_unavailable(adaptive_enabled, args.crawler.adaptive_selectors)
}

/// Preflight: `--export-format vector` without `--clean-ai` must fail before
/// any network request (#796).
///
/// Mirrors the #703/#652 `output_vectors_gate` in the orchestrator: without
/// `--clean-ai` there are no embeddings, so the vector exporter would write an
/// invalid `export.json` (`dimensions: null`, `model_name: null`, documents
/// without `embeddings`) while still reporting success (exit 0). An explicit
/// request the binary cannot honor is never a silent no-op: it is a
/// data-format error (exit 65). `ExportFormat::Jsonl` (the default) and
/// `ExportFormat::Auto` are never gated.
///
/// # Errors
///
/// Returns [`crate::CliExit::DataFormatError`] (exit 65) when the export
/// format is `Vector` and `--clean-ai` was not given.
///
/// # Why there is no longer a build-capability arm
///
/// This gate used to answer twice: once for a build that carried the local
/// inference stack, once for a build that did not — the second arm told the
/// operator to compile a configuration that no longer exists, an instruction
/// nobody can follow. ADR-0004 paso 6 removed that local stack, so there is
/// exactly ONE question left ("did the operator ask for vectors without
/// asking for the thing that produces them?"), and the sibling gate
/// (`orchestrator::output_vectors_gate`) has already answered it the same
/// way: a data-format error, exit 65.
pub fn check_export_format_vector(opts: &CrawlOptions) -> Result<(), CliExit> {
    if opts.export.export_format != ExportFormat::Vector {
        return Ok(());
    }
    if opts.ai {
        return Ok(());
    }

    warn!("--export-format vector rejected without --clean-ai; no embeddings to export");
    Err(CliExit::DataFormatError(
        "No hay vectores para exportar: '--export-format vector' requiere \
         '--clean-ai' para generar embeddings"
            .to_string(),
    ))
}

// ============================================================================
// LLM extraction provider gate (§8b)
// ============================================================================

/// Preflight: `--extract-with-llm` without a configured `completion` provider
/// must fail at startup, not on the first invocation (`ai-providers-design.md`
/// §8b).
///
/// This is the **expiry of the §8b DEBE**: the Container stays permissive
/// (`llm_port()` is `None` without a provider, by contract), but every service
/// binary that exposes an LLM flag MUST validate the port is present before
/// doing any work. Without this gate a daemon reports OK at boot and then
/// fails when a user or cron invokes the feature, with retries amplifying the
/// failure before anyone notices.
///
/// The check is deliberately **static** (flag + config presence) rather than a
/// probe of the constructed port: a reachable-but-broken provider is a runtime
/// concern with its own error classification, while a missing configuration is
/// a startup configuration error (exit 78).
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when `--extract-with-llm`
/// is set but the config declares no provider with the `completion`
/// capability, or when `--llm-provider` names an id that is unknown or lacks
/// the capability.
pub fn check_extract_with_llm(
    opts: &CrawlOptions,
    providers: &crate::domain::providers::ProvidersConfig,
) -> Result<(), CliExit> {
    if !opts.extract_with_llm {
        return Ok(());
    }

    let registry = crate::domain::providers::ProviderRegistry::new(providers.clone());
    let resolution = match opts.llm_provider.as_deref() {
        Some(id) => registry.resolve(id, crate::domain::providers::Capability::Completion),
        None => registry.resolve_default_completion(),
    };

    match resolution {
        Ok(_) => Ok(()),
        Err(e) => {
            warn!(
                requested_provider = ?opts.llm_provider,
                error = %e,
                "--extract-with-llm rejected: no usable completion provider"
            );
            Err(CliExit::ConfigError(format!(
                "'--extract-with-llm' requiere un provider LLM configurado \
                 con la capacidad `completion`: {e}. Configurá un provider \
                 en el archivo de configuración o quitá el flag."
            )))
        },
    }
}

// ============================================================================
// Pre-flight HTTP Connectivity Check (T-070)
// ============================================================================

/// Result of a pre-flight connectivity check.
pub enum PreflightResult {
    /// 2xx or 3xx response — all good
    Ok,
    /// 4xx or 5xx response — connectivity OK but server issue
    Warning(u16),
    /// DNS failure, connection refused, timeout — cannot reach host
    Failed(String),
}

/// Send a HEAD request to verify connectivity before starting discovery.
/// Falls back to GET with Range: bytes=0-0 if HEAD is blocked (405) or times out.
///
/// # SSRF layer 2 (#1615, G-4)
///
/// This path has no caller in the tree today — the discovery flow resolves its
/// seed through guarded code — which is exactly why it was unguarded: nothing
/// exercised it, so nothing noticed it built a client and dialled whatever it
/// was handed. That is a trap, not a defence: the next wiring would inherit an
/// unguarded fetch path, and a HEAD to a cloud-metadata address is a real
/// request.
///
/// Rather than delete a dead-but-public function (a compatibility change the
/// issue explicitly left as a maintainer decision), the guard goes in now, so
/// the function is safe the moment it is wired. The refusal rides the existing
/// `Failed` arm: to a caller asking "can I reach this host?", a refused target
/// and an unreachable one are the same answer, and the reason is carried in
/// the message rather than hidden.
///
/// The GET fallback is covered by the same check because it runs only after
/// this one passed.
pub async fn preflight_check(url: &url::Url) -> PreflightResult {
    if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(url) {
        return PreflightResult::Failed(rejection.to_string());
    }

    let client = match crate::create_http_client() {
        Ok(c) => c,
        Err(e) => return PreflightResult::Failed(format!("failed to create HTTP client: {e}")),
    };

    match client
        .head(url.as_str())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            if status < 400 {
                PreflightResult::Ok
            } else if status == 405 {
                warn!("HEAD request blocked (405), trying GET fallback...");
                preflight_get_fallback(&client, url).await
            } else {
                PreflightResult::Warning(status)
            }
        },
        Err(e) => {
            if e.is_timeout() || e.is_connect() {
                warn!("HEAD request failed ({}), trying GET fallback...", e);
                preflight_get_fallback(&client, url).await
            } else {
                PreflightResult::Failed(format!("network error: {e}"))
            }
        },
    }
}

/// Fallback to GET with Range: bytes=0-0 when HEAD is blocked.
async fn preflight_get_fallback(client: &wreq::Client, url: &url::Url) -> PreflightResult {
    match client
        .get(url.as_str())
        .header("Range", "bytes=0-0")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => PreflightResult::Ok,
        Ok(resp) => PreflightResult::Warning(resp.status().as_u16()),
        Err(e) => PreflightResult::Failed(format!("HEAD y GET fallaron: {e}")),
    }
}

// ============================================================================
// Display Helpers
// ============================================================================

/// Return emoji or ASCII equivalent based on NO_COLOR setting.
#[inline]
pub fn icon(emoji: &str, ascii: &str) -> String {
    if crate::should_emit_emoji() {
        emoji.to_string()
    } else {
        ascii.to_string()
    }
}

#[cfg(all(test, not(miri)))] // Command::spawn unsupported by Miri (#775)
mod tests_adaptive_selectors;

#[cfg(all(test, not(miri)))]
mod tests_export_format;

#[cfg(all(test, not(miri)))]
mod tests_js_strategy;

#[cfg(all(test, not(miri)))]
mod tests_obscura_binary;

#[cfg(all(test, not(miri)))]
mod tests_obscura_version;

#[cfg(all(test, not(miri)))]
mod tests_path_resolution;

#[cfg(all(test, not(miri)))]
mod tests_sink_gates;

#[cfg(all(test, not(miri)))]
mod tests_version_probe;

// ============================================================================
// Phase 3 — Normalization pipeline tests (split by theme, M3 PR B)
// ============================================================================
#[cfg(test)]
mod normalization_pipeline_tests;

#[cfg(test)]
mod normalization_budget_tests;

#[cfg(test)]
mod normalization_tests_support;
