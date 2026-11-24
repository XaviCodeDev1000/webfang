// unwrap/expect are denied workspace-wide ([workspace.lints.clippy], #2354);
// this attribute is the test-only carve-out for this bin's cfg(test) mods.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// Explicit per docs-truthfulness audit (S5): these rustdoc lints are
// allow-by-default, so CI's `-D warnings` never reaches them. Declare intent.
#![allow(rustdoc::missing_crate_level_docs)]
#![allow(rustdoc::private_doc_tests)]
#![allow(rustdoc::unescaped_backticks)]
//! WebFang - Production-ready web scraper with Clean Architecture
//!
//! Extracts clean, structured content from web pages using readability algorithm.
//!
//! # Architecture
//!
//! Following Clean Architecture:
//!
//! ```text
//! main.rs (thin entry point) -> orchestrator::run()
//!     |
//!     ├─→ Args::try_parse()           ← CLI parsing
//!     ├─→ handle_completions()        ← Subcommand handling
//!     ├─→ load_config_defaults()       ← TOML config
//!     ├─→ preflight::normalize()      ← Config merge
//!     ├─→ init_logging_dual()         ← stderr-only tracing
//!     └─→ orchestrator::run()         ← Full pipeline
//! ```
//!
//! **Golden Rule:** The application layer stays UI-free — the CLI, MCP
//! server and AI pipeline are thin adapters over it, never the reverse.

use webfang_core::cli::orchestrator;

use std::panic;
use std::sync::Arc;
#[cfg(feature = "adaptive-selectors")]
use webfang_core::application::adaptive_engine::{AdaptiveSelectorEngine, AdaptiveSelectorOptions};
use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::config::{load_config_defaults, resolve_config_path, ConfigDefaults};
use webfang_core::cli::error::{CliError, CliExit};
use webfang_core::cli::parse::subcommand_args;
use webfang_core::cli::preflight;
use webfang_core::cli::preflight::ArgSources;
use webfang_core::domain::semantic_cleaner::SemanticCleaner;
use webfang_core::infrastructure::platform::init_console_output_encoding;
#[cfg(feature = "adaptive-selectors")]
use webfang_core::infrastructure::scraper::dom_inspector::DefaultDomInspector;
use webfang_core::{init_logging_dual, is_no_color, Args, Commands};

#[path = "main/ai.rs"]
mod ai;
#[path = "main/handlers.rs"]
mod handlers;
mod provider_id;
mod proxy_wire;
#[path = "main/setup.rs"]
mod setup;
#[cfg(feature = "adaptive-selectors")]
use ai::{
    build_adaptive_engine, print_embedding_summary, resolve_tier2_remote_embedding,
    semantic_inspector,
};
use ai::{build_ai_cleaner, resolve_remote_embedding};
use handlers::{handle_connect, handle_doctor, handle_recover, handle_show};
use setup::{
    build_file_trace_layer, emit_orphan_threshold_notice, parse_args,
    resolve_log_level_with_config, resolve_url,
};

use provider_id::generated_provider_id;

#[tokio::main]
pub async fn main() -> CliExit {
    // XP-K-03 (#1808): UTF-8 console code page. FIRST statement of the
    // process, before the tracing subscriber and before any user-facing
    // write, because a code page set after output has started leaves the
    // earlier lines mojibake. Documented no-op off Windows.
    init_console_output_encoding();

    // D6 crash-injection harness: arm BEFORE anything else so every pinned
    // site (even the earliest pipeline stage) sees the same parsed spec.
    // Inert unless WEBFANG_CRASH_AT is set (one OnceLock write).
    webfang_core::cli::crash_points::arm_from_env();

    // Suppress OTel background thread panics during Tokio runtime shutdown.
    // The BatchSpanProcessor and PeriodicReader threads panic when the reactor
    // drops before they finish — this is a known SDK limitation, not our bug.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let thread_name = std::thread::current()
            .name()
            .unwrap_or("unknown")
            .to_string();
        if thread_name.starts_with("OpenTelemetry.") {
            eprintln!("Warning: OTel background thread '{thread_name}' panicked during shutdown (safe to ignore)");
            return;
        }
        default_hook(info);
    }));

    // tokio-console: usa 'cargo install tokio-console' y corre en otra terminal
    // El runtime con tokio[unstable] ya expone el endpoint automaticamente
    __main().await
}

