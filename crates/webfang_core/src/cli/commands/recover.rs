//! `webfang recover` — convert a stranded batch capture spool into `.md` files.
//!
//! A batch killed before its clean close leaves captured pages trapped in the
//! raw JSONL spool (#2611): the spool is written incrementally but the
//! markdown conversion and file writes only happen at the clean close. This
//! command is the recovery tool: it streams the spool through the SAME
//! conversion path the batch export uses (same extractor, same frontmatter,
//! same output filenames) and writes one `.md` per page, atomically.
//!
//! Failure policy (page-vs-I/O, #2611): a corrupt record or a failed
//! extraction is a PAGE failure — counted, reported, never fatal. A read or
//! write I/O failure is FATAL — pages converted before it are still written,
//! the operator is told the spool remains on disk, and the exit is `74` so
//! scripts can retry later.

use std::path::Path;

use tracing::{info, instrument, warn};

use crate::application::batch::spool_export::{
    convert_spool, save_converted, SpoolExportConfig, SpoolExportError,
};
use crate::application::crawler::bounded_sink::CapturedPageReader;
use crate::cli::error::CliExit;
use crate::domain::config::ScraperConfig;
use crate::domain::CorrelationId;
use crate::infrastructure::observability::error_logging::log_scrape_error;
use crate::infrastructure::output::file_saver::ObsidianOptions;
use crate::OutputFormat;

/// Run the recovery of one spool file into one output directory.
///
/// # Errors
///
/// Never returns `Err` — the exit code IS the contract:
/// - `0` — at least one page recovered (or the spool was empty);
/// - `3` — the spool had records but NONE could be recovered;
/// - `69` — `--strict` and any page failed or was corrupt;
/// - `74` — the spool could not be read or the output could not be written
///   (pages already converted are still written first, and the spool stays
///   on disk for a later attempt).
#[instrument(skip_all, fields(spool = %spool.display(), output = %output.display()))]
pub async fn run(spool: &Path, output: &Path, strict: bool) -> CliExit {
    let correlation = CorrelationId::new();
    let started = std::time::Instant::now();

    let cfg = SpoolExportConfig {
        output_dir: output.to_path_buf(),
        // Slice 1: options come from defaults only — the original run's
        // options are not recorded in the spool, and the sidecar that will
        // carry them (slice 2) does not exist yet. Divergence is documented
        // in `--help`.
        scraper: ScraperConfig::default().with_output_dir(output.to_path_buf()),
    };

    let mut reader = match CapturedPageReader::open(spool).await {
        Ok(reader) => reader,
        Err(e) => {
            warn!(error = %e, "recover could not open the spool");
            return CliExit::IoError(format!(
                "No se pudo abrir el spool {}: {e}",
                spool.display()
            ));
        },
    };

    let outcome = match convert_spool(&mut reader, &cfg).await {
        Ok(outcome) => outcome,
        Err(SpoolExportError::SpoolRead { source, partial }) => {
            // Keep the work already converted, then fail honestly.
            let saved = save_converted(
                &partial.converted,
                output,
                &OutputFormat::Markdown,
                &ObsidianOptions::default(),
            )
            .await;
            let saved_note = match saved {
                Ok(()) => format!(
                    " Se escribieron {} páginas convertidas antes del fallo.",
                    partial.converted.len()
                ),
                Err(save_err) => format!(
                    " Además, no se pudieron escribir las {} páginas ya convertidas: {save_err}",
                    partial.converted.len()
                ),
            };
            warn!(error = %source, converted = partial.converted.len(), "recover spool read failed");
            return CliExit::IoError(format!(
                "No se pudo leer el spool {}: {source}.{saved_note} \
                 El spool queda en disco; reintentá más tarde con webfang recover.",
                spool.display()
            ));
        },
        // Unreachable from convert_spool (only save_converted produces it),
        // but the match must stay total and honest.
        Err(e) => {
            return CliExit::IoError(format!("No se pudo convertir el spool: {e}"));
        },
    };

    if let Err(e) = save_converted(
        &outcome.converted,
        output,
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .await
    {
        warn!(error = %e, converted = outcome.converted.len(), "recover output write failed");
        return CliExit::IoError(format!(
            "No se pudieron escribir los archivos de salida en {}: {e}. \
             El spool queda en disco; reintentá más tarde con webfang recover.",
            output.display()
        ));
    }

    // Page failures travel the shared operational error contract so trace
    // queries join them with the run (#1604).
    for (url, e) in &outcome.failed {
        log_scrape_error(
            e,
            url,
            "recover",
            Some(&correlation),
            "spool page extraction failed",
        );
    }

    let duration_ms = u128::from(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    info!(
        converted = outcome.converted.len(),
        failed = outcome.failed.len(),
        skipped_corrupt = outcome.skipped_corrupt,
        duration_ms,
        "recover summary"
    );

    print_summary(spool, output, &outcome);

    if outcome.converted.is_empty() {
        let saw_input = !outcome.failed.is_empty() || outcome.skipped_corrupt > 0;
        if saw_input {
            return CliExit::ScraperFailure(format!(
                "Ninguna página pudo recuperarse del spool {}: {} fallidas, {} corruptas",
                spool.display(),
                outcome.failed.len(),
                outcome.skipped_corrupt
            ));
        }
        println!("  El spool está vacío — no hay páginas para recuperar.");
        return CliExit::Success;
    }

    if strict && (!outcome.failed.is_empty() || outcome.skipped_corrupt > 0) {
        return CliExit::PartialSuccess {
            success: outcome.converted.len(),
            failed: outcome.failed.len() + usize::try_from(outcome.skipped_corrupt).unwrap_or(0),
        };
    }

    CliExit::Success
}

/// Operator-facing summary (Spanish): what was recovered, where, and what
/// did not make it.
fn print_summary(
    spool: &Path,
    output: &Path,
    outcome: &crate::application::batch::spool_export::SpoolExportOutcome,
) {
    println!("Recuperación de spool completada");
    println!("  Spool: {}", spool.display());
    println!("  Destino: {}", output.display());
    println!("  Convertidas: {}", outcome.converted.len());
    println!("  Fallidas: {}", outcome.failed.len());
    println!("  Corruptas: {}", outcome.skipped_corrupt);
}
