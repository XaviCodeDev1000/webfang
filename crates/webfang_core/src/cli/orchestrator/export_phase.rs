//! Export phase of the crawl pipeline: run the exporters, then decide the
//! typed exit (including the resume short-circuit #705 and the truncated
//! discovery exit #2338).

use super::persistence::{resolve_export_dir, resolve_persistence_root};
use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;
use crate::cli::export_flow::{run_export, save_files, ExportConfig};
use crate::domain::result::ResumeStats;

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
///
/// #3097 / T4: this function no longer prints the “nothing pending” sentence.
/// The unified [`emit_resume_notice`] owns the user-facing resume text, and it
/// runs BEFORE this short-circuit, so an all-committed resume still explains
/// itself; a second message here would be a second source of truth.
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
        return Some(CliExit::Success);
    }
    None
}

/// The resume/discovery notice a run owes the operator (#3097 / T4).
///
/// One notice per run, on STDERR, `--quiet`-gated: a fresh run and a resumed
/// run must be distinguishable from the output alone, and a resumed run must
/// say what it read and what it decided to skip. The checkpoint counts ride
/// along only when the crawl actually consulted a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResumeNotice<'a> {
    /// The state file the run read, or `None` on a fresh run.
    pub(super) state_path: Option<&'a std::path::Path>,
    /// URLs discovery produced on THIS run (discovery re-runs on resume).
    pub(super) discovered: usize,
    /// Of those, the ones a previous run already committed.
    pub(super) already_committed: usize,
    /// Of those, the ones this run will drive.
    pub(super) pending: usize,
    /// The crawl-side checkpoint decision, when a checkpoint was consulted.
    pub(super) checkpoint: Option<ResumeStats>,
}

/// Render the notice, one line per fact.
///
/// Pure and separate from the printing so the contract text is unit-testable:
/// these strings are read by operators and asserted by
/// `tests/resume_message_test.rs`, which makes them a surface, not a log line.
pub(super) fn resume_notice_lines(notice: &ResumeNotice<'_>) -> Vec<String> {
    let Some(state_path) = notice.state_path else {
        return vec!["webfang: fresh run - no previous state consulted".to_string()];
    };
    let mut lines = vec![
        format!("webfang: resumed from {}", state_path.display()),
        format!(
            "webfang: breakdown: {} discovered, {} already committed, {} pending",
            notice.discovered, notice.already_committed, notice.pending
        ),
    ];
    if let Some(cp) = notice.checkpoint {
        lines.push(format!(
            "webfang: checkpoint: {} settled, {} re-driving, {} queued",
            cp.settled, cp.re_driving, cp.queued
        ));
    }
    lines.push(
        "webfang: discovery runs again on every resume and does not detect content changes"
            .to_string(),
    );
    lines
}

/// The lines a run actually emits: the whole notice, or nothing under
/// `--quiet`.
///
/// The gate is separated from the printing so it is testable without capturing
/// stderr, and so `--quiet` silences EVERY line instead of the first one.
pub(super) fn printed_notice_lines(quiet: bool, notice: &ResumeNotice<'_>) -> Vec<String> {
    if quiet {
        return Vec::new();
    }
    resume_notice_lines(notice)
}

/// Print the notice to STDERR, one `eprintln!` per line, unless `--quiet`.
pub(super) fn emit_resume_notice(quiet: bool, notice: &ResumeNotice<'_>) {
    for line in printed_notice_lines(quiet, notice) {
        eprintln!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A fresh run says exactly one thing, and says nothing about resuming.
    #[test]
    fn fresh_notice_is_one_line_and_claims_nothing_else() {
        let lines = resume_notice_lines(&ResumeNotice {
            state_path: None,
            discovered: 7,
            already_committed: 0,
            pending: 7,
            checkpoint: None,
        });
        assert_eq!(
            lines,
            vec!["webfang: fresh run - no previous state consulted".to_string()]
        );
    }

    /// A resumed run names the state file, the breakdown and the staleness
    /// note, in that order — the literal contract text operators read.
    #[test]
    fn resumed_notice_names_the_state_file_breakdown_and_staleness() {
        let state = Path::new("state/site.test.json");
        let lines = resume_notice_lines(&ResumeNotice {
            state_path: Some(state),
            discovered: 7,
            already_committed: 7,
            pending: 0,
            checkpoint: None,
        });
        assert_eq!(
            lines,
            vec![
                "webfang: resumed from state/site.test.json".to_string(),
                "webfang: breakdown: 7 discovered, 7 already committed, 0 pending".to_string(),
                "webfang: discovery runs again on every resume and does not detect content changes"
                    .to_string(),
            ]
        );
    }

    /// The checkpoint line exists only when the crawl consulted one, and it
    /// carries the numbers the scheduler acted on.
    #[test]
    fn checkpoint_line_appears_only_with_a_consulted_checkpoint() {
        let state = Path::new("state/site.test.json");
        let lines = resume_notice_lines(&ResumeNotice {
            state_path: Some(state),
            discovered: 7,
            already_committed: 3,
            pending: 4,
            checkpoint: Some(ResumeStats {
                skipped: 3,
                settled: 2,
                re_driving: 4,
                queued: 1,
            }),
        });
        assert_eq!(
            lines[2],
            "webfang: checkpoint: 2 settled, 4 re-driving, 1 queued"
        );
        assert_eq!(
            lines.len(),
            4,
            "one line per fact, no duplicates: {lines:?}"
        );
    }

    /// `--quiet` silences the WHOLE notice, not just its first line.
    #[test]
    fn quiet_suppresses_every_line() {
        let notice = ResumeNotice {
            state_path: Some(Path::new("state/site.test.json")),
            discovered: 1,
            already_committed: 0,
            pending: 1,
            checkpoint: None,
        };
        assert!(
            !printed_notice_lines(false, &notice).is_empty(),
            "the notice must exist to be suppressed"
        );
        assert_eq!(printed_notice_lines(true, &notice), Vec::<String>::new());
        assert_eq!(
            printed_notice_lines(false, &notice),
            resume_notice_lines(&notice),
            "the gate changes the COUNT of lines, never their text"
        );
    }
}
