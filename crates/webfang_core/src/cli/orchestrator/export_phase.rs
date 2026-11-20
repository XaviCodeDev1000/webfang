//! Export phase of the crawl pipeline: run the exporters, then decide the
//! typed exit (including the resume short-circuit #705 and the truncated
//! discovery exit #2338).

use super::persistence::{resolve_export_dir, resolve_persistence_root};
use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;
use crate::cli::export_flow::{run_export, save_files, ExportConfig};

use crate::domain;
use crate::domain::persistence::StateStorePort;
use crate::domain::semantic_cleaner::SemanticCleaner;
use crate::infrastructure::output::file_saver::ObsidianOptions;
use tracing::{error, info};

/// Export scraped results to files and run AI cleaning if requested.
///
/// `results` is shared ownership (`Arc<[ScrapedContent]>`): the export phase
/// hands the slice to the blocking pool (issue #1814, slice B AC4), which
/// needs 'static data, while the caller keeps its own handle for the report
/// phase. See [`crate::cli::export_flow::ExportConfig`].
///
/// `cancel` is the run's cooperative cancellation token (from the
/// `ShutdownGuard` in `run()`, or the batch flow's own token): it is CLONED
/// into the [`ExportConfig`] so the blocking-pool export can observe it
/// through `ResumeContext::observing_cancel` (#2244 decision A).
pub(crate) async fn export_phase(
    results: std::sync::Arc<[domain::ScrapedContent]>,
    opts: &CrawlOptions,
    state_store: Option<&dyn StateStorePort>,
    ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    cancel: &tokio_util::sync::CancellationToken,
) -> CliExit {
    export_phase_with_resume(
        results,
        opts,
        state_store,
        ai_cleaner,
        cancel,
        opts.crawl.resume,
    )
    .await
}

/// [`export_phase`] with an explicit resume-gate flag. The batch path uses
/// this to turn the gate ON when `--export-checkpoint` flushed pages during
/// the run, so the clean close skips the already-COMMITTED ones instead of
/// double-appending them (#2611 WU4).
pub(crate) async fn export_phase_with_resume(
    results: std::sync::Arc<[domain::ScrapedContent]>,
    opts: &CrawlOptions,
    state_store: Option<&dyn StateStorePort>,
    ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    cancel: &tokio_util::sync::CancellationToken,
    resume_gate: bool,
) -> CliExit {
    if opts.export.output_dir == std::path::Path::new("-") {
        return CliExit::UsageError(
            "\"-o -\" no está soportado para exportación multi-archivo. \
             Usa \"--output-vectors -\" para exportar vectores a stdout, \
             o especifica un directorio de salida."
                .to_string(),
        );
    }

    let output_dir = resolve_export_dir(opts);

    let obsidian_options = ObsidianOptions {
        wiki_links: opts.export.obsidian_wiki_links,
        relative_assets: opts.export.obsidian_relative_assets,
        tags: opts.export.obsidian_tags.clone(),
        rich_metadata: opts.export.obsidian_rich_metadata,
        quick_save: opts.export.quick_save,
        vault_path: opts.export.obsidian_vault.clone(),
    };

    let file_output_dir = if opts.export.quick_save {
        let inbox = resolve_persistence_root(opts).join("_inbox");
        // #1107: async creation on `tokio::fs` (no blocking syscall on the
        // executor) and the Result is propagated, not discarded — a real
        // failure (ENOSPC, read-only vault) now stops the export with a
        // typed CliExit instead of surfacing late and degraded inside
        // `save_files`. `create_dir_all` is idempotent, so the old
        // `exists()` pre-check is gone.
        if let Err(e) = tokio::fs::create_dir_all(&inbox).await {
            return CliExit::ConfigError(format!(
                "no se pudo crear el inbox '{}': {e}",
                inbox.display()
            ));
        }
        inbox
    } else {
        output_dir.clone()
    };

    save_files(
        &results,
        &file_output_dir,
        &opts.export.output_format,
        &obsidian_options,
    );

    let export_config = ExportConfig {
        results,
        output_dir,
        format: opts.export.output_format,
        export_format: opts.export.export_format,
        clean_ai: opts.ai,
        quick_save: opts.export.quick_save,
        vault_path: opts.export.obsidian_vault.as_ref(),
        obsidian_options,
        state_store,
        resume: resume_gate,
        cancel: Some(cancel.clone()),
        ai_threshold: opts.ai_config.threshold,
        ai_max_chars: opts.ai_config.max_chars,
        ai_offline: opts.ai_config.offline,
    };

    let export_result = run_export(export_config, ai_cleaner).await;

    match export_result {
        Ok(processed_urls) => {
            info!("Export completed for {} URLs", processed_urls.len());
            CliExit::Success
        },
        Err(e) => {
            error!(error = ?e, "Export failed");
            e
        },
    }
}

/// The typed exit for a discovery crawl that ended early (#2338): the
/// partial output may already be written (after the export) or not (the
/// nothing-pending short-circuit), but the run always reports FAILED.
pub(super) fn truncated_discovery_exit(detail: &str, reason: &str) -> CliExit {
    CliExit::NetworkError(format!(
        "el rastreo terminó anticipadamente; {detail}: {reason}"
    ))
}

/// #705 Paso 2: a `--resume` run where every discovered URL was already
/// processed by a prior run is a technical success ("nothing pending"), not a
/// network failure. Return `Some(CliExit::Success)` before the scrape phase so
/// an empty filtered list never reaches `report_phase`'s false exit-69 path.
/// The `discovered_count > 0` guard keeps a genuinely empty discovery on its
/// existing route instead of masking it as resume success.
pub(super) fn resume_nothing_pending(
    opts: &CrawlOptions,
    urls_to_scrape: &[url::Url],
    discovered_count: usize,
    root_correlation: &domain::CorrelationId,
) -> Option<CliExit> {
    if opts.crawl.resume && urls_to_scrape.is_empty() && discovered_count > 0 {
        info!(
        skipped = discovered_count,
        trace_id = %root_correlation.trace_id(),
        "resume: all discovered URLs already processed, nothing pending"
        );
        if !opts.export.quiet {
            println!(
"Resume: nada pendiente — {discovered_count} URL(s) ya procesadas en ejecuciones anteriores."
);
        }
        return Some(CliExit::Success);
    }
    None
}
