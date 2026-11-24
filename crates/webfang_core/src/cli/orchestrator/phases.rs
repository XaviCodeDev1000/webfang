//! The run phases around the orchestrator hub: the `--output-vectors`
//! pre-flight gate, the dry-run preview, the prepare/resume and scrape
//! routes, the scrape phase and the asset H2/TLS profile resolver.

use super::export_phase::{resume_nothing_pending, truncated_discovery_exit};
use super::persistence::resolve_persistence_mode;
use crate::application::adaptive_engine::AdaptiveSelectorEngine;
use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::CapturedPage;
use crate::cli::batch_flow::{load_batch_manager, run_batch};
use crate::cli::discovery_phase::{
    build_crawler_config_for_discovery, prepare_phase, PrepareResult,
};
use crate::cli::elastic::build_elastic_ingestion;
use crate::cli::error::CliExit;
use crate::cli::scrape_flow::{apply_resume_mode, scrape_urls};
use crate::cli::url_discovery::DiscoveryRetry;
use crate::domain;
use crate::domain::config::ScraperConfig;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::repository::DynVectorRepository;
use crate::domain::semantic_cleaner::SemanticCleaner;
use tracing::{info, warn};

/// Run dry-run: discover URLs and print them without scraping.
pub(super) async fn run_dry_run(
    opts: CrawlOptions,
    root_correlation: &domain::CorrelationId,
) -> CliExit {
    let tls_emulation = match HttpClientConfig::profile_from_name(&opts.network.h2_profile) {
        Ok(profile) => profile,
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };
    let crawler_config = match build_crawler_config_for_discovery(&opts, tls_emulation) {
        Ok(config) => config,
        Err(e) => return e,
    };

    // #784: with --batch-file, opts.url is empty, so discovering from it would
    // report "0 URL(s) would be scraped". List the batch URLs the user actually
    // supplied instead — that is the set a dry run should preview.
    if opts.batch.batch_file.is_some() {
        let budget = crate::domain::budget::BudgetModel::build(
            opts.budget_overrides,
            &crate::domain::budget::detector::SystemDetector,
        );
        let manager = match load_batch_manager(&opts, crawler_config, &budget).await {
            Ok(m) => m,
            Err(e) => return e,
        };
        let urls = manager.urls();
        info!(
            "Dry-run: listing {} batch URL(s) without scraping",
            urls.len()
        );
        println!("\nDry-run: {} URL(s) would be scraped:", urls.len());
        for url in &urls {
            println!("  {url}");
        }
        return CliExit::Success;
    }

    // F-14 (#1232 slice 1): dry-run shares the unified recursive discovery
    // with the real DOM path, so `--max-depth` is honored in previews.
    // dry-run-fail-fast: a preview reports seed reachability, so it runs a
    // single attempt with zero backoff sleeps (`FailFast`) instead of
    // spending the full operator retry budget before exiting 69.
    info!("Dry-run: discovering URLs without scraping...");
    let persistence_mode = resolve_persistence_mode(&opts);
    let output = match crate::cli::url_discovery::discover_urls_unified(
        crawler_config,
        &opts,
        &persistence_mode,
        None,
        root_correlation,
        DiscoveryRetry::FailFast,
    )
    .await
    {
        Ok(output) => output,
        Err(e) => return CliExit::NetworkError(format!("URL discovery failed: {e}")),
    };
    let discovered = &output.urls;

    // #1381: a seed the SSRF guard cuts opens no socket, so discovery completes
    // `Ok` with zero URLs and the preview below reported "nothing to scrape" —
    // exit 0 — for what is actually a policy refusal. Say so and leave with the
    // null-result code the sitemap arms already use for this shape (2), instead
    // of letting automation read a refusal as an empty site. Only the literal
    // entry layer is named: a hostname refused by the connect-time resolver is
    // indistinguishable here from a real DNS failure.
    if discovered.is_empty() {
        if let Some(exit) =
            crate::cli::error::empty_discovery_exit_when_seed_refused(opts.url.as_url())
        {
            return exit;
        }
    }

    // #1443: a dry-run that discovered nothing AND counted fetch errors means
    // the seed itself failed — the Engine consumes per-page failures into
    // `CrawlResult.errors` and completes `Ok`, so without this the preview
    // printed `Dry-run: 0 URL(s) would be scraped:` and exited 0 for a dead
    // seed. Precondition (single seed): dry-run without `--batch-file`
    // discovers from `opts.url` alone — the batch shape returns above — so
    // nothing is discoverable without fetching the seed, and a live seed
    // always lands in `urls` even when it carries no links. Runs AFTER the
    // #1381 guard above (strict SSRF-first order): a refused seed already
    // returned exit 2, so reaching here with errors means a real dial failed.
    // Reuses the `NetworkError` message/exit 69 the `Err` arm already uses.
    if discovered.is_empty() && output.errors > 0 {
        return CliExit::NetworkError(format!(
            "URL discovery failed: the seed did not respond ({} crawl error(s), 0 URLs discovered)",
            output.errors
        ));
    }

    println!("\nDry-run: {} URL(s) would be scraped:", discovered.len());
    for url in discovered {
        println!("  {url}");
    }

    // #2338: a preview whose discovery crawl ended early lists the partial
    // set (above) and still exits as a failure — never a silent exit 0.
    if let Some(reason) = output.truncated_by {
        return CliExit::NetworkError(format!(
            "the preview ended early (partial URLs listed above): {reason}"
        ));
    }
    CliExit::Success
}