/// Dispatch a parsed subcommand. Split out of `__main` so the top-level
/// function stays under the cognitive-complexity / length ratchet.
async fn dispatch_subcommand(args: &Args) -> CliExit {
    match args.subcommand.as_ref() {
        Some(Commands::Completions { shell }) => orchestrator::handle_completions(*shell),
        Some(Commands::Connect { .. }) => handle_connect(args).await,
        Some(Commands::Doctor) => handle_doctor(args).await,
        Some(Commands::Show) => handle_show(args),
        Some(Commands::Recover { .. }) => handle_recover(args).await,
        // Callers only reach here when a subcommand is present.
        None => CliExit::Success,
    }
}

async fn __main() -> CliExit {
    // 1. Parse CLI arguments (single pass, provenance captured)
    let (mut args, arg_sources) = match parse_args() {
        Ok(v) => v,
        Err(exit) => return exit,
    };

    // 1b. Merge positional URL into --url (positional is syntactic sugar)
    if let Some(pos_url) = args.positional_url.take() {
        args.crawler.url = Some(pos_url);
    }

    // 2. Handle subcommands. `connect` / `doctor` / `show` (issue #1933,
    //    ADR-0004 step 4) run BEFORE URL resolution: they do not touch the
    //    scrape path, and `connect`/`doctor` legitimately run with no URL.
    if args.subcommand.is_some() {
        return dispatch_subcommand(&args).await;
    }

    // 3. URL handling with interactive wizard
    if let Err(exit) = resolve_url(&mut args).await {
        return exit;
    }

    // 5. Load config file. The platform default may be absent (silent
    //    defaults); an explicit WEBFANG_CONFIG must be usable or the run stops
    //    here — a typo must not read as "ran with your config" (#1659).
    let config_defaults = match load_config_defaults() {
        Ok(config) => config,
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };

    // 5a. S10/T2b: ONE governing character budget. An explicit
    // --max-chars/WEBFANG_MAX_CHARS wins; otherwise the effective default
    // profile supplies it (including the operator's [[model_profiles]]
    // overrides). The same resolver feeds `show`/`doctor`, so the number the
    // operator reads is the number the chunker and completion guard enforce.
    let governed_max_chars = match webfang_core::domain::model_profile::governed_max_chars(
        args.ai.max_chars_explicit.then_some(args.ai.max_chars),
        &config_defaults.model_profiles,
    ) {
        Ok(chars) => chars,
        Err(e) => {
            return CliExit::ConfigError(format!(
                "could not resolve the governing character budget: {e}"
            ))
        },
    };
    args.ai.max_chars = governed_max_chars;

    // 5b. Nothing to validate: `--url`/positional are parsed into hardened
    // `ValidUrl` at the argv boundary (#1239), so an invalid or
    // credential-bearing URL is a clap usage error before this point.

    // 6. Trace file (cloned before normalize borrows args)
    let trace_file = args.crawler.trace_file.clone();

    // 6b. Single normalization pipeline (design D3) — replaces legacy merges
    let normalized = match preflight::normalize(&args, &arg_sources, &config_defaults) {
        Ok(n) => n,
        Err(e) => return e,
    };
    // ADR-0004 step 5: capture the deprecation verdict BEFORE `args` is
    // consumed by the projection below. The trigger is a parse-time fact (two
    // env vars clap never sees, plus a `value_source` distinction that does
    // not survive into `AiConfig`), while the emission needs a live subscriber
    // that does not exist yet — so the answer is carried, not the text.
    let pending_notices = args.ai.pending_notices;
    // Project contested fields; copy non-contested from Args via From
    let base = CrawlOptions::from(args);
    // CLI-explicit budget knobs live ONLY in `base` (--concurrency /
    // --batch-concurrency / --download-concurrency / --rate-limit-burst via
    // From<Args>); capture them before the move below (#897 item 1).
    let cli_budget = base.budget_overrides;
    let projected = normalized.into_crawl_options();
    // Overwrite contested fields with provenance-correct values, keep base for the rest
    let mut opts = base;
    opts.crawl.max_pages = projected.crawl.max_pages;
    opts.crawl.max_depth = projected.crawl.max_depth;
    opts.crawl.sitemap_url = projected.crawl.sitemap_url;
    opts.crawl.use_sitemap = projected.crawl.use_sitemap;
    opts.crawl.selector = projected.crawl.selector;
    opts.network.delay_ms = projected.network.delay_ms;
    // #2610: the jitter rides with the delay. Without this copy the TOML
    // channel dies HERE — the preflight stages and the Gate 3 matrix both
    // see the projected value, so only the binary's final merge could drop
    // it, and a TOML-configured jitter would silently keep a fixed cadence.
    opts.network.jitter_ms = projected.network.jitter_ms;
    opts.network.timeout_secs = projected.network.timeout_secs;
    opts.network.concurrency = projected.network.concurrency;
    opts.export.output_dir = projected.export.output_dir;
    opts.export.output_format = projected.export.output_format;
    opts.export.export_format = projected.export.export_format;
    opts.export.obsidian_tags = projected.export.obsidian_tags;
    opts.export.obsidian_wiki_links = projected.export.obsidian_wiki_links;
    opts.export.obsidian_relative_assets = projected.export.obsidian_relative_assets;
    opts.export.obsidian_rich_metadata = projected.export.obsidian_rich_metadata;
    opts.export.obsidian_vault = projected.export.obsidian_vault;
    opts.export.quick_save = projected.export.quick_save;
    opts.crawl.ignore_waf = projected.crawl.ignore_waf;
    // No sitemap coercion here (#1190): `into_crawl_options` already
    // applies the cross-field rule, and `SitemapConfig::resolve` is the
    // single home of the `Some(url) implies intent` rule at projection.
    // Budget overrides (#897 item 1): merge FIELD-WISE, never by slot, with
    // per-tier provenance rules (see `merge_budget_overrides`): the ranked
    // pipeline value wins the crawl tier; explicit CLI knobs win the rest.
    // Before #897 this assignment did not exist at all, so TOML/TUI values
    // were silently dropped from enforcement.
    opts.budget_overrides =
        preflight::merge_budget_overrides(cli_budget, projected.budget_overrides);

    // 6b2. Initialize logging (hoisted above the preflight gates on purpose):
    // the 6c-6f2 gates emit `warn!` diagnostics, and a subscriber is only
    // installed by `init_logging_dual`. A gate that fails fast must leave its
    // reason on stderr — otherwise the WARN would be silently dropped (#796).
    // Still stderr-only, respects quiet + NO_COLOR.
    let no_color = is_no_color();
    let log_level =
        resolve_log_level_with_config(opts.verbosity, config_defaults.log_level.as_deref());
    // #1814: the trace file is an explicitly requested artifact — a failure to
    // create it aborts the run (exit 78) instead of continuing untraced.
    let file_trace_layer = match build_file_trace_layer(trace_file) {
        Ok(layer) => layer,
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };

    // Initialize logging (stderr + optional JSONL file trace layer).
    // #1814: a subscriber that cannot be installed fails the run — silently
    // continuing without the requested log output hid the misconfiguration.
    if let Err(e) = init_logging_dual(log_level, opts.export.quiet, no_color, file_trace_layer) {
        return CliExit::ConfigError(e.to_string());
    }

    // ADR-0004 step 5, the orphan-threshold announcement (spec T5). Split out
    // of `__main` for the same reason `dispatch_subcommand` is: the complexity
    // and length ratchets (#516) apply per function, and the `if` block plus
    // its comment belong in a function named after what it does.
    emit_orphan_threshold_notice(pending_notices);

    // 6c. JS strategy dependency preflight (#685): --js-strategy full needs
    // Chrome installed — fail fast with exit 78 before any crawl starts.
    if let Err(exit) = preflight::check_js_dependencies(&opts) {
        return exit;
    }
    // F-52-c (#1278): pin the launcher to the gate-certified binary. The
    // resolution runs once here (process PATH, post-gate) and travels in
    // CrawlOptions so every chromium launch uses `chrome_executable`
    // instead of chromiumoxide auto-detection (which omits bare
    // `google-chrome` and accepts the `/opt/google/chrome` directory).
    // `None` (non-Full strategies, or a lost post-gate race) keeps
    // historical auto-detection on paths that never run this gate.
    opts.network.chrome_binary = preflight::resolve_chrome_binary();

    // 6d. Elastic sink preflight (#695): --elastic without persistence nor
    // --output-vectors would wire no sink and silently no-op — exit 78.
    if let Err(exit) = preflight::check_elastic_sink(&opts) {
        return exit;
    }

    // 6d2. Orphan --db-path guard (#799): --db-path is a silent no-op without
    // --elastic, so reject it before any crawl starts (exit 78) instead of
    // letting the run persist nothing.
    if let Err(exit) = preflight::check_db_path_requires_elastic(&opts) {
        return exit;
    }

    // 6e2. The `--adaptive-selectors` capability gate (#1813) is NOT here.
    //
    // It used to be this step, mirroring 6e. That was a fail-open waiting to
    // happen: a check wired into ONE caller is skipped by every other caller,
    // and the sibling `--rate-limit-burst` fix (#1813 T1) had already moved its
    // validation into the shared staging pipeline for exactly that reason. The
    // gate now runs inside `preflight::normalize` (step 6b, above), which every
    // `Args` front door traverses.
    //
    // This call site was provably dead once that moved: `normalize` runs at
    // :116 and returns before reaching here, and `From<Args>` copies
    // `args.crawler.adaptive_selectors` into `opts.adaptive_selectors`
    // unchanged, so `opts.adaptive_selectors` cannot be true when `normalize`
    // passed. Keeping it would have left a reader believing this was the
    // enforcement point. `preflight::check_adaptive_selectors_feature` remains
    // public for a caller that builds `CrawlOptions` directly.

    // 6f. Obsidian vault/output conflict preflight (#762): an explicit
    // --vault redirect cannot coexist with a custom non-default -o/WEBFANG_OUTPUT.
    if let Err(exit) = webfang_core::cli::commands::validate_vault_output_conflict(&opts) {
        return exit;
    }

    // 6f2. Vector export preflight (#796): --export-format vector without
    // --clean-ai would write an invalid export.json (no embeddings) yet
    // report success — fail fast before any network I/O, mirroring the
    // output-vectors gate (#703). Placed here (after the #762 vault gate,
    // before the orchestrator) to match the fail-fast philosophy of the other
    // 6c-6f gates; dry-run is not exempt for the same reason.
    if let Err(exit) = preflight::check_export_format_vector(&opts) {
        return exit;
    }

    // 8. Build optional engines and delegate to orchestrator
    build_and_run(opts).await
}
async fn build_and_run(opts: CrawlOptions) -> CliExit {
    // Build LLM provider if --extract-with-llm is set.
    // This is NOT gated by the `ai` feature — the remote provider port lives
    // in webfang_core unconditionally (docs/architecture/ai-providers-design.md §8).
    let llm_port = if opts.extract_with_llm {
        let providers_cfg = ConfigDefaults::load(&resolve_config_path());
        let registry = webfang_core::domain::providers::ProviderRegistry::new(
            webfang_core::domain::providers::ProvidersConfig {
                providers: providers_cfg.providers.clone(),
            },
        );
        let provider_cfg = match registry.resolve_default_completion() {
            Ok(cfg) => cfg,
            Err(e) => {
                return CliExit::ConfigError(format!(
                    "--extract-with-llm activado pero no hay proveedor completion configurado: {e}"
                ));
            },
        };
        let provider = match proxy_wire::build_completion_provider(&providers_cfg, provider_cfg) {
            Ok(p) => p,
            Err(e) => return e,
        };

        // Build Container and inject provider — validates startup per §8b
        let container = match webfang_core::application::container::Container::new(
            webfang_core::domain::CrawlerConfig::new(opts.url.as_url().clone()),
            webfang_core::domain::config::ScraperConfig::default(),
        )
        .await
        {
            Ok(c) => c,
            Err(e) => return CliExit::ConfigError(format!("contenedor: {e}")),
        }
        .with_llm_port(Arc::new(provider));

        // DEBE §8b: validar en startup que el puerto está disponible
        let llm_port = match container.llm_port() {
            Some(p) => p,
            None => {
                return CliExit::ConfigError(
                    "fallo interno: llm_port() es None tras inyección".to_string(),
                );
            },
        };
        Some(llm_port)
    } else {
        None
    };

    // Resolve the embedding slot ONCE (ADR-0004 paso 6): the vault-search
    // slot and the `--clean-ai` cleaner are the SAME port, so the endpoint
    // is probed once per run instead of twice.
    let embedding_port = match resolve_remote_embedding(&opts).await {
        Ok(port) => port,
        Err(e) => return e,
    };

    let (ai_cleaner, vault_ports) = match build_ai_cleaner(&opts, embedding_port) {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Resolve Tier 2 remote embedding (separate flag, separate privacy domain)
    // Only when adaptive-selectors is enabled, but defined here so it's
    // accessible after the orchestrator call for metrics collection.
    #[cfg(feature = "adaptive-selectors")]
    let tier2_remote: Option<webfang_core::domain::embedding_port::Tier2EmbeddingPort> = {
        match resolve_tier2_remote_embedding(&opts).await {
            Ok(Some(adapter)) => Some(
                webfang_core::domain::embedding_port::Tier2EmbeddingPort::new(
                    adapter as Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>,
                ),
            ),
            Ok(None) => None,
            Err(e) => return e,
        }
    };
    #[cfg(not(feature = "adaptive-selectors"))]
    let _tier2_remote: Option<webfang_core::domain::embedding_port::Tier2EmbeddingPort> = None;

    #[cfg(feature = "adaptive-selectors")]
    {
        // Inference worker bound derives from the budget model's Operation.inference
        // tier (task 2.5d); explicit operator flags arrive via BudgetOverrides.
        let engine_options = {
            let budget = webfang_core::domain::budget::BudgetModel::build(
                opts.budget_overrides,
                &webfang_core::domain::budget::detector::SystemDetector,
            );
            AdaptiveSelectorOptions {
                max_concurrent_inference: budget.inference().get(),
                ..AdaptiveSelectorOptions::default()
            }
        };
        // Tier 2 has its OWN privacy domain (issue #1977): the inspector
        // binds ONLY to an explicit `--tier2-embedding-provider`. The vault
        // slot's provider is never adopted here — it is a DIFFERENT type
        // (`Tier2EmbeddingPort` vs `Option<Arc<dyn EmbeddingPort>>`), so
        // `vault_ports.embedding_port` cannot reach `semantic_inspector` even
        // by accident. The local ONNX pool that used to serve this slot is
        // gone (ADR-0004 paso 6), so the explicit remote is the only producer.
        let semantic = semantic_inspector(tier2_remote.clone(), &engine_options);
        // Build adaptive engine inside the cfg block so semantic and engine_options are in scope
        let adaptive_engine = build_adaptive_engine(&opts, semantic, engine_options);

        let threshold = opts.ai_config.threshold;
        let exit =
            orchestrator::run(opts, ai_cleaner, adaptive_engine, vault_ports, llm_port).await;
        // Print embedding summary if Tier 2 remote was used
        if let Some(ref tier2) = tier2_remote {
            print_embedding_summary(tier2.port(), threshold).await;
        }
        exit
    }
    #[cfg(not(feature = "adaptive-selectors"))]
    {
        // No adaptive engine to feed — call orchestrator without it.
        // The `adaptive-selectors` feature gates the `AdaptiveSelectorOptions` type,
        // so we don't define a local copy here anymore.
        orchestrator::run(opts, ai_cleaner, vault_ports, llm_port).await
    }
}

#[cfg(test)]
#[path = "main/log_level_tests.rs"]
mod log_level_tests;
#[cfg(test)]
#[path = "main/wiring_tests.rs"]
mod wiring_tests;
