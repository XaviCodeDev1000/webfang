// ========================================================================
// merge_budget_overrides (#897 item 1 + review M1 of PR #925)
// ========================================================================
use super::normalization::*;
use super::normalization_tests_support::*;
use super::*;
use crate::cli::config::ConfigDefaults;
use crate::domain::budget::{BudgetOverrides, BurstPermits};

#[test]
fn merge_staged_crawl_outranks_cli_capture() {
    // M1: a plain `--concurrency 2` (unranked CLI capture) must NEVER
    // stomp a ranked staged value — the same contradiction that
    // made `network.concurrency` and the budget model disagree.
    let merged =
        merge_budget_overrides(test_overrides(Some(2), None), test_overrides(Some(5), None));
    assert_eq!(
        merged
            .crawl
            .map(crate::domain::budget::tiers::CrawlConcurrency::get),
        Some(5),
        "ranked staged crawl must win over the unranked CLI capture"
    );
}

#[test]
fn merge_cli_capture_fills_unstaged_crawl_gap() {
    // Nothing staged (no explicit knob anywhere): the CLI capture is
    // the only source and must survive.
    let merged = merge_budget_overrides(test_overrides(Some(3), None), test_overrides(None, None));
    assert_eq!(
        merged
            .crawl
            .map(crate::domain::budget::tiers::CrawlConcurrency::get),
        Some(3)
    );
}

#[test]
fn merge_cli_explicit_asset_wins_and_staged_toml_crawl_survives() {
    // Sharpest slot-copy guard, at unit level: TOML-staged crawl AND an
    // explicit CLI --download-concurrency must coexist field-wise.
    let merged =
        merge_budget_overrides(test_overrides(None, Some(6)), test_overrides(Some(2), None));
    assert_eq!(
        merged
            .crawl
            .map(crate::domain::budget::tiers::CrawlConcurrency::get),
        Some(2)
    );
    assert_eq!(
        merged
            .asset
            .map(crate::domain::budget::tiers::DownloadConcurrency::get),
        Some(6)
    );
}

/// Test fixture builder: overrides with only the two tiers under test.
fn test_overrides(crawl: Option<usize>, asset: Option<usize>) -> BudgetOverrides {
    BudgetOverrides {
        rate_burst: None,
        crawl: crawl.and_then(|v| crate::domain::budget::tiers::CrawlConcurrency::new(v).ok()),
        batch: None,
        asset: asset.and_then(|v| crate::domain::budget::tiers::DownloadConcurrency::new(v).ok()),
    }
}

#[test]
fn budget_overrides_default_is_none() {
    let book = stage_defaults();
    assert_eq!(book.budget_overrides.value.rate_burst, None);
    assert_eq!(book.budget_overrides.source, ConfigSource::Default);
}

#[test]
fn burst_zero_rejected_with_spanish_error() {
    let mut book = stage_defaults();
    let mut args = dummy_args();
    args.crawler.rate_limit_burst = Some("0".to_string()); // bypasses clap parser (programmatic)
    let mut sources = ArgSources::default();
    sources.set("rate_limit_burst", ConfigSource::Cli);
    let err = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
        .expect_err("zero burst must be rejected");
    match err {
        CliExit::ConfigError(msg) => assert!(
            msg.contains("--rate-limit-burst debe ser >= 1"),
            "unexpected error: {msg}"
        ),
        other => panic!("expected ConfigError, got {other:?}"),
    }
    // slot untouched
    assert_eq!(book.budget_overrides.value.rate_burst, None);
}

// ========================================================================
// #1813 — the preflight pipeline is the SINGLE validating authority for
// `--rate-limit-burst`, and `From<Args>` no longer parses it a second time.
// ========================================================================

/// `burst_non_numeric_rejected_with_spanish_error` — the last fail-open
/// arm, at the layer that actually owns the decision.
///
/// `auto` is deliberately NOT in the list: it is a keyword the sibling
/// `--concurrency` parser understands, not a typo, and hard-`Err`-ing on it
/// broke a spelling that worked on the base. See
/// `burst_auto_keyword_stages_as_not_set`.
#[test]
fn burst_non_numeric_rejected_with_spanish_error() {
    for raw in ["banana", "1_0", "12x"] {
        let mut book = stage_defaults();
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some(raw.to_string());
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Cli);
        let err = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
            .expect_err("a non-numeric burst must be rejected, not defaulted");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("no es un número"),
                "unexpected error for {raw}: {msg}"
            ),
            other => panic!("expected ConfigError for {raw}, got {other:?}"),
        }
        assert_eq!(book.budget_overrides.value.rate_burst, None);
    }
}

