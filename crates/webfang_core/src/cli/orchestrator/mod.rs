//! CLI orchestrator — coordinates the main scraping pipeline.
//!
//! Orchestrates URL discovery, scraping, and export phases.

use tracing::{info, instrument};

use crate::application::crawl_options::CrawlOptions;
use crate::cli::elastic::run_elastic_ingestion;
use crate::cli::error::CliExit;
use crate::cli::exit_codes::report_phase;

use crate::domain;

pub use crate::cli::parse::handle_completions;

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;
use crate::domain::semantic_cleaner::SemanticCleaner;

mod export_phase;
mod persistence;
mod phases;

pub(crate) use export_phase::{export_phase, export_phase_with_resume};
pub(crate) use persistence::resolve_persistence_root;
pub(crate) use phases::{output_vectors_gate, parse_asset_h2_profile};

#[cfg(test)]
#[path = "orchestrator_persistence_tests.rs"]
mod persistence_tests;
#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod tests;

use export_phase::truncated_discovery_exit;
use phases::{prepare_resume_route, run_batch_route, run_dry_run, run_scrape_route};
/// Main orchestration entry point.
///
/// Coordinates the full scraping pipeline:
/// 1. URL discovery + config preparation
/// 2. Scraping with progress
/// 3. Export results
/// 4. Report failures + exit code
#[allow(clippy::too_many_lines)]
#[instrument(
    level = "info",
    skip(opts, ai_cleaner, adaptive_engine, vault_ports, llm_port),
    fields(
        url = %opts.url,
        correlation_id = tracing::field::Empty,
        trace_id = tracing::field::Empty
    )
)]
pub async fn run(
    opts: CrawlOptions,
    ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    #[cfg(feature = "adaptive-selectors")] adaptive_engine: Option<
        std::sync::Arc<AdaptiveSelectorEngine>,
    >,
    vault_ports: crate::application::container::VaultAiPorts,
    llm_port: Option<std::sync::Arc<dyn crate::domain::llm_port::LlmPort>>,
) -> CliExit {
    // Run-root correlation identity (#501, #1439): the whole operation owns
    // ONE root, minted HERE at the orchestration entry and propagated into
    // every route — dry-run preview, batch, prepare/scrape phases. Before
    // #1439 the discovery Engine minted its own root milliseconds after this
    // event, silently splitting every DOM/batch run into two identities in
    // the trace (the historical #687 class). `#[instrument]` sees arguments
    // only, and the root is a LOCAL minted after span creation, so it is
    // declared `Empty` and recorded on the span right here (#1610, OBS-H1):
    // the root `run` span now carries its own identity, and the `run
    // identity` event below (kept as the historical contract other tooling
    // reads) inherits it through `span_fields`.
    let root_correlation = domain::CorrelationId::new();
    let run_span = tracing::Span::current();
    run_span.record(
        "correlation_id",
        tracing::field::display(root_correlation.to_string()),
    );
    run_span.record(
        "trace_id",
        tracing::field::display(root_correlation.trace_id()),
    );
    info!(
        correlation_id = %root_correlation,
        trace_id = %root_correlation.trace_id(),
        "run identity"
    );

    if opts.export.dry_run {
        return run_dry_run(opts, &root_correlation).await;
    }

    // #703/#652: pre-flight gate for `--output-vectors` — single source of
    // truth shared with `run_batch()` (see `output_vectors_gate`), and it must
    // run BEFORE `build_elastic_ingestion` wires the stream sink below.
    if let Some(exit) = output_vectors_gate(&opts) {
        return exit;
    }

    // #796: pre-flight gate for `--export-format vector` without `--clean-ai` —
    // mirrors the CLI preflight in `main.rs` so the MCP / batch path is also
    // covered (defense in depth: no invalid `export.json` with `dimensions: null`).
    if let Err(exit) = crate::cli::preflight::check_export_format_vector(&opts) {
        return exit;
    }

    // Process-level graceful shutdown (#653). The guard owns ONE signal
    // listener for the whole run; every phase observes its token cooperatively
    // so a SIGINT drains in-flight work and still exports it, instead of being
    // ignored until the operator escalates to SIGKILL.
    let shutdown = crate::cli::shutdown::ShutdownGuard::install();
    let cancel = shutdown.token();

    if opts.batch.enabled {
        return run_batch_route(
            opts,
            ai_cleaner,
            vault_ports,
            llm_port,
            &cancel,
            &root_correlation,
        )
        .await;
    }

    let (prepare, state_store) = match prepare_resume_route(&opts, &root_correlation).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    // #2338: the reason only turns into an exit code AFTER the partial output is written.
    let discovery_truncated_by = prepare.truncated_by.clone();
    let urls_to_scrape = prepare.urls_to_scrape.clone();

    let (results, failures, blocked, elastic_ingestion) = match run_scrape_route(
        &opts,
        &prepare,
        &urls_to_scrape,
        vault_ports,
        #[cfg(feature = "adaptive-selectors")]
        adaptive_engine,
        &root_correlation,
        &cancel,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Issue #1814 (slice B, AC4): the export runs on the blocking pool, which
    // requires 'static data, while `report_phase` still needs the pages after
    // the export. Share the slice: `Vec → Arc<[ScrapedContent]>` moves the
    // allocation, so downstream phases pay one refcount — never a content
    // clone.
    let results: std::sync::Arc<[domain::ScrapedContent]> = std::sync::Arc::from(results);

    if let Some(ref ingestion) = elastic_ingestion {
        if let Err(e) = run_elastic_ingestion(ingestion, &results).await {
            // #1949: canonical classify → exit routing (matrix row 26), never
            // a hardcoded IoError (74) — a transient backend failure is
            // "unavailable, retry" (69).
            return crate::cli::error::ingestion_exit_for(&e);
        }
    }

    // Release the ingestion pipeline (wreq connection pool + Rayon threads)
    // while the runtime is still active. Without this, the hyper pool
    // background task or Rayon thread join can block runtime shutdown.
    // See issue #335.
    drop(elastic_ingestion);
    tokio::task::yield_now().await;

    // #779: export the successfully-scraped pages BEFORE the report/exit
    // decision. Previously `report_phase` short-circuited on partial success
    // (some pages failed, some succeeded) and `export_phase` never ran — so a
    // partial-success crawl silently discarded all its content (exit 69 with an
    // empty output directory), unlike batch mode which always exports.
    let export_exit = export_phase(
        std::sync::Arc::clone(&results),
        &opts,
        state_store.as_deref(),
        ai_cleaner,
        &cancel,
    )
    .await;

    // Retention after the export wrote its artifacts (#1827): runs on both
    // the success and the cancelled path (export already ran there too).
    // Best-effort — never changes the run's exit code.
    crate::cli::retention::apply_retention(&opts).await;

    // Special cell — Cancelled (error-classification-matrix): cooperative
    // cancellation is a control signal, not an operational failure, so it
    // wins over classification-based routing below — a Ctrl-C mid-crawl
    // exits 0 even with partial failures (#509 semantics at the CLI
    // boundary). Export above already ran: captured content is still
    // written (graceful shutdown).
    if let Some(exit) = crate::cli::error::cancelled_exit(cancel.is_cancelled()) {
        return exit;
    }

    // Special cell — PartialRun (error-classification-matrix, #2338): the
    // discovery crawl ended early but the partial set above was already
    // scraped and exported. The operator gets BOTH the written output and
    // the failure (InternalFatal → 69), never a silent success.
    if let Some(reason) = discovery_truncated_by {
        return truncated_discovery_exit("los resultados parciales fueron escritos", &reason);
    }

    if let Some(exit) = report_phase(&results, &failures, blocked, opts.verbosity) {
        return exit;
    }

    export_exit
}
