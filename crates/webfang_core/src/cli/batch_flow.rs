//! CLI batch pipeline — the `--batch` lifecycle end to end.
//!
//! Extracted from `cli/orchestrator.rs` as part of the composition-root
//! decomposition (issue #1619, finding F1). The run lifecycle lives here: the
//! per-URL crawl, content extraction and the elastic/resume wiring. The
//! bounded capture spool's acquisition path (observe → decide → act) is the
//! private `batch_spool` child and the batch manager assembly — including the
//! batch crawl-config projection — is the private `batch_manager` child
//! (Ola 3 B4, #2966).
//!
//! The module owns no new policy. The exit-code decision stays in
//! [`crate::cli::exit_codes`], the canonical severity mapping in
//! [`crate::cli::error`], and the shared gates (`output_vectors_gate`,
//! `export_phase`, the persistence-root resolver) in
//! [`crate::cli::orchestrator`] — so both CLI flows keep one implementation of
//! each rule.

use tracing::{error, info, warn};

use super::batch_summary::log_batch_summary;
use crate::application::batch::BatchManagerSummary;
use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::BoundedFileSink;
use crate::cli::elastic::{build_elastic_ingestion, run_elastic_ingestion};
use crate::cli::error::CliExit;
use crate::cli::exit_codes::{batch_exit_code, report_phase};
use crate::cli::orchestrator::{output_vectors_gate, resolve_persistence_root};
use crate::domain;
use crate::domain::config::ScraperConfig;
use crate::domain::persistence::StateStorePort;

use crate::domain::semantic_cleaner::SemanticCleaner;

#[path = "batch_spool.rs"]
mod batch_spool;

use self::batch_spool::{build_batch_sink, discard_batch_spool};
pub(crate) use batch_spool::BATCH_SPOOL_FILE;

#[path = "batch_manager.rs"]
mod batch_manager;

use self::batch_manager::{prepare_batch_manager, resolve_batch_tls_emulation};
pub(crate) use batch_manager::load_batch_manager;