/// The `auto` counterpart, at the same layer: the keyword must stage as
/// "not set" — no override written, NO error — so the hardware-derived
/// budget applies. Asserting the empty slot matters as much as the `Ok`:
/// writing a value here would silently override the derived burst, which is
/// the other half of the same regression.
#[test]
fn burst_auto_keyword_stages_as_not_set() {
    for raw in ["auto", "AUTO", "  auto  "] {
        let mut book = stage_defaults();
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some(raw.to_string());
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Cli);
        let written = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
            .unwrap_or_else(|e| panic!("`{raw}` must not be rejected: {e:?}"));
        assert_eq!(
            book.budget_overrides.value.rate_burst, None,
            "`{raw}` must leave the burst slot for the derived default"
        );
        assert_eq!(written, 0, "`{raw}` is not set, so it writes no field");
    }
}

/// **The regression guard for the #1813 design itself.** `From<Args>` used
/// to parse the burst independently, so a valid `--rate-limit-burst` was
/// delivered through TWO paths. This test reproduces `main.rs` step 6b
/// literally — `normalize` → `into_crawl_options` → `From<Args>` →
/// `merge_budget_overrides` — because deleting the second parse is only
/// safe if the first one alone still delivers the override. If
/// `merge_budget_overrides` ever stops preferring `staged` when the CLI
/// capture is `None`, the burst would be silently dropped for every
/// operator and nothing else would notice.
#[test]
fn main_flow_merge_preserves_valid_burst_override() {
    let mut args = dummy_args();
    args.crawler.rate_limit_burst = Some("7".to_string());
    let mut sources = ArgSources::default();
    sources.set("rate_limit_burst", ConfigSource::Cli);

    // main.rs:116
    let normalized = normalize(&args, &sources, &dummy_config()).expect("normalize ok");
    // main.rs:121 — the infallible projection; contributes no burst.
    let base = crate::application::crawl_options::CrawlOptions::from(args);
    assert_eq!(
        base.budget_overrides.rate_burst, None,
        "precondition: `From<Args>` supplies no burst"
    );
    let cli_budget = base.budget_overrides;
    // main.rs:126
    let projected = normalized.into_crawl_options();
    // main.rs:156
    let merged = merge_budget_overrides(cli_budget, projected.budget_overrides);

    assert_eq!(
        merged.rate_burst,
        BurstPermits::new(7).ok(),
        "a valid --rate-limit-burst 7 must still reach the engine as the \
         rate-limiter burst after the duplicate parse was removed"
    );
}

/// The env-var front door reaches the same rejection. Same provenance rank
/// as argv, so this is triangulation rather than a new code path — but it
/// is the one the #1813 acceptance criterion names explicitly.
#[test]
fn burst_non_numeric_from_env_rejected() {
    let mut book = stage_defaults();
    let mut args = dummy_args();
    args.crawler.rate_limit_burst = Some("banana".to_string());
    let mut sources = ArgSources::default();
    sources.set("rate_limit_burst", ConfigSource::Environment);
    let err = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
        .expect_err("env-sourced non-numeric burst must be rejected");
    assert!(
        matches!(err, CliExit::ConfigError(_)),
        "expected ConfigError, got {err:?}"
    );
}

#[test]
fn normalize_end_to_end_burst_reaches_crawl_options() {
    let mut args = dummy_args();
    args.crawler.rate_limit_burst = Some("9".to_string());
    let mut sources = ArgSources::default();
    sources.set("rate_limit_burst", ConfigSource::Environment);
    let normalized = normalize(&args, &sources, &dummy_config()).expect("normalize ok");
    assert_eq!(
        normalized.budget_overrides.value.rate_burst,
        BurstPermits::new(9).ok()
    );
    let opts = normalized.into_crawl_options();
    assert_eq!(opts.budget_overrides.rate_burst, BurstPermits::new(9).ok());
}

#[test]
fn normalize_precedence_total_order() {
    let args = {
        let mut a = dummy_args();
        a.crawler.max_pages = 10;
        a
    };
    let mut sources = ArgSources::default();
    sources.set("max_pages", ConfigSource::Cli);
    let config = ConfigDefaults {
        max_pages: Some(25),
        ..dummy_config()
    };
    let normalized = normalize(&args, &sources, &config).expect("normalize ok");
    assert_eq!(normalized.max_pages.value, 10);
    assert_eq!(normalized.max_pages.source, ConfigSource::Cli);
}
