// ============================================================================
// Phase 3 — Normalization pipeline (RED: tests before implementation)
// These tests pin the rank-guard pipeline per design D3/D4. They intentionally
// fail to compile while the production symbols do not exist (strict TDD RED).
// ============================================================================
use super::config_stages::*;
use super::env_cli_stages::*;
use super::normalization::*;
use super::normalization_tests_support::*;
use super::*;
use crate::cli::config::ConfigDefaults;
use crate::domain::budget::BurstPermits;
use crate::domain::config_value::ConfigSource;

#[test]
fn stage_defaults_seeds_default_sources() {
    let book = stage_defaults();
    assert_eq!(book.max_pages.source, ConfigSource::Default);
    assert_eq!(book.max_pages.value, 10);
    assert_eq!(book.delay_ms.source, ConfigSource::Default);
}

#[test]
fn stage_config_file_writes_config_file_source() {
    let mut book = stage_defaults();
    let config = ConfigDefaults {
        max_pages: Some(25),
        ..dummy_config()
    };
    let written = stage_config_file(&mut book, &config);
    assert!(written >= 1);
    assert_eq!(book.max_pages.value, 25);
    assert_eq!(book.max_pages.source, ConfigSource::ConfigFile);
}

#[test]
fn rank_guard_config_file_rejected_over_cli() {
    let mut book = stage_defaults();
    book.max_pages = crate::domain::config_value::ConfigValue::new(10, ConfigSource::Cli);
    let config = ConfigDefaults {
        max_pages: Some(25),
        ..dummy_config()
    };
    let written = stage_config_file(&mut book, &config);
    assert_eq!(written, 0);
    assert_eq!(book.max_pages.value, 10);
    assert_eq!(book.max_pages.source, ConfigSource::Cli);
}

/// #1813 S1 — the `--adaptive-selectors` capability gate is a step of
/// [`normalize`], not a call in the CLI binary's step list.
///
/// ## Why these two tests are mutually `cfg`-gated
///
/// `normalize` reads the feature state from `cfg!`, which is a
/// compile-time constant, so no runtime argument can make the real pipeline
/// fail in an `--all-features` build. The two lanes therefore each prove
/// something different, and BOTH are non-empty:
///
/// - `#[cfg(not(feature = "adaptive-selectors"))]` (negative lane) — the
///   REJECTION, driving the real `normalize`. This is the test that says
///   "the gate is inside the shared pipeline", not merely that a helper
///   function rejects the same input.
/// - `#[cfg(feature = "adaptive-selectors")]` (`--all-features` lane) —
///   the NON-REGRESSION guard: on a build that HAS the feature, the same
///   `normalize` call with the same request must still succeed, so moving
///   the gate into the pipeline cannot have broken the working case.
///
/// `cargo nextest list -p webfang_core -E 'test(adaptive)'` on each lane is
/// the proof that neither compiles to nothing. The end-to-end CLI proof
/// (real spawned binary, real exit code, zero network requests) lives in
/// `tests/adaptive_selectors_gate_test.rs`.
#[cfg(not(feature = "adaptive-selectors"))]
#[test]
fn normalize_rejects_adaptive_selectors_on_a_non_adaptive_build() {
    let mut args = dummy_args();
    args.crawler.adaptive_selectors = true;

    let err = normalize(&args, &ArgSources::default(), &dummy_config())
        .expect_err("the shared pipeline must reject the capability it cannot honor");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("--adaptive-selectors") && msg.contains("`adaptive-selectors`"),
            "normalize's rejection must name the flag and the feature, got: {msg}"
        ),
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

/// Non-regression guard for the positive lane: with the feature compiled in,
/// `--adaptive-selectors` through the same pipeline must still succeed.
///
/// The assertion is on the pipeline SUCCEEDING, which is the whole point:
/// the flag used to work on this lane and must keep working. `adaptive_selectors`
/// itself is deliberately not asserted on the projected options —
/// `NormalizedConfig::into_crawl_options` has never carried it; it travels
/// via `CrawlOptions::from(Args)` — so pinning it here would pin a
/// projection detail this change did not touch.
#[cfg(feature = "adaptive-selectors")]
#[test]
fn normalize_accepts_adaptive_selectors_on_an_adaptive_build() {
    let mut args = dummy_args();
    args.crawler.adaptive_selectors = true;

    normalize(&args, &ArgSources::default(), &dummy_config())
        .expect("an adaptive build must normalize a --adaptive-selectors request");
}

