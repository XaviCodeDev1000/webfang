//! Config normalization pipeline (stages, cross-field rules, budget overrides).

use std::path::PathBuf;

use tracing::{info, instrument};

use crate::application::crawl_options::CrawlOptions;
use crate::cli::config::ConfigDefaults;
use crate::domain::budget::{BudgetOverrides, BurstPermits};
use crate::domain::config_value::{ConfigSource, ConfigValue};
use crate::infrastructure::observability::log_scrape_error;
use crate::{Args, CliExit, ConcurrencyConfig, ExportFormat, OutputFormat};

use super::config_stages::stage_config_file;
use super::env_cli_stages::stage_env_cli;
use super::field_book::{ArgSources, FieldBook, NormalizedConfig};

pub(super) fn try_write<T: Clone>(
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
pub(super) fn stage_defaults() -> FieldBook {
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
pub(super) fn stage_budget_overrides(
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
pub(super) fn apply_cross_field_rules(book: &mut FieldBook) -> usize {
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
pub(super) fn validate_stage(book: &FieldBook) -> Result<(), CliExit> {
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
pub(super) fn check_adaptive_selectors_feature_with(
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
pub(super) fn check_adaptive_selectors_capability(args: &Args) -> Result<(), CliExit> {
    check_adaptive_selectors_capability_with(cfg!(feature = "adaptive-selectors"), args)
}

/// Feature-injectable core of [`check_adaptive_selectors_capability`] — the
/// #761/#796 injection seam, so the negative case is runnable in the
/// `--all-features` lane that CI uses.
pub(super) fn check_adaptive_selectors_capability_with(
    adaptive_enabled: bool,
    args: &Args,
) -> Result<(), CliExit> {
    adaptive_selectors_unavailable(adaptive_enabled, args.crawler.adaptive_selectors)
}
