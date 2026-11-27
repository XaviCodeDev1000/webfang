//! ADR-002 equivalence proof (slice 2): the hand-derived clap surface of
//! [`CrawlerArgs`] must stay in lockstep with the OptionsSpec crawler
//! group. Written FIRST against the UNMIGRATED parsers (pin); the
//! migration then routes validation through the spec and must keep them
//! green.
//!
//! Deferred from the spec this slice (asserted here so the list stays
//! honest): `concurrency`, `rate_limit_burst`, `include_patterns`,
//! `exclude_patterns`, `headers`, `cookies`, and the feature-gated
//! `clean_ai` / `adaptive_selectors`.

use super::*;
use crate::cli::args::test_support::{
    assert_defaults, assert_help, assert_long_short_alias_env_heading, assert_structural_one,
    assert_structural_shape, assert_surface_covered_except, collect_args, parse_args_hermetic,
};
use crate::domain::options_spec as spec;
use clap::Args as _;

/// CrawlerArgs fields intentionally OUTSIDE the spec this slice, with the
/// reason each was deferred.
const DEFERRED_FROM_SPEC: &[(&str, &str)] = &[
    (
        "concurrency",
        "custom ConcurrencyConfig FromStr with auto detection",
    ),
    (
        "rate_limit_burst",
        "raw-string preflight staging with warn-and-default semantics",
    ),
    ("include_patterns", "Vec arg with ',' delimiter"),
    ("exclude_patterns", "Vec arg with ',' delimiter"),
    ("headers", "repeatable Vec arg with ';' delimiter"),
    ("cookies", "repeatable Vec arg with ';' delimiter"),
];

/// All clap args generated for `CrawlerArgs`, keyed by arg id.
pub(super) fn command_args() -> Vec<clap::Arg> {
    collect_args(CrawlerArgs::augment_args(clap::Command::new(
        "webfang-crawler",
    )))
}

/// Module-owner shorthand for the shared lookup so the pins below
/// keep reading as `arg_by_id(&args, id)`.
pub(super) fn arg_by_id<'a>(args: &'a [clap::Arg], id: &str) -> &'a clap::Arg {
    crate::cli::args::test_support::arg_by_id(args, id, "CrawlerArgs")
}

#[test]
fn deferred_list_is_honest_about_its_reasons() {
    assert_eq!(spec::crawler::GROUP.len() + DEFERRED_FROM_SPEC.len(), 50);
    let args = command_args();
    for (id, _) in DEFERRED_FROM_SPEC {
        assert!(
            args.iter().any(|a| a.get_id() == *id),
            "deferred id `{id}` no longer exists in CrawlerArgs — update the list"
        );
    }
}

#[test]
fn clap_surface_is_fully_covered_by_the_spec() {
    let deferred_ids: Vec<&str> = DEFERRED_FROM_SPEC.iter().map(|(id, _)| *id).collect();
    assert_surface_covered_except(&command_args(), spec::crawler::GROUP, &deferred_ids);
}

#[test]
fn long_short_aliases_env_and_heading_match_the_spec() {
    assert_long_short_alias_env_heading(&command_args(), spec::crawler::GROUP);
}

#[test]
fn defaults_match_the_spec() {
    assert_defaults(&command_args(), spec::crawler::GROUP);
}

#[test]
fn help_text_matches_the_spec() {
    assert_help(&command_args(), spec::crawler::GROUP);
}

#[test]
fn representative_values_parse_identically_through_clap() {
    // Defaults (hermetic: ambient WEBFANG_* must not leak into the
    // bare parse — issue #926).
    let defaults = parse_args_hermetic(&[]).expect("bare invocation must parse");
    assert_eq!(defaults.crawler.selector, "body");
    assert_eq!(defaults.crawler.delay_ms, 1000);
    assert_eq!(defaults.crawler.jitter_ms, 0);
    assert_eq!(defaults.crawler.max_pages, 10);
    assert_eq!(defaults.crawler.max_depth, 2);
    assert!(!defaults.crawler.use_sitemap);
    assert!(!defaults.crawler.resume);

    // Short forms in isolation (`--url` may only appear once per parse).
    let shorts = parse_args_hermetic(&["-u", "https://example.org", "-s", "main"])
        .expect("shorts must parse");
    assert_eq!(
        shorts.crawler.url.as_ref().map(ValidUrl::as_str),
        Some("https://example.org/")
    );
    assert_eq!(shorts.crawler.selector, "main");
}