/// A run that did NOT ask for the capability must never be failed closed by
/// the new pipeline step — in EITHER lane. This one is deliberately not
/// `cfg`-gated, so it is the single test that holds in both.
#[test]
fn normalize_does_not_gate_a_run_that_did_not_request_adaptive_selectors() {
    let args = dummy_args();
    assert!(
        !args.crawler.adaptive_selectors,
        "precondition: the default Args does not request the capability"
    );
    normalize(&args, &ArgSources::default(), &dummy_config())
        .expect("normalize must not fail closed on a default request set");
}

/// Field errors keep their existing precedence over the new capability
/// step. The gate runs LAST inside `normalize` precisely so a run that is
/// already misconfigured reports the misconfiguration it always did.
///
/// `max_pages` is staged only when the provenance map records it — the
/// pipeline is rank-guarded, so an `Args` value with no recorded source is
/// not a staged value and the book keeps its default. Recording the source
/// is what makes this the conflict case it claims to be.
#[test]
fn normalize_reports_field_errors_before_the_capability_gate() {
    let mut args = dummy_args();
    args.crawler.adaptive_selectors = true;
    args.crawler.max_pages = 0;
    let mut sources = ArgSources::default();
    sources.set("max_pages", ConfigSource::Cli);

    let err = normalize(&args, &sources, &dummy_config())
        .expect_err("max_pages 0 must still be rejected");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("max_pages"),
            "the long-standing max_pages error must keep winning, got: {msg}"
        ),
        other => panic!("expected ConfigError, got: {other:?}"),
    }
}

#[test]
fn stage_env_cli_writes_cli_obsidian_tags() {
    let mut book = stage_defaults();
    let mut args = dummy_args();
    args.obsidian.obsidian_tags = Some(vec!["scraped".into(), "web-dev".into(), "rust".into()]);
    let mut sources = ArgSources::default();
    sources.set("obsidian_tags", ConfigSource::Cli);
    let written = stage_env_cli(&mut book, &args, &sources);
    assert!(written >= 1);
    assert_eq!(book.obsidian_tags.source, ConfigSource::Cli);
    assert_eq!(book.obsidian_tags.value, vec!["scraped", "web-dev", "rust"]);
}

#[test]
fn stage_env_cli_environment_then_cli() {
    let mut book = stage_defaults();
    let mut args = dummy_args();
    args.crawler.max_pages = 7;
    let mut sources = ArgSources::default();
    sources.set("max_pages", ConfigSource::Environment);
    let written_env = stage_env_cli(&mut book, &args, &sources);
    assert!(written_env >= 1);
    assert_eq!(book.max_pages.source, ConfigSource::Environment);
    // Now CLI outranks Environment even when value equals default
    let mut args2 = dummy_args();
    args2.crawler.max_pages = 10;
    let mut sources2 = ArgSources::default();
    sources2.set("max_pages", ConfigSource::Cli);
    let written_cli = stage_env_cli(&mut book, &args2, &sources2);
    assert!(written_cli >= 1);
    assert_eq!(book.max_pages.value, 10);
    assert_eq!(book.max_pages.source, ConfigSource::Cli);
}

#[test]
fn cross_field_sitemap_url_implies_use_sitemap_from_any_source() {
    // Via config file
    let mut book = stage_defaults();
    let _config = ConfigDefaults { ..dummy_config() };
    // Simulate sitemap_url coming from config stage then cross-field rule
    book.sitemap_url = crate::domain::config_value::ConfigValue::new(
        Some("https://example.com/sitemap.xml".to_string()),
        ConfigSource::ConfigFile,
    );
    let _ = apply_cross_field_rules(&mut book);
    assert!(
        book.use_sitemap.value,
        "sitemap_url via ConfigFile must imply use_sitemap"
    );
    // Via CLI
    let mut book2 = stage_defaults();
    book2.sitemap_url = crate::domain::config_value::ConfigValue::new(
        Some("https://example.com/sitemap.xml".to_string()),
        ConfigSource::Cli,
    );
    let _ = apply_cross_field_rules(&mut book2);
    assert!(book2.use_sitemap.value);
    // Via Environment (third covered source)
    let mut book3 = stage_defaults();
    book3.sitemap_url = crate::domain::config_value::ConfigValue::new(
        Some("https://example.com/sitemap.xml".to_string()),
        ConfigSource::Environment,
    );
    let _ = apply_cross_field_rules(&mut book3);
    assert!(book3.use_sitemap.value);
}