/// Run the scraping loop over all URLs with progress events.
///
/// # Errors
///
/// Returns [`crate::error::ScraperError`] if the configured H2/TLS profile name
/// is not recognized or the fetch router's HTTP client cannot be built (a setup
/// failure, before any URL is scraped).
#[allow(clippy::too_many_arguments)]
pub(super) async fn scrape_phase(
    urls: &[url::Url],
    scraper_config: &ScraperConfig,
    opts: &CrawlOptions,
    observer: &dyn crate::application::progress_observer::ProgressObserver,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    engine: Option<&AdaptiveSelectorEngine>,
    root_correlation: &domain::CorrelationId,
    cancel: &tokio_util::sync::CancellationToken,
    captured: &[CapturedPage],
) -> Result<
    (
        Vec<domain::ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
        usize,
    ),
    crate::error::ScraperError,
> {
    scrape_urls(
        urls,
        scraper_config,
        opts,
        observer,
        downloader,
        engine,
        root_correlation,
        cancel,
        captured,
    )
    .await
}

/// The prepare + resume route of [`super::run`]: resolve the persistence
/// mode, run discovery/preparation, apply the resume filter, and evaluate
/// the "nothing pending" (#705 Step 2) and truncated-discovery (#2338)
/// short-circuits.
///
/// On success returns the prepared phase result — with the resume-FILTERED
/// url list in [`PrepareResult::urls_to_scrape`] — plus the optional state
/// store. `Err` carries the early [`CliExit`] (discovery failure,
/// nothing-pending success, truncated discovery).
pub(super) async fn prepare_resume_route(
    opts: &CrawlOptions,
    root_correlation: &domain::CorrelationId,
) -> Result<
    (
        PrepareResult,
        Option<std::sync::Arc<dyn crate::domain::persistence::StateStorePort>>,
    ),
    CliExit,