/// Run batch processing mode: scrape multiple URLs from stdin or file.
///
/// #1215: `--batch` SCRAPES each input URL (exactly one page per URL) — it
/// never BFS-crawls seeds. The crawl-expansion knobs (`--max-depth`,
/// `--max-pages`, `--sitemap`) do NOT apply here; a non-default value earns
/// a loud warning in [`prepare_batch_manager`] instead of silently changing
/// the run. `--single-page` is honored trivially: single-page is what batch
/// always does.
///
/// The batch pipeline spools every fetched page body through a shared
/// [`BoundedFileSink`] and then runs the full export / elastic / resume
/// pipeline — the same stages `run()` applies to single-page mode, so
/// `--batch` actually writes `.md` + `.jsonl` and honors `--elastic` /
/// `--resume` (#631, #637), with bounded memory (#653).
pub(crate) async fn run_batch(
    opts: CrawlOptions,
    ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    vault_ports: crate::application::container::VaultAiPorts,
    _llm_port: Option<std::sync::Arc<dyn crate::domain::llm_port::LlmPort>>,
    cancel: &tokio_util::sync::CancellationToken,
    root_correlation: &domain::CorrelationId,
) -> CliExit {
    // #703/#652: pre-flight gate for `--output-vectors` — same single source of
    // truth as `run()` (see `output_vectors_gate`). First statement here so the
    // exit fires before any crawl, spool, or sink wiring runs.
    if let Some(exit) = output_vectors_gate(&opts) {
        return exit;
    }

    // #796: same `export_format vector` gate as `run()` — covers the batch path
    // so `--batch --export-format vector` without `--clean-ai` also fails fast.
    if let Err(exit) = crate::cli::preflight::check_export_format_vector(&opts) {
        return exit;
    }

    // Resolve the TLS/H2 fingerprint once so the batch crawl engine honors
    // `--h2-profile` (#312). An unknown profile is a config error (exit 78),
    // matching the scrape phase — never silently crawl with a wrong fingerprint.
    let tls_emulation = match resolve_batch_tls_emulation(&opts) {
        Ok(profile) => profile,
        Err(e) => return e,
    };

    let (summary, sink) =
        match run_batch_crawl(&opts, tls_emulation, cancel, root_correlation).await {
            Ok(pair) => pair,
            Err(e) => return e,
        };

    let extracted = extract_batch_content(&sink, &opts).await;
    discard_batch_spool(&sink).await;
    // The spool is gone: the sidecar describing it goes with it, so the next
    // run's acquisition sees a clean pair instead of a stale sidecar.
    let _ = crate::cli::batch_flush::clear_sidecar(&resolve_persistence_root(&opts)).await;
    let (results, failures) = match extracted {
        Ok(pair) => pair,
        Err(e) => return e,
    };

    // Issue #1814 (slice B, AC4): share the slice with the blocking-pool
    // export (same rationale as orchestrator::run) — `batch_exit_code` still
    // needs `results.len()` after the export, and an `Arc` move never clones
    // page content.
    let results: std::sync::Arc<[domain::ScrapedContent]> = std::sync::Arc::from(results);

    // Resume mode (#637): construct the state store so `export_phase` can mark
    // each URL as processed — no URL filtering, they were already crawled.
    let state_store = match build_batch_resume_store(&opts) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let elastic_ingestion = match build_elastic_ingestion(&opts, vault_ports).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    if let Err(e) = run_batch_elastic(&elastic_ingestion, &results).await {
        return e;
    }

    // Print extraction failures to stderr (crawl failures are already logged
    // via `log_batch_summary`). Do NOT short-circuit here: always export the
    // pages we did capture so `--batch` writes `.md` + `.jsonl` even on partial
    // failure (#631). The batch path has no robots-blocked counter — its crawl
    // engine reports blocks through `summary` — so pass 0.
    let _ = report_phase(&results, &failures, 0, opts.verbosity);

    // WU4: with `--export-checkpoint` the flush already COMMITTED the
    // flushed pages into the batch-namespace store; turn the resume gate
    // on at the clean close so they skip instead of double-appending.
    let resume_gate = opts.crawl.resume || opts.batch.export_checkpoint > 0;
    let export_exit = crate::cli::orchestrator::export_phase_with_resume(
        std::sync::Arc::clone(&results),
        &opts,
        state_store.as_deref(),
        ai_cleaner,
        // The RUN's token (#2244 decision A), received from `run()` where
        // `ShutdownGuard::install()` owns it: it already drives the crawls
        // above, and now the export observes that same one. The batch flow
        // does not mint a token of its own.
        cancel,
        resume_gate,
    )
    .await;

    // Final exit code aggregates BOTH crawl-level and extraction-level outcomes
    // with `#537` severity routing: partial success -> 69, all-fail with an
    // internal fatal error -> 3, otherwise 0. Crawl failures were only logged
    // above, so this is the only place the batch's true status surfaces. A
    // failed export (`IoError` 74, `ConfigError` 78, ...) surfaces only when
    // that aggregate is `Success`, mirroring the single-run path's export
    // ordering (orchestrator.rs) — a dropped export exit here used to make a
    // batch with a failed export exit 0 (#1820).

    // Special cell — Cancelled: same precedence as the single-run path —
    // cancellation beats both the classification-based routing and the export
    // exit, and exits 0.
    if let Some(exit) = crate::cli::error::cancelled_exit(cancel.is_cancelled()) {
        return exit;
    }

    let total_failed = summary.failed + failures.len();
    let mut all_errors = summary.errors;
    all_errors.extend(failures);
    let batch_exit = batch_exit_code(results.len(), total_failed, &all_errors);
    if !matches!(batch_exit, CliExit::Success) {
        return batch_exit;
    }
    export_exit
}

