//! CLI batch pipeline — the `--batch` lifecycle end to end.
//!
//! Extracted from `cli/orchestrator.rs` as part of the composition-root
//! decomposition (issue #1619, finding F1). Everything reachable only from
//! `run_batch` lives here: the per-URL crawl, the bounded capture spool,
//! content extraction, the elastic/resume wiring and the batch crawl-config
//! projection.
//!
//! The module owns no new policy. The exit-code decision stays in
//! [`crate::cli::exit_codes`], the canonical severity mapping in
//! [`crate::cli::error`], and the shared gates (`output_vectors_gate`,
//! `export_phase`, the persistence-root resolver) in
//! [`crate::cli::orchestrator`] — so both CLI flows keep one implementation of
//! each rule.

use tracing::{error, info, warn};

use crate::application::batch::{BatchManager, BatchManagerSummary};
use crate::application::crawl_options::{CrawlLimits, CrawlOptions};
use crate::application::crawler::BoundedFileSink;
use crate::cli::discovery_phase::base_crawler_config_builder;
use crate::cli::elastic::{build_elastic_ingestion, run_elastic_ingestion};
use crate::cli::error::CliExit;
use crate::cli::exit_codes::{batch_exit_code, report_phase};
use crate::cli::orchestrator::{output_vectors_gate, resolve_persistence_root};
use crate::domain;
use crate::domain::config::ScraperConfig;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::persistence::StateStorePort;
use crate::CrawlerConfig;

use crate::domain::semantic_cleaner::SemanticCleaner;

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

/// Warn when crawl-expansion flags are set on a `--batch` run (#1215).
///
/// Batch scrapes each input URL (one page per URL), so `--max-depth`,
/// `--max-pages`, and `--sitemap` cannot expand anything here. A value that
/// differs from the default means the operator asked for a crawl — staying
/// silent would let them assume one happened — hence `warn!`, naming every
/// inert flag. Defaults are read from [`CrawlLimits::default`] (the same
/// source the CLI defaults mirror) rather than hardcoded, so the comparison
/// cannot rot when defaults move.
fn warn_batch_crawl_flags_ignored(opts: &CrawlOptions) {
    let defaults = CrawlLimits::default();
    let mut inert = Vec::new();
    if opts.crawl.max_depth != defaults.max_depth {
        inert.push(format!("--max-depth {}", opts.crawl.max_depth));
    }
    if opts.crawl.max_pages != defaults.max_pages {
        inert.push(format!("--max-pages {}", opts.crawl.max_pages));
    }
    if opts.crawl.use_sitemap || opts.crawl.sitemap_url.is_some() {
        inert.push("--sitemap".to_string());
    }
    if inert.is_empty() {
        info!("Batch mode scrapes each input URL (one page per URL, no crawling)");
    } else {
        warn!(
            flags = inert.join(", "),
            "--batch scrapes each input URL (one page per URL): crawl-expansion flags are ignored (#1215)"
        );
    }
}

