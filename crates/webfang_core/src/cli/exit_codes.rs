//! CLI exit-code policy — folds scrape/batch failure sets into a [`CliExit`].
//!
//! Extracted from `cli/orchestrator.rs` as part of the composition-root
//! decomposition (issue #1619, finding F1). This module owns *only* the
//! decision "given these results and failures, which exit do we leave with" —
//! it performs no I/O, no crawling and no scraping.
//!
//! The severity routing itself lives in [`crate::cli::error`] (#839), which
//! holds the canonical class → exit mapping. The functions here are the two
//! call-site adapters (`report_phase` for the single-URL flow, `batch_exit_code`
//! for the batch flow) plus the shared `format_failure` renderer, so both flows
//! keep an identical precedence chain.

use tracing::info;

use crate::cli::error::CliExit;
use crate::domain;

/// Build the user-facing failure line for a single URL.
///
/// At `verbosity` 0 only the top-level `Display` message is shown, which keeps
/// network errors (DNS, connect) to a single readable line. At `verbosity` 1+
/// the full root-cause chain is preserved via `Error::source()` (D4), appending
/// each cause as `← cause`.
pub(crate) fn format_failure(
    url: &str,
    error: &crate::error::ScraperError,
    verbosity: u8,
) -> String {
    let mut chain = error.to_string();
    if verbosity > 0 {
        let mut src = std::error::Error::source(error);
        while let Some(cause) = src {
            chain.push_str(&format!("  ← {cause}"));
            src = cause.source();
        }
    }
    format!("Failed to scrape {url}: {chain}")
}

/// Report failures and determine the exit code.
///
/// Returns `None` if all pages scraped successfully (caller proceeds to export).
/// At `verbosity` 0 only the top-level error message is shown; at 1+ the full
/// root-cause chain is appended (see [`format_failure`]).
///
/// Robots-blocked routing (#705): when NOTHING was scraped and NOTHING failed
/// but `blocked > 0`, every URL was refused by robots.txt — the run exits
/// `CliExit::Forbidden` (77) with a Spanish hint about `--ignore-robots`
/// instead of the misleading "no pages scraped" network error. Any real
/// failure or any scraped page keeps the historical routing below.
///
/// Severity routing (#537 + #706 + matrix rows 21/22) applies when every
/// URL failed:
///
/// 1. An internal-fatal failure wins → `CliExit::ScraperFailure` (3) — an
///    internal bug must not masquerade as a transient network outage.
/// 2. A permanent-kind [`crate::error::ScraperError::Io`] failure is next →
///    `CliExit::IoError` (74) — an unwritable output path is EX_IOERR.
/// 3. Any [`crate::error::ScraperError::ExtractionFailed`] is next →
///    `CliExit::DataFormatError` (65) — the pages were fetched but carried
///    no usable content (JS-only shells, poor fallback).
/// 4. Anything else keeps `CliExit::NetworkError` (69).
///
/// The partial-success case always reports `PartialSuccess` (69) regardless of
/// failure severity: some content was scraped, which is the dominant signal.
pub(crate) fn report_phase(
    results: &[domain::ScrapedContent],
    failures: &[(String, crate::error::ScraperError)],
    blocked: usize,
    verbosity: u8,
) -> Option<CliExit> {
    for (url, error) in failures {
        eprintln!("{}", format_failure(url, error, verbosity));
    }

    if !failures.is_empty() && !results.is_empty() {
        return Some(CliExit::PartialSuccess {
            success: results.len(),
            failed: failures.len(),
        });
    }

    // Canonical robots-blocked override (#705): fires only when NOTHING was
    // scraped and NOTHING failed, delegating to the single implementation in
    // `cli::error` (#839).
    if let Some(exit) =
        crate::cli::error::forbidden_exit_when_all_blocked(results.len(), failures.len(), blocked)
    {
        return Some(exit);
    }

    if results.is_empty() {
        // Exit-code precedence in the all-fail arm: internal-fatal (3)
        // outranks the permanent-Io override (74), which outranks
        // extraction-failed (65), which outranks the transient fallback
        // (69) (#706 + matrix rows 21/22). An internal bug must never
        // masquerade as a data-format error. The Io override sits here —
        // AFTER the InternalFatal sweep, BEFORE extraction-failed — because
        // since `ScraperError::classify` splits by kind, a permanent io
        // error is PermanentFatal (never caught by the 3 sweep), and a run
        // that failed entirely on an unwritable output path must report 74,
        // not fall through to 65/69.
        // Exit-code precedence in the all-fail arm: internal-fatal (3)
        // outranks the permanent-Io override (74), which outranks
        // extraction-failed (65), which outranks the transient fallback
        // (69) (#706 + matrix rows 21/22). An internal bug must never
        // masquerade as a data-format error. The Io override sits here —
        // AFTER the InternalFatal sweep, BEFORE extraction-failed — because
        // since `ScraperError::classify` splits by kind, a permanent io
        // error is PermanentFatal (never caught by the 3 sweep), and a run
        // that failed entirely on an unwritable output path must report 74,
        // not fall through to 65/69. Each arm delegates to its canonical
        // mapping function in `cli::error` (#839) so every exit-code
        // decision has exactly one implementation.
        if let Some(exit) = crate::cli::error::scraper_failure_exit_when_internal_fatal(failures) {
            return Some(exit);
        }
        if let Some(exit) = permanent_io_error_for_failures(failures) {
            return Some(exit);
        }
        if let Some(exit) =
            crate::cli::error::data_format_error_exit_when_extraction_failed(failures)
        {
            return Some(exit);
        }
        eprintln!("No pages were successfully scraped");
        return Some(CliExit::NetworkError(
            "No pages were successfully scraped".into(),
        ));
    }

    info!("Successfully scraped {} pages", results.len());
    None
}