/// Scrape every batch URL (one page per URL, #1215), spooling each fetched
/// body to disk, and return the run summary plus the sink holding the spool.
/// Performs the no-URL / no-content guards so `--batch` fails loudly instead
/// of writing nothing (#631).
///
/// The sink is a [`BoundedFileSink`], not an in-memory buffer: a large batch of
/// heavy pages must not grow the resident set without a ceiling (#653).
async fn run_batch_crawl(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    cancel: &tokio_util::sync::CancellationToken,
    root_correlation: &domain::CorrelationId,
) -> Result<(BatchManagerSummary, std::sync::Arc<BoundedFileSink>), CliExit> {
    let sink = std::sync::Arc::new(build_batch_sink(opts).await?);
    let manager =
        prepare_batch_manager(opts, tls_emulation, sink.clone(), root_correlation).await?;

    // Resume filtered EVERYTHING: nothing to crawl, nothing to flush — a
    // clean no-op (exit 0), not "no content" 69. The export phase below still
    // runs over an empty result set and commits nothing new.
    if manager.url_count() == 0 && opts.crawl.resume {
        info!("resume: all batch URLs already processed — nothing to crawl");
        return Ok((BatchManagerSummary::default(), sink));
    }

    // WU3 (#2611): when `--export-checkpoint <N>` is set, run the flush watch
    // alongside the crawl so a killed run loses at most the pages captured
    // since the last checkpoint instead of the whole batch.
    let flush_done = tokio_util::sync::CancellationToken::new();
    let flush_task = spawn_flush_watch(opts, sink.clone(), flush_done.clone(), root_correlation);

    let summary = manager.process_all_summary_cancellable(cancel).await;
    log_batch_summary(&summary, root_correlation);

    // The crawl is over: stop the watch loop and let its FINAL flush (bounded
    // by FLUSH_SHUTDOWN_TIMEOUT) run before the sink closes.
    flush_done.cancel();
    if let Some(handle) = flush_task {
        let _ = tokio::time::timeout(
            crate::cli::batch_flush::FLUSH_SHUTDOWN_TIMEOUT
                .saturating_add(std::time::Duration::from_secs(1)),
            handle,
        )
        .await;
    }

    if cancel.is_cancelled() {
        warn!("shutdown requested — exporting the pages captured so far");
        // Stop the spool writer at the next page boundary so a shutdown never
        // waits on a spool that has stopped draining (#1616). `finish` still
        // flushes and joins it, so the pages already persisted are exported.
        sink.cancel();
    }

    flush_batch_sink(&sink).await?;

    Ok((summary, sink))
}

