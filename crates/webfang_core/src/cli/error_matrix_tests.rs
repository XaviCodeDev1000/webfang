use super::*;
use crate::error::ErrorClass;
use std::process::Termination;

// ====================================================================
// Error Classification Matrix — Sprint 1-2 P0-err final DoD item.
// Contract: docs/error-classification-matrix.md (ID 261bdb66).
// ====================================================================

/// The matrix document and these helpers are one contract written twice, and
/// nothing used to check they still agreed — which is exactly how F-10 could be
/// reported as "taxonomy still partial" with no way to say what was missing
/// (#1294). This test reads the document: every class must have a row, a row
/// naming a number must match the helper's number, and a class the helper
/// refuses to pin must be documented as variant- or outcome-dependent.
#[test]
fn matrix_doc_agrees_with_the_class_exit_helpers() {
    const DOC: &str = include_str!("../../../../docs/error-classification-matrix.md");

    /// Two-column rows under the "Default exit codes by class" heading.
    fn default_exit_rows() -> Vec<(String, String)> {
        let mut rows = Vec::new();
        let mut inside = false;
        for line in DOC.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                inside = rest.starts_with("Default exit codes by class");
                continue;
            }
            if !inside || !line.starts_with('|') {
                continue;
            }
            let cells: Vec<String> = line
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .map(str::to_owned)
                .collect();
            if cells.len() < 2 || cells[0].starts_with("---") || cells[0].contains("Class") {
                continue;
            }
            rows.push((cells[0].clone(), cells[1].clone()));
        }
        assert!(!rows.is_empty(), "the document exposes no exit-code rows");
        rows
    }

    let rows = default_exit_rows();
    for class in [
        ErrorClass::TransientRetriable,
        ErrorClass::TransientBackoff,
        ErrorClass::PermanentFatal,
        ErrorClass::InternalFatal,
        ErrorClass::DomainRecoverable,
    ] {
        let needle = format!("`{class:?}`");
        let right = rows
            .iter()
            .find(|(left, _)| left.contains(&needle))
            .map(|(_, right)| right.clone())
            .unwrap_or_else(|| {
                panic!("the matrix has no row naming {needle}; the helper is undocumented")
            });
        match default_exit_code_for_class(class) {
            Some(code) => {
                let documented: Vec<&str> = right
                    .split(|c: char| !c.is_ascii_digit())
                    .filter(|part| !part.is_empty())
                    .collect();
                assert!(
                    documented.contains(&code.to_string().as_str()),
                    "{class:?}: the document says {right:?} but the helper returns {code}"
                );
            },
            None => assert!(
                right.contains("varies") || right.contains(';'),
                "{class:?}: the helper returns None on purpose, so the row must say \
                     the exit depends on the variant or the outcome, got {right:?}"
            ),
        }
    }
}

// DoD: each ErrorClass maps to its documented default exit code.
#[test]
fn default_exit_code_transient_retriable_is_69() {
    assert_eq!(
        default_exit_code_for_class(ErrorClass::TransientRetriable),
        Some(EXIT_UNAVAILABLE)
    );
}

#[test]
fn default_exit_code_transient_backoff_is_69() {
    assert_eq!(
        default_exit_code_for_class(ErrorClass::TransientBackoff),
        Some(EXIT_UNAVAILABLE)
    );
}

#[test]
fn default_exit_code_internal_fatal_is_scraper_failure_never_usage() {
    // InternalFatal must map to ScraperFailure (3), NEVER UsageError 64:
    // an internal bug is not user error.
    assert_eq!(
        default_exit_code_for_class(ErrorClass::InternalFatal),
        Some(EXIT_SCRAPER_FAILURE)
    );
}

#[test]
fn permanent_fatal_has_no_single_default_exit() {
    // Variant-dependent per contract rows (64/65/69/76/77/78): forcing a
    // single code here would be wrong, so the mapping honestly returns None.
    assert_eq!(
        default_exit_code_for_class(ErrorClass::PermanentFatal),
        None
    );
}

#[test]
fn domain_recoverable_has_no_single_default_exit() {
    // Contract: 0 if any item succeeded; 65 if ALL items failed — decided
    // by run outcome, not by the class alone.
    assert_eq!(
        default_exit_code_for_class(ErrorClass::DomainRecoverable),
        None
    );
}

#[test]
fn cli_exit_for_class_transient_maps_to_network_error_variant() {
    let exit = cli_exit_for_class(ErrorClass::TransientRetriable, "boom");
    assert!(matches!(exit, Some(CliExit::NetworkError(_))));
    assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_UNAVAILABLE));
}

#[test]
fn cli_exit_for_class_internal_fatal_maps_to_scraper_failure_variant() {
    let exit = cli_exit_for_class(ErrorClass::InternalFatal, "bug");
    assert!(matches!(exit, Some(CliExit::ScraperFailure(_))));
    assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
}

#[test]
fn cli_exit_for_class_variant_dependent_classes_return_none() {
    assert_eq!(cli_exit_for_class(ErrorClass::PermanentFatal, "x"), None);
    assert_eq!(cli_exit_for_class(ErrorClass::DomainRecoverable, "x"), None);
}

// ---- Special cell — Cancelled ----

#[test]
fn cancelled_maps_to_success_exit_0_before_classification() {
    let exit = cancelled_exit(true).expect("cancellation must yield an exit");
    assert!(matches!(exit, CliExit::Success));
    assert_eq!(exit.report(), ExitCode::from(EXIT_SUCCESS));
}

#[test]
fn non_cancelled_run_yields_no_cancelled_interception() {
    assert_eq!(cancelled_exit(false), None);
}
