//! Operator-facing batch reporting — the run summary and the strict-input
//! gate.
//!
//! Split from `batch_flow.rs` (#2795): the operator-readable reporting layer
//! (the summary line, the input accounting, the counting errors) is a seam
//! of its own, and keeping it here keeps `batch_flow.rs` under its
//! size-ratchet baseline. The exit-code decision stays in
//! [`crate::cli::exit_codes`], the canonical severity mapping in
//! [`crate::cli::error`].

use tracing::error;

use crate::application::batch::{BatchInputLines, BatchManager, BatchManagerSummary};
use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;

/// Log the aggregated batch summary. The first line carries the
/// succeeded/failed totals; when input was read from `--batch-file`/stdin
/// and anything did not become a URL, the SAME line carries the input
/// accounting (#2795). The cause travels WITH the summary, not only with
/// the per-URL event the processor emits: this is the event an operator
/// reads after the run, and before #2784 it was the one place a batch
/// failure was reported with no cause at all.
///
/// `root_correlation` is the batch run-root identity: each failed URL emits
/// the shared operational error contract carrying it, so trace-file queries
/// join failures with the run (#1604).
pub(crate) fn log_batch_summary(
    summary: &BatchManagerSummary,
    root_correlation: &crate::domain::CorrelationId,
) {
    let mut line = format!(
        "Batch complete: {}/{} succeeded, {} failed",
        summary.succeeded, summary.total_urls, summary.failed
    );

    // #2795: the operator arithmetic closes in ONE line. Whenever input was
    // read from `--batch-file`/stdin and anything did not become a URL, the
    // summary carries the input accounting — read = valid + rejected + blank
    // + comment — so a mostly-discarded list can no longer masquerade as a
    // clean run. Silent when every line became a URL: no noise on clean runs.
    if let Some(stats) = &summary.input_lines {
        if stats.lines_read != stats.valid {
            line.push_str(&format!(
                " — {} lines read: {} valid, {} rejected, {} blank, {} comment",
                stats.lines_read, stats.valid, stats.rejected, stats.blank, stats.comment
            ));
        }
    }
    println!("{line}");

    // #2823: the two failure populations are reported APART. A run whose
    // resolver was down used to print exactly like a site that blocked every
    // URL, and the operator had no way to tell them apart from the output.
    if summary.resolution_failures > 0 || summary.block_failures > 0 {
        println!(
            "Failures by cause: {} resolution, {} site refusals",
            summary.resolution_failures, summary.block_failures
        );
    }
    if summary.aborted_on_resolution {
        println!(
            "Batch stopped early: the resolver is not answering ({} resolution failures)",
            summary.resolution_failures
        );
    }

    for (url, err) in &summary.errors {
        crate::infrastructure::observability::log_scrape_error_with_cause(
            err,
            url,
            "batch",
            Some(root_correlation),
            "batch URL failed",
        );
    }
}

/// The operator-facing input accounting — the countable half of #2795's
/// reconciliation. One formatter for BOTH counting errors (no-URLs usage
/// error, `--strict-input` config error) so they name the same numbers the
/// final summary prints.
pub(crate) fn input_accounting(stats: &BatchInputLines) -> String {
    format!(
        "líneas leídas: {}, válidas: {}, rechazadas: {}, vacías: {}, comentarios: {}",
        stats.lines_read, stats.valid, stats.rejected, stats.blank, stats.comment
    )
}

/// `--strict-input` (#2795): a rejected input line is a configuration error
/// (exit 78) instead of a silent trim of the work. Blank and comment lines
/// are legal input and never trip the gate. The exit rides
/// [`CliExit::ConfigError`] — the same policy-violation class as the
/// `--offline` gate (#2166) and sysexits `EX_CONFIG` — while the non-strict
/// path keeps the matrix's InvalidUrl → 64 (row 15): this is a new
/// CLI-boundary policy exit, not a reclassification.
pub(crate) fn enforce_strict_input(
    manager: &BatchManager,
    opts: &CrawlOptions,
) -> Result<(), CliExit> {
    let Some(stats) = manager.input_lines() else {
        return Ok(());
    };
    if !opts.batch.strict_input || stats.rejected == 0 {
        return Ok(());
    }
    error!(
        rejected = stats.rejected,
        lines_read = stats.lines_read,
        "--strict-input policy violated by the batch input"
    );
    Err(CliExit::ConfigError(format!(
        "--strict-input activo: la entrada tiene líneas rechazadas ({}). \
         Corregí las líneas indicadas en los avisos o volvé a correr sin --strict-input.",
        input_accounting(stats)
    )))
}