#[test]
fn obsidian_tag_trim_dedup_byte_parity() {
    let mut book = stage_defaults();
    book.obsidian_tags = crate::domain::config_value::ConfigValue::new(
        vec![
            " rust ".to_string(),
            "rust".to_string(),
            "cargo".to_string(),
            " ".to_string(),
        ],
        ConfigSource::Default,
    );
    let _ = apply_cross_field_rules(&mut book);
    assert_eq!(
        book.obsidian_tags.value,
        vec!["rust".to_string(), "cargo".to_string()]
    );
}

#[test]
fn validate_stage_blocks_invalid_max_pages() {
    let mut book = stage_defaults();
    book.max_pages = crate::domain::config_value::ConfigValue::new(0, ConfigSource::Cli);
    let err = validate_stage(&book).expect_err("max_pages 0 must fail validation");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("max_pages"),
            "Spanish validation error, got: {msg}"
        ),
        other => panic!("expected ConfigError, got {other:?}"),
    }
}

#[test]
fn into_crawl_options_projection_parity() {
    let book = stage_defaults();
    let normalized = NormalizedConfig::from_book(book);
    let opts = normalized.into_crawl_options();
    let expected = CrawlOptions::default();
    assert_eq!(opts.crawl.max_pages, expected.crawl.max_pages);
    assert_eq!(opts.crawl.selector, expected.crawl.selector);
    assert_eq!(opts.export.output_format, expected.export.output_format);
}

/// CLI wiring verification (issue #1599, slice 4 — verify first): an
/// explicit `--concurrency 2` must surface as `BudgetOverrides.crawl`
/// through `into_crawl_options` (explicit-wins), so the engine derives
/// its scheduler spawn bound from the override instead of the machine
/// tier. Expected outcome: verification-only, zero CLI diff.
#[test]
fn into_crawl_options_explicit_concurrency_feeds_budget_override() {
    let mut book = stage_defaults();
    book.concurrency = ConfigValue::new(ConcurrencyConfig::new(2), ConfigSource::Cli);
    let opts = NormalizedConfig::from_book(book).into_crawl_options();
    assert_eq!(
        opts.budget_overrides
            .crawl
            .map(crate::domain::budget::tiers::CrawlConcurrency::get),
        Some(2),
        "explicit --concurrency 2 must reach the engine as a crawl-tier override"
    );
}

/// CLI wiring verification (issue #1599, slice 4): auto concurrency
/// (`None`) stages no crawl-tier override, preserving today's
/// detector-derived behavior exactly.
#[test]
fn into_crawl_options_auto_concurrency_preserves_derived_behavior() {
    let book = stage_defaults();
    let opts = NormalizedConfig::from_book(book).into_crawl_options();
    assert_eq!(
        opts.budget_overrides.crawl, None,
        "auto concurrency must not stage a crawl-tier override"
    );
}

#[test]
fn stage_budget_overrides_writes_cli_burst() {
    let mut book = stage_defaults();
    let mut args = dummy_args();
    args.crawler.rate_limit_burst = Some("7".to_string());
    let mut sources = ArgSources::default();
    sources.set("rate_limit_burst", ConfigSource::Cli);
    let written =
        stage_budget_overrides(&mut book, &args, &sources, &dummy_config()).expect("staging ok");
    assert_eq!(written, 1);
    assert_eq!(book.budget_overrides.source, ConfigSource::Cli);
    assert_eq!(
        book.budget_overrides.value.rate_burst,
        BurstPermits::new(7).ok()
    );
}

#[test]
fn stage_budget_overrides_toml_staged_at_config_file_rank_and_cli_outranks() {
    let mut book = stage_defaults();
    let config = ConfigDefaults {
        rate_limit_burst: Some(4),
        ..dummy_config()
    };
    let no_sources = ArgSources::default();
    let written =
        stage_budget_overrides(&mut book, &dummy_args(), &no_sources, &config).expect("staging ok");
    assert_eq!(written, 1);
    assert_eq!(book.budget_overrides.source, ConfigSource::ConfigFile);
    // CLI outranks the TOML value
    let mut args = dummy_args();
    args.crawler.rate_limit_burst = Some("6".to_string());
    let mut sources = ArgSources::default();
    sources.set("rate_limit_burst", ConfigSource::Cli);
    let written_cli =
        stage_budget_overrides(&mut book, &args, &sources, &config).expect("staging ok");
    assert_eq!(written_cli, 1);
    assert_eq!(book.budget_overrides.source, ConfigSource::Cli);
    assert_eq!(
        book.budget_overrides.value.rate_burst,
        BurstPermits::new(6).ok()
    );
}