/// Load the batch manager, attach the capture sink, and assert it has work.
///
/// Warns loudly when crawl-expansion flags (`--max-depth`, `--max-pages`,
/// `--sitemap`) differ from their defaults: since #1215 batch scrapes each
/// URL instead of crawling it, those flags are inert here and the operator
/// must hear about it rather than assume a crawl happened.
///
/// `root_correlation` (#1439) travels onto the [`BatchProcessor`](crate::application::batch::BatchProcessor) so every
/// per-URL crawl engine of this run shares the one announced identity.
async fn prepare_batch_manager(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    sink: std::sync::Arc<BoundedFileSink>,
    root_correlation: &domain::CorrelationId,
) -> Result<BatchManager, CliExit> {
    warn_batch_crawl_flags_ignored(opts);
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let crawler_config = build_batch_crawler_config(opts, tls_emulation, &budget)?;
    let manager = load_batch_manager(opts, crawler_config, &budget)
        .await?
        .with_content_sink(sink.clone())
        .with_correlation(root_correlation.clone());

    // WU4 (#2609/#2611): with `--resume`, drop the URLs whose records are
    // COMMITTED-proven in the batch store BEFORE any fetch — a relaunch must
    // not re-download pages the previous run already exported (S7 iv: zero
    // double downloads). Runs on the blocking pool: the store read takes
    // the flock and does file I/O.
    let had_urls_before_resume_filter = manager.url_count() > 0;
    let manager = if opts.crawl.resume {
        filter_committed_batch(manager, opts, sink, root_correlation).await?
    } else {
        manager
    };

    if manager.url_count() == 0 {
        if had_urls_before_resume_filter {
            // Every input URL is already COMMITTED: the batch has nothing to
            // do, and that is a SUCCESS (exit 0), not a usage error — same
            // contract as the crawl-classic resume (#2609).
            info!("resume: all batch URLs already processed, nothing pending");
        } else {
            error!("No URLs provided for batch processing");
            return Err(CliExit::UsageError("No URLs provided".into()));
        }
    }

    info!(
        "Starting batch processing: {} URLs, concurrency={}",
        manager.url_count(),
        budget.batch().get()
    );

    Ok(manager)
}