/// Spawn the checkpoint flush watch when `--export-checkpoint` is on, else
/// return `None`. The sidecar is written at startup (offset 0) with the
/// spool's identity, so the next run's acquisition can attribute this spool.
fn spawn_flush_watch(
    opts: &CrawlOptions,
    sink: std::sync::Arc<BoundedFileSink>,
    done: tokio_util::sync::CancellationToken,
    root_correlation: &domain::CorrelationId,
) -> Option<tokio::task::JoinHandle<()>> {
    use crate::application::batch::progress::{
        spool_first_line_sha256, ExtractionOptions, SpoolIdentity,
    };

    let checkpoint = opts.batch.export_checkpoint;
    if checkpoint == 0 {
        return None;
    }

    // The flush COMMITTEDs each exported page into the batch-namespace
    // store so the clean close (and a --resume relaunch) skips it (WU4).
    let store = match build_batch_resume_store(opts) {
        Ok(Some(s)) => Some(crate::cli::scrape_flow::record_store_bridge(&*s)),
        _ => None,
    };

    let root = resolve_persistence_root(opts);
    let output_dir = resolve_persistence_root(opts);
    let spool_path = root.join(BATCH_SPOOL_FILE);

    // Identity of the spool this run is writing: empty at startup (the sink
    // was just created/truncated), so `first_line_sha256` is None — the
    // sidecar written now records `None` and is only authoritative until the
    // first page lands. The flush loop refreshes the identity once the first
    // line exists (see run_flush_watch's persist_sidecar call after the first
    // successful flush).
    let identity = match std::fs::read(&spool_path) {
        Ok(bytes) => SpoolIdentity {
            first_line_sha256: spool_first_line_sha256(&bytes),
        },
        Err(_) => SpoolIdentity {
            first_line_sha256: None,
        },
    };

    // Persist the startup sidecar (offset 0) synchronously BEFORE the crawl so
    // a kill in the first seconds still leaves an attributable artifact.
    let extraction = ExtractionOptions {
        selector: Some(opts.crawl.selector.clone()),
        ignore_waf: opts.crawl.ignore_waf,
    };
    let startup_sidecar =
        crate::cli::batch_flush::running_sidecar(&identity, &output_dir, &extraction, 0);
    let startup_root = root.clone();
    let _ = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        if let Ok(rt) = rt {
            let _ = rt.block_on(crate::cli::batch_flush::write_sidecar(
                &startup_root,
                &startup_sidecar,
            ));
        }
    })
    .join();

    // Supervised: the handle is joined in run_batch_crawl under a timeout
    // right after the crawl finishes (the watch's own final flush).
    #[allow(clippy::disallowed_methods)]
    let handle = tokio::spawn(crate::cli::batch_flush::run_flush_watch(
        crate::cli::batch_flush::FlushWatchParams {
            sink,
            spool_identity: identity,
            root,
            output_dir,
            extraction,
            checkpoint,
            correlation: root_correlation.clone(),
            done,
            store,
        },
    ));
    Some(handle)
}

/// Flush the capture spool and fail loudly when the batch produced nothing.
///
/// An empty spool means `--batch` would write zero files while reporting
/// success — the regression #631 fixed.
async fn flush_batch_sink(sink: &BoundedFileSink) -> Result<(), CliExit> {
    let captured = sink.finish().await.map_err(|e| {
        error!(error = %e, "batch content spool flush failed");
        CliExit::IoError(format!("No se pudo volcar el contenido capturado: {e}"))
    })?;

    if captured == 0 {
        error!("Batch captured no page bodies — nothing to export");
        return Err(CliExit::NetworkError("Batch produced no content".into()));
    }

    // The spool could not keep up with the crawl AND the sink's memory ceiling
    // was reached, so some bodies were never spooled (#1616). The crawl itself
    // is sound and the export is still valid, but the run is incomplete and the
    // operator has to be able to tell that from a healthy batch.
    let dropped = sink.dropped();
    if dropped > 0 {
        warn!(
            captured_pages = captured,
            dropped_pages = dropped,
            "batch capture spool saturated — these page bodies were not exported"
        );
    }

    Ok(())
}

/// Build the resume state store for `--resume` (#637) so `export_phase`
/// can mark each already-crawled URL as processed. Returns `None` when
/// neither `--resume` nor `--export-checkpoint` is on.
///
/// # Namespace (design L3, WU4)
///
/// With `--export-checkpoint` active the store lives under
/// `batch-<hash8(output_dir)>` — a COMMITTED must mean "exported to THIS
/// output dir", so two batches targeting different output dirs never skip
/// against each other's records. Plain `--resume` (no checkpoint) keeps
/// the legacy host-derived domain so existing state files keep working
/// (#2609 verified against that layout).
///
/// The factory is lazy and infallible, so this always succeeds when either
/// flag is on; the `Result` keeps the `CliExit::IoError` route for a future
/// eager factory.
fn build_batch_resume_store(
    opts: &CrawlOptions,
) -> Result<Option<std::sync::Arc<dyn StateStorePort>>, CliExit> {
    let checkpoint = opts.batch.export_checkpoint;
    // WU4: the checkpoint flush COMMITTEDs into the batch-namespace store even
    // without --resume, so the clean close can skip what the flush already
    // exported (no double append). Plain --resume (no checkpoint) keeps the
    // legacy host-derived domain for existing state files (#2609).
    if !opts.crawl.resume && checkpoint == 0 {
        return Ok(None);
    }
    let state_dir = opts
        .crawl
        .state_dir
        .clone()
        .unwrap_or_else(crate::cli::scrape_flow::resolve_default_state_dir);
    let domain = if checkpoint > 0 {
        crate::cli::batch_flush::batch_namespace(&resolve_persistence_root(opts))
    } else {
        opts.url.host_str().unwrap_or("batch").to_string()
    };
    Ok(Some(crate::application::container::build_state_store(
        state_dir, &domain,
    )))
}

