//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.
#![allow(missing_docs)]

use std::path::PathBuf;
use tracing::warn;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::config::ConfigDefaults;
use crate::domain::budget::{BudgetOverrides, BurstPermits};
use crate::domain::config_value::{ConfigSource, ConfigValue};
use crate::infrastructure::observability::log_scrape_error;
use crate::{Args, CliExit, ConcurrencyConfig, ExportFormat, OutputFormat};
use std::collections::BTreeMap;
use tracing::{info, instrument};

mod field_book;
pub(crate) use field_book::FieldBook;
pub use field_book::{ArgSources, NormalizedConfig};

mod js_strategy;
mod path_resolution;
pub use js_strategy::{check_js_dependencies, resolve_chrome_binary};

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