#[test]
fn target_discovery_behavior_and_display_flags_parse_identically() {
    let parsed = parse_args_hermetic(&[
        "--url",
        "https://example.com",
        "--selector",
        "article p",
        "--delay-ms",
        "250",
        "--max-pages",
        "5",
        "--use-sitemap",
        "--sitemap-url",
        "https://example.com/sitemap.xml",
        "--single-page",
        "--resume",
        "--state-dir",
        "/tmp/wf-state",
        "--download-assets",
        "--extraction-fingerprint",
        "-vvv",
        "-q",
        "-n",
        "--trace-file",
        "trace.jsonl",
    ])
    .expect("representative crawler flags must parse");

    let c = &parsed.crawler;
    assert_eq!(
        c.url.as_ref().map(ValidUrl::as_str),
        Some("https://example.com/")
    );
    assert_eq!(c.selector, "article p");
    assert_eq!(c.delay_ms, 250);
    assert_eq!(c.max_pages, 5);
    assert!(c.use_sitemap);
    assert_eq!(
        c.sitemap_url.as_deref(),
        Some("https://example.com/sitemap.xml")
    );
    assert!(c.single_page);
    assert!(c.resume);
    assert_eq!(c.state_dir, Some(std::path::PathBuf::from("/tmp/wf-state")));
    assert!(c.download_assets);
    assert!(!c.download_images);
    assert!(!c.download_documents);
    assert!(c.extraction_fingerprint);
    assert_eq!(c.verbose, 3);
    assert!(c.quiet);
    assert!(c.dry_run);
    assert_eq!(c.trace_file, Some(std::path::PathBuf::from("trace.jsonl")));
}

#[test]
fn crawler_http_download_and_feature_flags_parse_identically() {
    let parsed = parse_args_hermetic(&[
        "--max-depth",
        "4",
        "--timeout-secs",
        "60",
        "--asset-naming",
        "slug",
        "--download-concurrency",
        "8",
        "--max-retries",
        "2",
        "--backoff-base-ms",
        "500",
        "--backoff-max-ms",
        "8000",
        "--accept-language",
        "es-AR,es;q=0.9",
        "--user-agent",
        "webfang-parity/1.0",
        "--max-file-size",
        "1048576",
        "--download-timeout",
        "45",
        "--sitemap-depth",
        "1",
        "--checkpoint-interval",
        "50",
        "--no-checkpoint",
        "--ignore-robots",
        "--ignore-waf",
        "--autoscale",
        "--no-session-health",
        "--h2-profile",
        "Chrome131",
        "--js-strategy",
        "hybrid",
        "--obscura-binary",
        "/usr/local/bin/obscura",
        "--dom-preprune=false",
    ])
    .expect("representative crawler flags must parse");

    let c = &parsed.crawler;
    assert_eq!(c.max_depth, 4);
    assert_eq!(c.timeout_secs, 60);
    assert_eq!(c.asset_naming, "slug");
    assert_eq!(c.download_concurrency, Some(8));
    assert_eq!(c.max_retries, 2);
    assert_eq!(c.backoff_base_ms, 500);
    assert_eq!(c.backoff_max_ms, 8000);
    assert_eq!(c.accept_language, "es-AR,es;q=0.9");
    assert_eq!(c.user_agent.as_deref(), Some("webfang-parity/1.0"));
    assert_eq!(c.max_file_size, 1048576);
    assert_eq!(c.download_timeout, 45);
    assert_eq!(c.sitemap_depth, 1);
    assert_eq!(c.checkpoint_interval, 50);
    assert!(c.no_checkpoint);
    assert!(c.ignore_robots);
    assert!(c.ignore_waf);
    assert!(c.autoscale);
    assert!(c.no_session_health);
    assert_eq!(c.h2_profile, "Chrome131");
    assert_eq!(c.js_strategy, crate::domain::JsStrategy::Hybrid);
    assert_eq!(c.obscura_binary, "/usr/local/bin/obscura");
    assert!(!c.dom_preprune);
}

/// Slice 3 (ADR-002) pin: structural clap surface of the spec-covered
/// crawler options that the spec-driven builder must reproduce
/// byte-for-byte. Written against the derive BEFORE the migration.
/// `-v` keeps its own action dispatch (it counts occurrences,
/// `Count`) but shares the value-name / possible-values / delimiter /
/// long-help shape with every other spec.
#[test]
fn structural_actions_value_names_and_possible_values_match_the_spec() {
    let args = command_args();
    for s in spec::crawler::GROUP {
        if !s.active() {
            continue; // gated off in this build: placeholder pinned separately
        }
        let arg = arg_by_id(&args, s.id);
        if s.id == "verbose" {
            assert!(
                matches!(arg.get_action(), clap::ArgAction::Count),
                "verbose must use Count",
            );
            assert_structural_shape(arg, s);
        } else {
            assert_structural_one(arg, s);
        }
    }
}

/// Slice 3 pin: the two special arities inside the group — `-v`
/// counts occurrences and `--dom-preprune` accepts an optional value.
#[test]
fn count_and_optional_value_bool_arity_is_pinned() {
    let args = command_args();
    let verbose = arg_by_id(&args, "verbose");
    assert!(matches!(verbose.get_action(), clap::ArgAction::Count));

    let dom = arg_by_id(&args, "dom_preprune");
    assert!(matches!(dom.get_action(), clap::ArgAction::SetTrue));
    let range = dom
        .get_num_args()
        .expect("dom-preprune must declare num_args");
    assert_eq!((range.min_values(), range.max_values()), (0, 1));
    let defaults: Vec<String> = dom
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    assert_eq!(defaults, vec!["true"]);
}
