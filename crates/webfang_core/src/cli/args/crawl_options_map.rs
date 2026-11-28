//! `Args` → `CrawlOptions` projection.
//!
//! Split from `args/mod.rs`: the projection is a pure mapping step from the
//! parsed CLI surface onto the runtime options, distinct from parsing and
//! spec-parity work; keeping it here keeps `args/mod.rs` under its
//! size-ratchet baseline. The trait impl is discovered globally, so the
//! module stays private.

use super::Args;
use crate::domain::ValidUrl;

impl From<Args> for crate::application::crawl_options::CrawlOptions {
    /// Convert CLI arguments into structured [`crate::application::crawl_options::CrawlOptions`].
    ///
    /// This is an owned, lossless conversion — every field in `Args` maps
    /// to exactly one field in `CrawlOptions`. The `url` field is parsed
    /// from `Option<String>` into `Url` (panics if invalid; CLI validation
    /// guarantees validity before this point).
    ///
    /// The `url` parse is the ONLY fallible-to-panic step left here:
    /// `--rate-limit-burst` is not parsed by this conversion at all —
    /// `rate_burst` is contributed as `None` and the preflight pipeline is
    /// the single validating authority (`stage_budget_overrides`, exit 78
    /// for `0` / out-of-`u32` / non-numeric, #1813). See the field comment
    /// in the body for the full history of the removed duplicate parse.
    #[allow(clippy::too_many_lines)]
    fn from(args: Args) -> Self {
        // Capture BEFORE the move into NetworkOptions: explicit operator
        // concurrency feeds the budget model as a crawl-tier override.
        let explicit_crawl = args.crawler.concurrency.get();
        let explicit_batch = args.export.batch_concurrency;
        let explicit_download = args.crawler.download_concurrency;
        use crate::application::crawl_options::{
            CrawlLimits, ExportOptions, IngestionTuning, NetworkOptions,
        };

        let url = url_from_args(&args);

        let overrides = args.elastic_overrides();
        let ai_config = build_ai_config(&args);

        Self {
            url,
            verbosity: args.crawler.verbose,
            quiet: args.crawler.quiet,
            ai: args.crawler.clean_ai,
            adaptive_selectors: args.crawler.adaptive_selectors,
            extraction_fingerprint: args.crawler.extraction_fingerprint,
            crawl: CrawlLimits {
                selector: args.crawler.selector,
                max_depth: args.crawler.max_depth,
                max_pages: args.crawler.max_pages,
                single_page: args.crawler.single_page,
                include_patterns: args.crawler.include_patterns,
                exclude_patterns: args.crawler.exclude_patterns,
                // No CLI flag sets this anymore (#880 removed --interactive);
                // internal knob, defaults to false.
                interactive: false,
                resume: args.crawler.resume,
                state_dir: args.crawler.state_dir,
                use_sitemap: args.crawler.use_sitemap,
                sitemap_url: args.crawler.sitemap_url,
                checkpoint_interval: args.crawler.checkpoint_interval,
                no_checkpoint: args.crawler.no_checkpoint,
                ignore_robots: args.crawler.ignore_robots,
                ignore_waf: args.crawler.ignore_waf,
                no_session_health: args.crawler.no_session_health,
                autoscale_enabled: args.crawler.autoscale,
                dom_preprune: args.crawler.dom_preprune,
            },
            network: NetworkOptions {
                user_agent: args.crawler.user_agent,
                accept_language: args.crawler.accept_language,
                concurrency: args.crawler.concurrency,
                delay_ms: args.crawler.delay_ms,
                jitter_ms: args.crawler.jitter_ms,
                timeout_secs: args.crawler.timeout_secs,
                timeout_secs_explicit: args.crawler.timeout_secs_explicit,
                max_retries: args.crawler.max_retries,
                backoff_base_ms: args.crawler.backoff_base_ms,
                backoff_max_ms: args.crawler.backoff_max_ms,
                download_images: args.crawler.download_images || args.crawler.download_assets,
                download_documents: args.crawler.download_documents || args.crawler.download_assets,
                max_file_size: Some(args.crawler.max_file_size),
                download_timeout_secs: args.crawler.download_timeout,
                h2_profile: args.crawler.h2_profile,
                js_strategy: args.crawler.js_strategy,
                cookie_jar_scope: args.crawler.cookie_jar_scope,
                obscura_binary: args.crawler.obscura_binary,
                // F-52-b: default idle; CLI/env override via --js-wait.
                post_load_wait: args.crawler.post_load_wait,
                // F-52-c: unresolved at projection; the preflight gate
                // (main.rs 6c) resolves it post-validation.
                chrome_binary: None,
                custom_headers: args
                    .crawler
                    .headers
                    .iter()
                    .filter_map(|h| {
                        h.split_once(':')
                            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                    })
                    .collect(),
                initial_cookies: args
                    .crawler
                    .cookies
                    .iter()
                    .filter_map(|c| {
                        c.split_once('=')
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                    })
                    .collect(),
            },
            export: ExportOptions {
                output_format: args.export.format,
                export_format: args.export.export_format,
                output_dir: args.export.output,
                dry_run: args.crawler.dry_run,
                quiet: args.crawler.quiet,
                // #762: capture --vault EXPLICITNESS at parse time (config
                // merge + autodetection can fill `obsidian_vault` later).
                vault_is_explicit: args.obsidian.vault.is_some(),
                obsidian_vault: args.obsidian.vault,
                obsidian_rich_metadata: args.obsidian.obsidian_rich_metadata,
                obsidian_tags: args.obsidian.obsidian_tags.unwrap_or_default(),
                obsidian_wiki_links: args.obsidian.obsidian_wiki_links,
                obsidian_relative_assets: args.obsidian.obsidian_relative_assets,
                quick_save: args.obsidian.quick_save,
                retention_days: args.export.retention_days,
            },
            elastic: IngestionTuning {
                enabled: args.export.elastic,
                cpu_cores: overrides.cpu_cores,
                ram_budget_bytes: overrides.ram_budget_bytes,
                db_path: overrides.db_path,
                max_resource_bytes: overrides.max_resource_bytes,
                output_vectors: args.export.output_vectors.clone(),
            },
            pipeline_enabled: args.export.pipeline,
            pipeline_output_format: args.export.pipeline_output,
            batch: crate::application::crawl_options::BatchOptions {
                enabled: args.export.batch || args.export.batch_file.is_some(),
                batch_file: args.export.batch_file,
                concurrency: args.export.batch_concurrency,
                export_checkpoint: args.export.export_checkpoint,
                strict_input: args.export.strict_input,
            },
            asset_naming: args.crawler.asset_naming,
            download_concurrency: args.crawler.download_concurrency,
            ai_config,
            extract_with_llm: args.llm.extract_with_llm,
            llm_provider: args.llm.llm_provider.clone(),
            embedding_provider: args.llm.embedding_provider.clone(),
            tier2_embedding_provider: args.llm.tier2_embedding_provider.clone(),
            tier2_consent: args.llm.tier2_consent,
            budget_overrides: crate::domain::budget::BudgetOverrides {
                // #1813: `--rate-limit-burst` is resolved by ONE authority —
                // `preflight::stage_budget_overrides`, reached through
                // `preflight::normalize`, which `webfang_cli::main` runs
                // (step 6b) BEFORE this conversion (step 6b). It rejects `0`,
                // out-of-`u32` and non-numeric values with a Spanish
                // `CliExit::ConfigError` (exit 78), and `merge_budget_overrides`
                // carries its validated `BurstPermits` into the final options
                // (`cli_capture.rate_burst.or(staged.rate_burst)`).
                //
                // This field used to parse the raw string a SECOND time and
                // `panic!` on rejection, on the claim that the binary validates
                // first. That claim was true only for the shipped binary's
                // argv/env path, so the panics were a live landmine for any
                // caller constructing `Args` programmatically (library
                // embedders, MCP, tests) — production code must not abort on
                // user input. Since an infallible `From` cannot report an
                // error, contributing `None` and deferring to the pipeline is
                // the honest shape: the duplicate parse is gone, and the
                // override is still delivered by the pipeline.
                //
                // `main_flow_merge_preserves_valid_burst_override`
                // (`cli::preflight`) pins that a VALID burst survives exactly
                // this hand-off, so removing the second parse cannot silently
                // drop the operator's value.
                rate_burst: None,
                // Explicit `--concurrency` (when not "auto") feeds the
                // model as a crawl override — same explicit-wins rule as
                // the preflight pipeline path.
                crawl: explicit_crawl
                    .and_then(|v| crate::domain::budget::tiers::CrawlConcurrency::new(v).ok()),
                batch: explicit_batch
                    .and_then(|v| crate::domain::budget::tiers::BatchConcurrency::new(v).ok()),
                asset: explicit_download
                    .and_then(|v| crate::domain::budget::tiers::DownloadConcurrency::new(v).ok()),
            },
        }
    }
}