/// Filter the batch URL list down to the not-yet-COMMITTED ones (WU4).
///
/// Uses the SAME typed gate as the crawl-classic resume
/// ([`crate::application::resume::filter_committed`]) against the batch
/// store, so a relaunched `--resume` batch skips exactly the pages a
/// previous run committed — including pages a checkpoint flush committed
/// mid-run. All-skipped is NOT an error here: the run exits through the
/// normal empty-batch path, which reports success with zero work.
async fn filter_committed_batch(
    manager: BatchManager,
    opts: &CrawlOptions,
    sink: std::sync::Arc<BoundedFileSink>,
    root_correlation: &domain::CorrelationId,
) -> Result<BatchManager, CliExit> {
    let store = match build_batch_resume_store(opts) {
        Ok(Some(s)) => crate::cli::scrape_flow::record_store_bridge(&*s),
        _ => return Ok(manager),
    };
    let urls = manager.urls();
    let parsed: Vec<url::Url> = urls
        .iter()
        .filter_map(|u| url::Url::parse(u).ok())
        .collect();
    let total = parsed.len();
    let manager_urls = manager.urls();
    let (pending, _run_id) = {
        let store = store.clone();
        let parsed = parsed.clone();
        tokio::task::spawn_blocking(move || {
            crate::application::resume::filter_committed(
                parsed,
                &store as &dyn crate::domain::persistence::RecordStorePort,
            )
        })
        .await
        .map_err(|e| CliExit::IoError(format!("No se pudo leer el estado de resume: {e}")))?
    };
    let pending_set: std::collections::HashSet<String> =
        pending.iter().map(|u| u.as_str().to_string()).collect();
    let kept: Vec<String> = manager_urls
        .into_iter()
        .filter(|u| {
            url::Url::parse(u)
                .map(|p| pending_set.contains(p.as_str()))
                .unwrap_or(true)
        })
        .collect();
    let skipped = total.saturating_sub(kept.len());
    if skipped > 0 {
        info!(
            skipped,
            total, "resume: skipping URLs already COMMITTED in the batch store"
        );
    }
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    // Rebuild the manager over the pending URLs with a fresh config — the
    // crawler_config was consumed by load_batch_manager, so rebuild it.
    let tls_emulation = resolve_batch_tls_emulation(opts)?;
    let crawler_config = build_batch_crawler_config(opts, tls_emulation, &budget)?;
    Ok(
        BatchManager::from_urls(kept, crawler_config, budget.batch().nonzero())
            .with_content_sink(sink)
            .with_correlation(root_correlation.clone()),
    )
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

/// File name of the batch capture spool, under the run's persistence root.
pub(crate) const BATCH_SPOOL_FILE: &str = ".webfang-batch-capture.jsonl";

/// Spanish disclosure for a spool quarantined as an orphan (slice 2, #2611).
///
/// The data is NOT deleted — it is renamed aside and the operator is pointed
/// at `webfang recover`, which can turn it back into `.md` files.
const ORPHAN_SPOOL_MESSAGE: &str = "Se encontró un archivo temporal de captura de una ejecución anterior que no se puede atribuir a esta corrida (sin sidecar utilizable o con identidad que no coincide). NO se eliminó: se movió a la ruta indicada. Recuperá las páginas con: webfang recover --spool <ruta> -o <dir>";

/// Observe the sidecar + spool pair at the given root (the pure decision's
/// inputs) without mutating anything.
async fn observe_spool_pair(
    root: &std::path::Path,
) -> (
    crate::application::batch::progress::SidecarState,
    crate::application::batch::progress::SpoolFileState,
) {
    use crate::application::batch::progress::{
        spool_first_line_sha256, BatchProgressV1, SidecarState, BATCH_PROGRESS_FILE,
    };

    let spool_path = root.join(BATCH_SPOOL_FILE);
    let sidecar_path = root.join(BATCH_PROGRESS_FILE);

    let sidecar = match tokio::fs::read(&sidecar_path).await {
        Err(_) => SidecarState::Absent,
        Ok(bytes) => match serde_json::from_slice::<BatchProgressV1>(&bytes) {
            Ok(parsed) => SidecarState::Parsed(Box::new(parsed)),
            Err(_) => SidecarState::Unreadable,
        },
    };

    let spool = match tokio::fs::read(&spool_path).await {
        Err(_) => crate::application::batch::progress::SpoolFileState {
            len_bytes: 0,
            first_line_sha256: None,
        },
        Ok(bytes) => crate::application::batch::progress::SpoolFileState {
            len_bytes: bytes.len() as u64,
            first_line_sha256: spool_first_line_sha256(&bytes),
        },
    };

    (sidecar, spool)
}

/// Quarantine a leftover spool whose data cannot be attributed (L5: a sidecar
/// that does not describe the spool must not choose its destination).
///
/// Renames the spool to a timestamped sibling — NEVER deletes — and discloses
/// the path, size and the recover remedy in Spanish. Returns where the orphan
/// went so the acquisition outcome can carry it.
async fn quarantine_orphan_spool(
    spool_path: &std::path::Path,
) -> Option<(std::path::PathBuf, u64)> {
    use crate::application::batch::progress::orphan_spool_path;

    let metadata = match tokio::fs::metadata(spool_path).await {
        Ok(m) => m,
        // Nothing on disk: no orphan to quarantine.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(
                error = %e,
                spool = %spool_path.display(),
                "orphan spool could not be inspected before quarantine"
            );
            return None;
        },
    };
    let orphan_bytes = metadata.len();
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let orphan_path = orphan_spool_path(spool_path, now_secs);

    match tokio::fs::rename(spool_path, &orphan_path).await {
        Ok(()) => {
            tracing::warn!(
                spool = %spool_path.display(),
                orphan = %orphan_path.display(),
                orphan_bytes,
                user_message = ORPHAN_SPOOL_MESSAGE,
                "quarantined an unattributable batch capture spool"
            );
            Some((orphan_path, orphan_bytes))
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                spool = %spool_path.display(),
                user_message = ORPHAN_SPOOL_MESSAGE,
                "an unattributable batch capture spool could not be moved aside; it stays in place"
            );
            None
        },
    }
}