> {
    // PersistenceMode unified control-plane — pure resolver with default dir.
    // Built BEFORE prepare_phase so discovery Engine can be wired with
    // `with_persistence` (checkpoint interval flows from the mode, not hardcoded).
    // The resolver itself never logs (#1045): `--state-dir` without `--resume`
    // is reported via `ResolverNotes` and warned about here, the one call
    // site that knows about user flags.
    let persistence_mode = resolve_persistence_mode(opts);

    let prepare = match prepare_phase(opts, &persistence_mode, root_correlation).await {
        Err(e) => return Err(e),
        Ok(p) => p,
    };
    // Destructured so the discovered list can move into `apply_resume_mode`
    // while the rest of the struct travels on to the scrape route.
    let PrepareResult {
        urls_to_scrape: discovered_urls,
        scraper_config,
        shared_downloader,
        captured_pages,
        truncated_by: discovery_truncated_by,
    } = prepare;
    // #2338: the reason only turns into an exit code AFTER the partial output
    // is written — it rides along on the returned struct.
    let discovered_count = discovered_urls.len();
    let (urls_to_scrape, state_store) = match apply_resume_mode(
        discovered_urls,
        &persistence_mode,
        opts.url.as_str(),
        root_correlation,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return Err(e),
    };

    // #705 Step 2: a --resume run where every discovered URL was already
    // processed by a prior run is a technical success ("nothing pending").
    // #2338: a TRUNCATED discovery never reports Success — the run failed
    // even when nothing is pending (the prior run's output stays on disk, so
    // no export claim is made here; the reason is the review finding from
    // lineage review-e9b0d70105d5b474).
    if let Some(exit) =
        resume_nothing_pending(opts, &urls_to_scrape, discovered_count, root_correlation)
    {
        if let Some(reason) = &discovery_truncated_by {
            return Err(truncated_discovery_exit(
                "nothing pending to reprocess, but discovery did not complete",
                reason,
            ));
        }
        return Err(exit);
    }

    Ok((
        PrepareResult {
            urls_to_scrape,
            scraper_config,
            shared_downloader,
            captured_pages,
            truncated_by: discovery_truncated_by,
        },
        state_store,
    ))
}