/// Parse the CLI URL into a [`url::Url`] for the `From<Args> -> CrawlOptions`
/// Take the already-hardened seed URL from the args.
///
/// #1239: parsing and hardening happen at the argv boundary
/// ([`parse_seed_url`](crate::cli::args::crawler::parse_seed_url), wired as the clap value parser for both `--url` and
/// the positional URL), so by the time `From<Args>` runs there is nothing
/// left to parse — the value IS a [`ValidUrl`]. The static fallback only
/// covers `From<Args>` callers that bypass clap in tests; it is a
/// compile-time constant, not user input.
#[allow(clippy::expect_used)]
fn url_from_args(args: &Args) -> ValidUrl {
    match args.crawler.url.clone() {
        Some(url) => url,
        None => ValidUrl::parse("https://example.com").expect("hardcoded fallback URL must parse"),
    }
}

/// Build the ungated [`AiConfig`](crate::application::crawl_options::AiConfig) — ONE reader for every cargo configuration
/// (ADR-0004, slice C).
///
/// This used to be two `cfg`-gated copies: one that read `threshold`,
/// `offline` and `ai_model`, and one that fell back to
/// `AiConfig::default()` for them. Now that [`AiArgs`] is entirely ungated,
/// the split would only be a second, stale copy of the same reads — and the
/// `..Default` fallback is precisely the silent-reset shape worth deleting: it
/// dropped three operator-supplied values without a word.
fn build_ai_config(args: &Args) -> crate::application::crawl_options::AiConfig {
    crate::application::crawl_options::AiConfig {
        threshold: args.ai.threshold,
        max_chars: args.ai.max_chars,
        offline: args.ai.offline,
        strict_clean_ai: args.ai.strict_clean_ai,
    }
}