/// Create the disk-backed capture sink for a batch run through the UNIFIED
/// acquisition path (slice 2, WU1): observe → decide → act on the SAME data
/// the pure table saw, THEN hand a fresh sink its spool path.
///
/// Order pinned by the design (L7): spool identity and the sidecar are read
/// BEFORE anything truncates; the decision is the pure [`decide_sweep`]; an
/// orphan is renamed aside (never deleted, never salvage-able by this run)
/// and the new run gets a fresh spool at the original path. Salvage itself
/// belongs to the flush seam (WU3) — acquisition only records what was
/// decided so the flush can act on it.
async fn build_batch_sink(opts: &CrawlOptions) -> Result<BoundedFileSink, CliExit> {
    use crate::application::batch::progress::decide_sweep;

    let root = resolve_persistence_root(opts);
    let spool_path = root.join(BATCH_SPOOL_FILE);

    let (sidecar_state, spool_state) = observe_spool_pair(&root).await;
    match decide_sweep(&sidecar_state, &spool_state) {
        crate::application::batch::progress::SweepDecision::OrphanSpool => {
            // Data without a usable description: quarantine and start fresh.
            let quarantined = quarantine_orphan_spool(&spool_path).await;
            if quarantined.is_none() && spool_state.len_bytes > 0 {
                // The rename failed and the residue still sits in the spool
                // path. This run must NOT truncate it (that would destroy
                // unattributable data): fail the run loudly instead.
                return Err(CliExit::IoError(format!(
                    "No se pudo apartar el spool huérfano en {}. Bórralo o recupéralo con \
                     webfang recover antes de relanzar el batch.",
                    spool_path.display()
                )));
            }
        },
        crate::application::batch::progress::SweepDecision::SalvageFromOffset { .. } => {
            // WU3 wires the flush-side salvage; acquisition records the
            // decision in the structured log so it is queryable now.
            tracing::info!(
                spool = %spool_path.display(),
                "salvageable spool detected — flush-side salvage pending WU3"
            );
        },
        crate::application::batch::progress::SweepDecision::Nothing => {},
    }

    // One buffered page per concurrent crawl, plus headroom, keeps the writer
    // from becoming the bottleneck without unbounding memory. The bound derives
    // from the budget model's Operation.batch tier (task 2.5c).
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let buffer = budget
        .batch()
        .get()
        .saturating_mul(2)
        .max(crate::application::crawler::bounded_sink::DEFAULT_SINK_BUFFER);
    BoundedFileSink::new(spool_path, buffer).await.map_err(|e| {
        error!(error = %e, "batch content spool could not be created");
        CliExit::IoError(format!(
            "No se pudo crear el archivo temporal de captura: {e}"
        ))
    })
}