/// Run the elastic / output-vectors ingestion for the batch pipeline (#636,
/// #637) and release the ingestion handle afterwards.
///
/// #1949: the exit decision is NOT made here — every ingestion failure routes
/// through the canonical helper
/// [`ingestion_exit_for`](crate::cli::error::ingestion_exit_for), which maps
/// the error's class per matrix row 26 (transient backend failure → 69,
/// panicked/cancelled task → 3) instead of a hardcoded `IoError` (74).
async fn run_batch_elastic(
    ingestion: &Option<
        std::sync::Arc<
            crate::application::elastic_ingestion::ElasticIngestion<
                crate::domain::repository::DynVectorRepository,
            >,
        >,
    >,
    results: &[domain::ScrapedContent],
) -> Result<(), CliExit> {
    if let Some(ref ingestion) = ingestion {
        run_elastic_ingestion(ingestion, results)
            .await
            .map_err(|e| crate::cli::error::ingestion_exit_for(&e))?;
    }
    Ok(())
}

/// Convert the batch-captured pages into [`ScrapedContent`](crate::domain::ScrapedContent) and collect
/// per-page extraction failures.
///
/// Pages are streamed one at a time from the sink's spool (#653) — the raw
/// bodies are never all resident at once. Each body goes through the same
/// [`extract_content`](crate::application::extraction::extract_content) path as single-page mode: Readability → text fallback →
/// binary detection. Pages that fail extraction are logged and reported; the
/// `exit_code` decision is made afterwards by `report_phase`.
///
/// The batch crawl had one fetch per URL, so `CrawlTaskCtx` uses the default
/// asset downloader (`None`) — the same behavior as `--no-images` /
/// `--no-documents`.
///
/// # Errors
///
/// Returns [`CliExit`] when the capture spool cannot be opened or read.
/// Per-page failures (extraction errors and corrupt records) are collected
/// in the `failures` vec / skipped counter instead of aborting the whole
/// batch — the same page-vs-I/O split the recovery path uses (#2611).
async fn extract_batch_content(
    sink: &BoundedFileSink,
    opts: &CrawlOptions,
) -> Result<
    (
        Vec<domain::ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
    ),
    CliExit,
> {
    let output_dir = resolve_persistence_root(opts);
    let cfg = crate::application::batch::spool_export::SpoolExportConfig {
        scraper: ScraperConfig::default()
            .with_output_dir(output_dir)
            .with_selector(opts.crawl.selector.clone())
            .with_ignore_waf(opts.crawl.ignore_waf),
    };

    let mut reader = sink.reader().await.map_err(|e| {
        error!(error = %e, "batch capture spool could not be opened");
        CliExit::IoError(format!("No se pudo leer el contenido capturado: {e}"))
    })?;

    // Single shared conversion path (P6-2/F-16, #1290): the same helper
    // `recover` and the incremental flush feed — identical DTO by
    // construction. Conversion only: writing stays in `export_phase`.
    let outcome = crate::application::batch::spool_export::convert_spool(&mut reader, &cfg)
        .await
        .map_err(|e| {
            error!(error = %e, "batch capture spool could not be converted");
            CliExit::IoError(format!("No se pudo leer el contenido capturado: {e}"))
        })?;

    Ok((
        outcome.converted.into_iter().map(|c| c.content).collect(),
        outcome.failed,
    ))
}

#[cfg(test)]
#[path = "batch_flow_tests.rs"]
mod tests;