/// The scrape route of [`super::run`]: wire the elastic ingestion pipeline,
/// build the headless progress observer, bridge the cfg-gated adaptive
/// engine into an always-present reference, and run the scrape phase.
///
/// Returns the scraped contents, the per-URL failures, the blocked count and
/// the optional ingestion pipeline. `Err` is a config error (exit 78) when
/// the scrape setup fails — unknown TLS profile or HTTP client build error —
/// so the run never silently scrapes with a wrong fingerprint.
pub(super) async fn run_scrape_route(
    opts: &CrawlOptions,
    prepare: &PrepareResult,
    urls_to_scrape: &[url::Url],
    vault_ports: crate::application::container::VaultAiPorts,
    #[cfg(feature = "adaptive-selectors")] adaptive_engine: Option<
        std::sync::Arc<AdaptiveSelectorEngine>,
    >,
    root_correlation: &domain::CorrelationId,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<
    (
        Vec<domain::ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
        usize,
        Option<
            std::sync::Arc<
                crate::application::elastic_ingestion::ElasticIngestion<DynVectorRepository>,
            >,
        >,
    ),
    CliExit,
> {
    let elastic_ingestion = match build_elastic_ingestion(opts, vault_ports).await {
        Ok(v) => v,
        Err(e) => return Err(e),
    };

    // Create observer with stderr fallback (no channel) for non-TUI mode:
    // scraping runs fully headless; there is no TUI progress screen.
    let observer = Box::new(
        crate::application::progress_observer::LiveProgressObserver::new(None, opts.export.quiet),
    );
    // Bridge the cfg-gated engine into an always-present reference option for
    // the scrape phase: `None` when the feature is compiled out.
    #[cfg(feature = "adaptive-selectors")]
    let engine_ref = adaptive_engine.as_deref();
    #[cfg(not(feature = "adaptive-selectors"))]
    let engine_ref: Option<&AdaptiveSelectorEngine> = None;

    let (results, failures, blocked) = match scrape_phase(
        urls_to_scrape,
        &prepare.scraper_config,
        opts,
        observer.as_ref(),
        prepare
            .shared_downloader
            .as_deref()
            .map(|d| d as &dyn crate::domain::ports::AssetDownloaderPort),
        engine_ref,
        root_correlation,
        cancel,
        &prepare.captured_pages,
    )
    .await
    {
        Ok(pair) => pair,
        // Setup failures (unknown TLS profile, HTTP client build error) are
        // config errors: surface the message and exit 78 rather than silently
        // scraping with a wrong fingerprint.
        Err(e) => return Err(CliExit::ConfigError(e.to_string())),
    };

    Ok((results, failures, blocked, elastic_ingestion))
}

/// Parse H2/TLS profile from CLI string for the asset download path.
///
/// Delegates to the domain resolver
/// [`crate::domain::profile::profile_from_name`], which accepts the full
/// [`wreq_util::Profile`] catalog. Unlike the strict page-fetch path, the asset
/// path is best-effort: an unknown name logs a warning and falls back to
/// `Chrome145` rather than failing the run.
pub(crate) fn parse_asset_h2_profile(s: &str) -> wreq_util::Profile {
    crate::domain::profile::profile_from_name(s).unwrap_or_else(|| {
        tracing::warn!(
            profile = %s,
            fallback = "Chrome145",
            "unknown asset H2 profile; falling back to Chrome145 (see `cargo doc -p wreq-util` for all profiles)"
        );
        wreq_util::Profile::Chrome145
    })
}

/// Pre-flight gate for `--output-vectors` (#703, #652).
///
/// `--output-vectors` can only write embeddings when semantic cleaning is
/// requested (`--clean-ai` / `opts.ai`). Every entry point (`run`,
/// `run_batch`) must run this gate BEFORE `build_elastic_ingestion` wires the
/// stream sink: `StreamRepository::new(path)` creates/truncates the target
/// file as a construction side effect, so failing downstream of it leaks a
/// 0-byte vectors file alongside a success exit (silent data loss for RAG
/// pipelines, class S1).
///
/// - `output_vectors && !opts.ai` → `DataFormatError` (exit 65, `EX_DATA`)
///   with a user-facing message.
///
/// Returns `Some(exit)` when the run must be aborted, `None` to proceed.
pub(crate) fn output_vectors_gate(opts: &CrawlOptions) -> Option<CliExit> {
    opts.elastic.output_vectors.as_ref()?;

    if opts.ai {
        return None;
    }
    warn!("--output-vectors refused without --clean-ai; no vectors to export");
    Some(CliExit::DataFormatError(
        "No vectors to export: '--output-vectors' requires '--clean-ai' to generate embeddings"
            .to_string(),
    ))
}

/// Batch route of [`run`]: delegates to `run_batch` carrying the run-root
/// identity (#1439) and applies retention afterwards (#1827).
pub(super) async fn run_batch_route(
    opts: CrawlOptions,
    ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    vault_ports: crate::application::container::VaultAiPorts,
    llm_port: Option<std::sync::Arc<dyn crate::domain::llm_port::LlmPort>>,
    cancel: &tokio_util::sync::CancellationToken,
    root_correlation: &domain::CorrelationId,
) -> CliExit {
    // #1439: the run-root minted above travels through `run_batch_crawl`
    // onto the `BatchProcessor`, so every per-URL crawl engine of one
    // batch run shares this single identity instead of minting its own
    // (the old per-crawl mint split the batch trace in two).
    // `opts` is cloned for the batch call so retention can still read
    // the flag values afterwards (#1827); the copy is negligible next
    // to the batch run itself.
    let exit = run_batch(
        opts.clone(),
        ai_cleaner,
        vault_ports,
        llm_port,
        cancel,
        root_correlation,
    )
    .await;
    // Retention after the batch exported (#1827): best-effort, never
    // changes the run's exit code.
    crate::cli::retention::apply_retention(&opts).await;
    exit
}