/// Remove the batch capture spool once its pages have been extracted.
///
/// Best-effort: a leftover spool is noise, not a failure of the run. If THIS
/// removal fails, the residue is not invisible either — the next batch run's
/// acquisition path discloses or quarantines any survivor, which is the same
/// remedy the crash case gets.
async fn discard_batch_spool(sink: &BoundedFileSink) {
    if let Err(e) = tokio::fs::remove_file(sink.spool_path()).await {
        tracing::debug!(
            error = %e,
            spool = %sink.spool_path().display(),
            "batch capture spool could not be removed"
        );
    }
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

/// Print the batch completion summary and log each failed URL.
///
/// `root_correlation` is the batch run-root identity: each failed URL emits
/// the shared operational error contract (`log_scrape_error`) carrying it,
/// so trace-file queries join failures with the run (#1604).
fn log_batch_summary(summary: &BatchManagerSummary, root_correlation: &domain::CorrelationId) {
    println!(
        "Batch complete: {}/{} succeeded, {} failed",
        summary.succeeded, summary.total_urls, summary.failed
    );

    for (url, err) in &summary.errors {
        crate::infrastructure::observability::log_scrape_error(
            err,
            url,
            "batch",
            Some(root_correlation),
            "batch URL failed",
        );
    }
}

/// Resolve the TLS/H2 fingerprint for the batch crawl engine.
///
/// An unknown profile is a config error (exit 78), matching the scrape phase —
/// never silently crawl with a wrong fingerprint (#312).
fn resolve_batch_tls_emulation(opts: &CrawlOptions) -> Result<wreq_util::Profile, CliExit> {
    HttpClientConfig::profile_from_name(&opts.network.h2_profile)
        .map_err(|e| CliExit::ConfigError(e.to_string()))
}

/// Build the crawler config for the batch engine, honoring `--h2-profile`.
///
/// `--delay-ms` and the concurrency bound are propagated here (#653): without
/// them the batch engine crawled at full speed with its own default concurrency,
/// making both flags silent no-ops on the `--batch` path.
///
/// #1215: the `max_pages`/`max_depth` values carried here are the CLI crawl
/// budget, but they never expand batch seeds — the processor rebuilds a
/// seed-only config per URL (the batch processor pins depth 0 / 1 page),
/// so every seed is scraped exactly once and `--max-pages 1` can no longer
/// starve seeds.
///
/// The concurrency bound comes from the run's [`BudgetModel`](crate::domain::budget::BudgetModel) Operation.crawl
/// tier (task 2.5b).
fn build_batch_crawler_config(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    budget: &crate::domain::budget::BudgetModel,
) -> Result<CrawlerConfig, CliExit> {
    Ok(base_crawler_config_builder(opts)?
        // Concurrency bound derives from the run's budget model
        // Operation.crawl tier (task 2.5b), not from the raw CLI flag.
        .concurrency(budget.crawl().nonzero())
        .tls_emulation(tls_emulation)
        .build())
}

/// Load the batch manager from a file or stdin.
///
/// Also reached by the `--batch-file` dry-run preview (`run_dry_run`), which
/// lists the URLs a batch run would scrape without fetching them.
pub(crate) async fn load_batch_manager(
    opts: &CrawlOptions,
    crawler_config: CrawlerConfig,
    budget: &crate::domain::budget::BudgetModel,
) -> Result<BatchManager, CliExit> {
    if let Some(ref path) = opts.batch.batch_file {
        info!("Reading URLs from file: {}", path.display());
        // #2355: the budget tier is already `NonZeroUsize` — hand the typed
        // value through instead of downgrading to `usize` and revalidating.
        BatchManager::from_file(path, crawler_config, budget.batch().nonzero()).map_err(|e| {
            error!(error = %e, "Failed to read URLs from file");
            CliExit::IoError(format!("Failed to read URLs from file: {e}"))
        })
    } else {
        info!("Reading URLs from stdin");
        load_batch_manager_from_stdin(crawler_config, budget.batch().nonzero()).await
    }
}

/// Read batch URLs from stdin on a blocking thread.
async fn load_batch_manager_from_stdin(
    crawler_config: CrawlerConfig,
    concurrency: std::num::NonZeroUsize,
) -> Result<BatchManager, CliExit> {
    // spawn_blocking: stdin read is blocking I/O that must not run on the
    // Tokio async runtime thread pool — it would block other tasks.
    match tokio::task::spawn_blocking(move || BatchManager::from_stdin(crawler_config, concurrency))
        .await
    {
        Ok(result) => result.map_err(|e| {
            error!(error = %e, "Failed to read URLs from stdin");
            CliExit::IoError(format!("Failed to read URLs from stdin: {e}"))
        }),
        Err(join_err) => {
            error!(error = %join_err, "stdin read task panicked");
            Err(CliExit::IoError(format!("Failed to read URLs: {join_err}")))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_batch_crawler_config, build_batch_sink, observe_spool_pair, quarantine_orphan_spool,
        run_batch, BATCH_SPOOL_FILE,
    };
    use crate::application::crawl_options::CrawlOptions;
    use crate::cli::error::CliExit;
    use crate::CrawlerConfig;

    /// Build the batch config projection for `opts` the way preflight does:
    /// the explicit overrides ride through a freshly built [`BudgetModel`].
    fn projected_config(opts: &CrawlOptions) -> CrawlerConfig {
        let budget = crate::domain::budget::BudgetModel::build(
            opts.budget_overrides,
            &crate::domain::budget::detector::SystemDetector,
        );
        build_batch_crawler_config(opts, wreq_util::Profile::Chrome145, &budget)
            .expect("valid test projection must build")
    }

    #[test]
    fn batch_config_propagates_delay_and_model_concurrency() {
        // Regression for #653: per-URL rate limiting never engaged on the
        // batch path. The concurrency bound now derives from the run's
        // BudgetModel crawl tier; an explicit `--concurrency` value reaches
        // it THROUGH the model (explicit-wins override, design D4).
        let mut opts = CrawlOptions::default();
        opts.network.delay_ms = 750;
        // Explicit flag feeds the model override exactly as preflight does.
        opts.network.concurrency = crate::ConcurrencyConfig::new(2);
        if let Some(explicit) = opts.network.concurrency.get() {
            opts.budget_overrides.crawl =
                crate::domain::budget::tiers::CrawlConcurrency::new(explicit).ok();
        }

        let config = projected_config(&opts);

        assert_eq!(config.delay_ms, 750, "--delay-ms must reach the crawler");
        assert_eq!(
            config.concurrency.get(),
            2,
            "explicit --concurrency must reach the crawler through the model"
        );
    }

    #[test]
    fn batch_config_propagates_budget_overrides() {
        // Bug R2-1: --batch crawls each URL through the crawl Engine via
        // BatchProcessor.process_single_url -> crawl_site; the operator
        // overrides must ride on the base config handed to the batch job.
        let mut opts = CrawlOptions::default();
        opts.budget_overrides.crawl = crate::domain::budget::tiers::CrawlConcurrency::new(4).ok();
        opts.budget_overrides.rate_burst = crate::domain::budget::tiers::BurstPermits::new(13).ok();

        let config = projected_config(&opts);

        assert_eq!(
            config.budget_overrides.crawl.map(|c| c.get()),
            Some(4),
            "explicit --concurrency must reach the batch Engine"
        );
        assert_eq!(
            config.budget_overrides.rate_burst.map(|b| b.get()),
            Some(13),
            "explicit --rate-limit-burst must reach the batch Engine"
        );
    }

    #[test]
    fn batch_config_auto_concurrency_uses_model_tier() {
        // With no explicit flag, the model's auto-derived crawl tier is used.
        let opts = CrawlOptions::default();
        assert!(opts.network.concurrency.is_auto());
        let budget = crate::domain::budget::BudgetModel::for_test_preset();

        let config = build_batch_crawler_config(&opts, wreq_util::Profile::Chrome145, &budget)
            .expect("valid test projection must build");

        assert_eq!(
            config.concurrency.get(),
            budget.crawl().get(),
            "auto mode must use the model's derived Operation.crawl tier"
        );
    }

    #[test]
    fn batch_config_zero_delay_disables_throttling() {
        let mut opts = CrawlOptions::default();
        opts.network.delay_ms = 0;

        let config = build_batch_crawler_config(
            &opts,
            wreq_util::Profile::Chrome145,
            &crate::domain::budget::BudgetModel::for_test_preset(),
        )
        .expect("valid test projection must build");

        assert_eq!(config.delay_ms, 0);
    }

    #[tokio::test]
    async fn run_batch_returns_data_error_when_output_vectors_without_clean_ai() {
        let mut opts = CrawlOptions::default();
        opts.elastic.output_vectors = Some("vectors.jsonl".to_string());
        let cancel = tokio_util::sync::CancellationToken::new();

        let exit = run_batch(
            opts,
            None,
            crate::application::container::VaultAiPorts::default(),
            None,
            &cancel,
            &crate::domain::CorrelationId::new(),
        )
        .await;

        assert!(
            matches!(exit, CliExit::DataFormatError(_)),
            "expected CliExit::DataFormatError, got {exit:?}"
        );
    }

    /// Capture-subscriber harness, so the acquisition's DISCLOSURE can be read
    /// rather than reviewed (DF-L1, #1615; slice 2 orphan quarantine).
    #[derive(Clone)]
    struct SpoolCaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SpoolCaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture buffer lock is never poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run an async closure under an in-memory `tracing` subscriber and return
    /// what it logged.
    async fn capture_logs<T, F, Fut>(f: F) -> (String, T)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let subscriber = {
            let sink = std::sync::Arc::clone(&buf);
            tracing_subscriber::fmt()
                .with_writer(move || SpoolCaptureWriter(std::sync::Arc::clone(&sink)))
                .with_ansi(false)
                .finish()
        };
        // Thread-local default: the sweep runs on this thread, and installing a
        // global subscriber here would leak into every other test in the crate.
        let _guard = tracing::subscriber::set_default(subscriber);
        let value = f().await;
        let text = String::from_utf8(
            buf.lock()
                .expect("capture buffer lock is never poisoned")
                .clone(),
        )
        .expect("tracing writes UTF-8");
        (text, value)
    }

    // --- Slice 2 (WU1): the unified spool acquisition (observe → decide → act).

    use crate::application::batch::progress::{
        BatchProgressV1, ExtractionOptions, SidecarState, SpoolIdentity, BATCH_PROGRESS_FILE,
    };

    /// Seed a sidecar describing the CURRENT first line of the spool at `dir`.
    async fn seed_matching_sidecar(dir: &tempfile::TempDir, watermark: u64) {
        let spool_bytes = tokio::fs::read(dir.path().join(BATCH_SPOOL_FILE))
            .await
            .expect("read spool");
        let first_line = crate::application::batch::progress::spool_first_line_sha256(&spool_bytes);
        let sidecar = BatchProgressV1 {
            version: 1,
            spool: SpoolIdentity {
                first_line_sha256: first_line,
            },
            output_dir: dir.path().join("out"),
            extraction_options: ExtractionOptions {
                selector: Some("body".to_string()),
                ignore_waf: false,
            },
            watermark_bytes: watermark,
        };
        tokio::fs::write(
            dir.path().join(BATCH_PROGRESS_FILE),
            serde_json::to_vec(&sidecar).expect("serialize sidecar"),
        )
        .await
        .expect("write sidecar");
    }

    /// `observe_spool_pair` must feed the pure table EXACTLY what is on disk:
    /// a matching sidecar parses, a corrupt one is Unreadable, and the spool
    /// identity is the first-line hash.
    #[tokio::test]
    async fn observe_reads_sidecar_state_and_spool_identity() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        tokio::fs::write(dir.path().join(BATCH_SPOOL_FILE), b"{\"url\":\"a\"}\nrest")
            .await
            .expect("seed spool");

        // Absent sidecar.
        let (sidecar, spool) = observe_spool_pair(dir.path()).await;
        assert_eq!(sidecar, SidecarState::Absent);
        assert!(
            spool.first_line_sha256.is_some(),
            "a full line has identity"
        );
        assert_eq!(
            spool.len_bytes,
            b"{\"url\":\"a\"}\nrest".len() as u64,
            "the observed length is the real file length"
        );

        // Corrupt sidecar → Unreadable (NOT absent — the distinction is the
        // whole point of the table).
        tokio::fs::write(dir.path().join(BATCH_PROGRESS_FILE), b"{broken")
            .await
            .expect("seed bad sidecar");
        let (sidecar, _) = observe_spool_pair(dir.path()).await;
        assert_eq!(sidecar, SidecarState::Unreadable);

        // Valid sidecar → Parsed with its content.
        seed_matching_sidecar(&dir, 7).await;
        let (sidecar, _) = observe_spool_pair(dir.path()).await;
        assert!(
            matches!(sidecar, SidecarState::Parsed(_)),
            "got {sidecar:?}"
        );
    }

    /// An orphan spool is renamed aside — never deleted — and the disclosure
    /// names the path, the size and the recover remedy in Spanish.
    #[tokio::test]
    async fn an_unattributable_spool_is_quarantined_and_disclosed() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let spool = dir.path().join(BATCH_SPOOL_FILE);
        let body = "{\"url\":\"https://a.example\",\"body\":\"PAGE\"}\n".repeat(8);
        tokio::fs::write(&spool, &body)
            .await
            .expect("seed leftover");
        // Unreadable sidecar + non-empty spool = OrphanSpool by the table.
        tokio::fs::write(dir.path().join(BATCH_PROGRESS_FILE), b"{broken")
            .await
            .expect("seed bad sidecar");

        let (logs, orphan) = capture_logs(|| {
            let spool = spool.clone();
            async move { quarantine_orphan_spool(&spool).await }
        })
        .await;

        let (orphan_path, orphan_bytes) = orphan.expect("the orphan must be moved aside");
        assert!(!spool.exists(), "the original path is free for the new run");
        assert!(orphan_path.exists(), "the data survives under the new name");
        assert_eq!(
            orphan_bytes,
            body.len() as u64,
            "the disclosure must size the residue"
        );
        assert_eq!(
            orphan_path.parent(),
            spool.parent(),
            "the orphan stays in the same directory"
        );
        assert!(orphan_path.to_string_lossy().contains(".orphaned"));
        assert!(logs.contains("no se puede atribuir"), "{logs}");
        assert!(logs.contains("NO se eliminó"), "{logs}");
        assert!(logs.contains("webfang recover"), "{logs}");
    }

    /// A missing spool is not an orphan: the acquisition is a no-op.
    #[tokio::test]
    async fn a_missing_spool_quarantines_nothing() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let orphan = quarantine_orphan_spool(&dir.path().join(BATCH_SPOOL_FILE)).await;
        assert!(orphan.is_none(), "nothing on disk, nothing to quarantine");
    }

    /// The full acquisition on a CLEAN root (no spool, no sidecar) must let a
    /// fresh run start: the sink is built and the spool path is the original.
    #[tokio::test]
    async fn acquisition_on_a_clean_root_starts_a_fresh_sink() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut opts = CrawlOptions::default();
        opts.export.output_dir = dir.path().to_path_buf();

        let sink = build_batch_sink(&opts).await.expect("sink on clean root");
        assert_eq!(
            sink.spool_path(),
            dir.path().join(BATCH_SPOOL_FILE),
            "the spool lands at the canonical path"
        );
    }

    /// An orphan whose rename fails must ABORT the run rather than truncate:
    /// truncating the spool path would destroy unattributable data (L5).
    #[tokio::test]
    #[cfg(unix)]
    async fn an_unmovable_orphan_fails_the_run_instead_of_truncating() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().expect("temp dir");
        let spool = dir.path().join(BATCH_SPOOL_FILE);
        tokio::fs::write(&spool, b"unattributable data")
            .await
            .expect("seed");
        tokio::fs::write(dir.path().join(BATCH_PROGRESS_FILE), b"{broken")
            .await
            .expect("bad sidecar");
        // Read-only directory: the rename out must fail.
        let mut perms = std::fs::metadata(dir.path()).expect("dir").permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(dir.path(), perms).expect("chmod");

        let mut opts = CrawlOptions::default();
        opts.export.output_dir = dir.path().to_path_buf();
        let exit = build_batch_sink(&opts).await;

        // Restore before asserting so a failure never leaves a locked dir.
        let mut perms = std::fs::metadata(dir.path()).expect("dir").permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(dir.path(), perms).expect("restore");

        assert!(
            matches!(exit, Err(CliExit::IoError(_))),
            "an unmovable orphan must fail the run, got {exit:?}"
        );
        let kept = tokio::fs::read(&spool).await.expect("spool intact");
        assert_eq!(
            kept, b"unattributable data",
            "the unattributable data must survive byte for byte"
        );
    }
}