/// Fold an all-failed error set into a severity-aware exit (#537).
///
/// Severity routing lives entirely in the canonical mapping functions of
/// [`crate::cli::error`] (#839): internal-fatal → `ScraperFailure` (3) via
/// `scraper_failure_exit_when_internal_fatal`, permanent-Io → `IoError`
/// (74) via the per-item `permanent_io_error_exit_for`, extraction-failed →
/// `DataFormatError` (65) via `data_format_error_exit_when_extraction_failed`.
/// This local adapter only dispatches the variant through the canonical
/// per-item helper, keeping one implementation of the 74 decision.
fn permanent_io_error_for_failures(
    failures: &[(String, crate::error::ScraperError)],
) -> Option<CliExit> {
    failures.iter().find_map(|(_, e)| match e {
        crate::error::ScraperError::Io(io_err) => {
            crate::cli::error::permanent_io_error_exit_for(io_err)
        },
        _ => None,
    })
}

/// Determine the CLI exit code from batch scrape results.
///
/// Severity routing (#537 + #706 + matrix rows 21/22) for all-fail runs
/// (`failed > 0 && succeeded == 0`):
///
/// 1. An internal-fatal failure wins → `CliExit::ScraperFailure` (3).
/// 2. A permanent-kind I/O failure is next → `CliExit::IoError` (74).
/// 3. Any [`crate::error::ScraperError::ExtractionFailed`] is next →
///    `CliExit::DataFormatError` (65).
/// 4. Anything else keeps `CliExit::NetworkError` (69).
///
/// Partial success remains `CliExit::PartialSuccess` (69) regardless of
/// failure severity — some content was scraped, which is the dominant signal.
pub(crate) fn batch_exit_code(
    succeeded: usize,
    failed: usize,
    errors: &[(String, crate::error::ScraperError)],
) -> CliExit {
    if failed > 0 && succeeded == 0 {
        // Same precedence as `report_phase` (#706): internal-fatal (3)
        // first, then the permanent-Io override (74), then
        // extraction-failed (65), then the transient fallback (69).
        // The 74 override must precede extraction-failed so an all-fail run
        // caused by an unwritable output path reports 74 truthfully. Each
        // arm delegates to its canonical mapping function in `cli::error`
        // (#839) so every exit-code decision has exactly one implementation.
        if let Some(exit) = crate::cli::error::scraper_failure_exit_when_internal_fatal(errors) {
            return exit;
        }
        if let Some(exit) = permanent_io_error_for_failures(errors) {
            return exit;
        }
        if let Some(exit) = crate::cli::error::data_format_error_exit_when_extraction_failed(errors)
        {
            return exit;
        }
        CliExit::NetworkError("All batch URLs failed".into())
    } else if failed > 0 {
        CliExit::PartialSuccess {
            success: succeeded,
            failed,
        }
    } else {
        CliExit::Success
    }
}

#[cfg(test)]
#[path = "exit_codes_tests.rs"]
mod tests;
