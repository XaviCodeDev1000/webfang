use super::*;
use std::process::Termination;

// ---- Typed overrides over class defaults ----

#[test]
fn forbidden_override_when_all_blocked_maps_to_77() {
    let exit = forbidden_exit_when_all_blocked(0, 0, 5);
    assert!(matches!(exit, Some(CliExit::Forbidden(_))));
    assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_FORBIDDEN));
}

#[test]
fn forbidden_override_requires_nothing_scraped_or_failed() {
    // Any real failure or any scraped page keeps historical routing (None).
    assert_eq!(forbidden_exit_when_all_blocked(1, 0, 5), None);
    assert_eq!(forbidden_exit_when_all_blocked(0, 2, 5), None);
    assert_eq!(forbidden_exit_when_all_blocked(0, 0, 0), None);
}

#[test]
fn data_format_override_when_extraction_failed_maps_to_65() {
    let failures = vec![(
        "https://example.com".to_string(),
        crate::error::ScraperError::ExtractionFailed {
            url: "https://example.com".into(),
            reason: "empty shell".into(),
        },
    )];
    let exit = data_format_error_exit_when_extraction_failed(&failures);
    assert!(matches!(exit, Some(CliExit::DataFormatError(_))));
    assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_DATA_ERROR));
}

#[test]
fn data_format_override_ignores_non_extraction_failures() {
    let failures = vec![(
        "https://example.com".to_string(),
        crate::error::ScraperError::Network(Box::<std::io::Error>::new(std::io::Error::other(
            "reset",
        ))),
    )];
    assert_eq!(
        data_format_error_exit_when_extraction_failed(&failures),
        None
    );
    assert_eq!(data_format_error_exit_when_extraction_failed(&[]), None);
}

#[test]
fn empty_discovery_override_for_sitemap_variants_maps_to_2() {
    for err in [
        crate::error::ScraperError::SitemapEmpty,
        crate::error::ScraperError::SitemapNotFound("none".into()),
    ] {
        let exit = empty_discovery_exit_for(&err);
        assert!(matches!(exit, Some(CliExit::EmptyDiscovery(_))));
        assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_EMPTY_DISCOVERY));
    }
}

#[test]
fn empty_discovery_override_ignores_other_errors() {
    let err = crate::error::ScraperError::Network(Box::<std::io::Error>::new(
        std::io::Error::other("reset"),
    ));
    assert_eq!(empty_discovery_exit_for(&err), None);
}

/// #1381: a seed the entry guard cuts pre-socket must surface as the
/// documented null-result code (2), naming the guard's own English cause
/// and BOTH members of the operator hatch pair.
#[test]
fn issue_1381_refused_literal_seed_maps_to_empty_discovery_2() {
    let _guard =
        webfang_test_utils::EnvGuard::clean(&[crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV]);
    for raw in [
        "http://127.0.0.1:8080/",
        "http://10.0.0.5/",
        "http://169.254.169.254/latest/meta-data/",
    ] {
        let seed = Url::parse(raw).expect("test URLs always parse");
        let exit = empty_discovery_exit_when_seed_refused(&seed)
            .unwrap_or_else(|| panic!("{raw} must map to a null-result exit"));
        let CliExit::EmptyDiscovery(message) = exit else {
            panic!("a policy refusal must not escalate to a network failure");
        };
        assert!(
            message.contains("SSRF detected")
                && message.contains(crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV)
                && message.contains(crate::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV),
            "message must name the cause and both hatches, got: {message}"
        );
        assert_eq!(
            CliExit::EmptyDiscovery(message).report(),
            ExitCode::from(EXIT_EMPTY_DISCOVERY),
            "exit-code contract for a guard-refused preview"
        );
    }
}

/// The override must stay quiet for everything the entry guard does not
/// refuse: public literals and hostnames alike. A hostname is cut by the
/// connect-time resolver, whose error the CLI cannot tell from a real DNS
/// failure — claiming SSRF there would be a new lie (#1381 note).
#[test]
fn issue_1381_public_and_hostname_seeds_keep_the_null_result_quiet() {
    let _guard =
        webfang_test_utils::EnvGuard::clean(&[crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV]);
    for raw in ["https://example.com/", "https://93.184.216.34/"] {
        let seed = Url::parse(raw).expect("test URLs always parse");
        assert_eq!(
            empty_discovery_exit_when_seed_refused(&seed),
            None,
            "{raw} is not an entry-guard refusal"
        );
    }
}

/// Zero false positives: with the documented hatch armed the guard does not
/// refuse the seed, so an empty preview stays the empty preview it always
/// was (exit 0).
#[test]
fn issue_1381_entry_guard_hatch_silences_the_refusal() {
    let _guard = webfang_test_utils::EnvGuard::with(&[(
        crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    )]);
    let seed = Url::parse("http://127.0.0.1:8080/").expect("test URL always parses");
    assert_eq!(
        empty_discovery_exit_when_seed_refused(&seed),
        None,
        "an opted-out operator must not be told the seed was refused"
    );
}

#[test]
fn config_override_maps_to_78() {
    for err in [
        crate::error::ScraperError::Config("bad key".into()),
        crate::error::ScraperError::H2Config("bad alpn".into()),
    ] {
        let exit = config_error_exit_for(&err);
        assert!(matches!(exit, Some(CliExit::ConfigError(_))));
        assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_CONFIG));
    }
}

#[test]
fn config_override_ignores_non_config_errors() {
    let err = crate::error::ScraperError::InvalidUrl("nope".into());
    assert_eq!(config_error_exit_for(&err), None);
}

#[test]
fn permanent_io_override_maps_to_74() {
    let exit = permanent_io_error_exit_for(&std::io::Error::from(std::io::ErrorKind::NotFound));
    assert!(matches!(exit, Some(CliExit::IoError(_))));
    assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_IO_ERROR));
}

#[test]
fn transient_io_kinds_keep_the_class_default_instead_of_74() {
    // Rows 21: Interrupted / WouldBlock / TimedOut are TransientRetriable;
    // the class default (69) applies, so no typed override fires.
    for kind in [
        std::io::ErrorKind::Interrupted,
        std::io::ErrorKind::WouldBlock,
        std::io::ErrorKind::TimedOut,
    ] {
        let err = std::io::Error::from(kind);
        assert_eq!(permanent_io_error_exit_for(&err), None);
    }
}

#[test]
fn scraper_failure_override_when_internal_fatal_maps_to_3() {
    let failures = vec![
        (
            "https://example.com".to_string(),
            crate::error::ScraperError::Internal("boom".into()),
        ),
        (
            "https://example.org".to_string(),
            crate::error::ScraperError::Internal("crash".into()),
        ),
    ];
    let exit = scraper_failure_exit_when_internal_fatal(&failures);
    let CliExit::ScraperFailure(msg) = exit.clone().expect("internal-fatal must yield an exit")
    else {
        unreachable!("checked above");
    };
    assert_eq!(msg, "Scraper failure: 2 internal error(s) out of 2 URLs");
    assert_eq!(
        exit.expect("checked above").report(),
        ExitCode::from(EXIT_SCRAPER_FAILURE)
    );
}

#[test]
fn scraper_failure_override_ignores_non_internal_fatal_failures() {
    // TransientRetriable and PermanentFatal never trigger the exit-3 fold.
    let failures = vec![
        (
            "https://example.com".to_string(),
            crate::error::ScraperError::Network(Box::<std::io::Error>::new(std::io::Error::from(
                std::io::ErrorKind::TimedOut,
            ))),
        ),
        (
            "https://example.org".to_string(),
            crate::error::ScraperError::ExtractionFailed {
                url: "https://example.org".into(),
                reason: "empty shell".into(),
            },
        ),
    ];
    assert_eq!(scraper_failure_exit_when_internal_fatal(&failures), None);
    assert_eq!(scraper_failure_exit_when_internal_fatal(&[]), None);
}

// ---- Elastic-ingestion exit routing (matrix row 26, #1949) ----
//
// The batch call site routes EVERY ingestion failure through
// `ingestion_exit_for`; these pins hold the sub-case split the matrix row
// documents: transient backend failure → 69, panicked/cancelled task → 3.

#[test]
fn ingestion_transient_network_failure_maps_to_unavailable_69() {
    // Row 26: the re-download's transport failure (Network class) is the
    // transient backend failure the row is about — "unavailable, retry",
    // never IoError(74).
    let error = crate::error::ScraperError::Network(Box::new(std::io::Error::from(
        std::io::ErrorKind::ConnectionRefused,
    )));
    let exit = ingestion_exit_for(&error);
    assert!(matches!(exit, CliExit::NetworkError(_)));
    let CliExit::NetworkError(message) = exit.clone() else {
        unreachable!("checked above")
    };
    assert_eq!(exit.report(), ExitCode::from(EXIT_UNAVAILABLE));
    assert!(
        message.contains("Vector ingestion failed"),
        "the user-facing message must stay in English and name the phase: {message}"
    );
    assert!(
        message.contains("network error"),
        "the class-identifying prefix must survive the routing: {message}"
    );
}

#[test]
fn ingestion_transient_backoff_timeout_maps_to_unavailable_69() {
    // Row 26 names TransientBackoff as the class: a re-download timeout
    // lands on the same 69 through the class default.
    let exit = ingestion_exit_for(&crate::error::ScraperError::GlobalTimeout);
    assert!(matches!(exit, CliExit::NetworkError(_)));
    assert_eq!(exit.report(), ExitCode::from(EXIT_UNAVAILABLE));
}

#[test]
fn ingestion_panicked_task_maps_to_scraper_failure_3() {
    // `join_failure` (cli/elastic.rs) constructs exactly this variant for a
    // panicked task. `Ingestion` classifies InternalFatal, so the panic is
    // a job failure (3) — a data-loss defect is never a retryable outage.
    // #1949 pins the sub-case split instead of forcing 69 everywhere.
    let error = crate::error::ScraperError::ingestion(
        "elastic ingestion task panicked for https://example.test/pagina",
    );
    let exit = ingestion_exit_for(&error);
    assert!(matches!(exit, CliExit::ScraperFailure(_)));
    let CliExit::ScraperFailure(message) = exit.clone() else {
        unreachable!("checked above")
    };
    assert_eq!(exit.report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
    // The #1941 attribution contract survives the rerouting: English
    // context + the URL whose vectors were lost.
    assert!(
        message.contains("Vector ingestion failed")
            && message.contains("panicked")
            && message.contains("https://example.test/pagina"),
        "the failure must keep naming the phase, the cause and the URL: {message}"
    );
}

#[test]
fn ingestion_cancelled_task_maps_to_scraper_failure_3() {
    // `join_failure` constructs the same `Ingestion` variant for an aborted
    // task — same class, same exit, pinned separately so the split cannot
    // silently collapse onto one arm or the other.
    let error = crate::error::ScraperError::ingestion(
        "elastic ingestion task cancelled for https://example.test/pagina",
    );
    let exit = ingestion_exit_for(&error);
    assert!(matches!(exit, CliExit::ScraperFailure(_)));
    assert_eq!(exit.report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
}

#[test]
fn ingestion_persistence_failure_maps_to_scraper_failure_3() {
    // The SQLite/vector-sink write failure classifies InternalFatal (row
    // 23): a data-integrity error is never blindly retried.
    let exit = ingestion_exit_for(&crate::error::ScraperError::Persistence(
        "sqlite write failed".into(),
    ));
    assert!(matches!(exit, CliExit::ScraperFailure(_)));
    assert_eq!(exit.report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
}

#[test]
fn ingestion_ssrf_config_refusal_maps_to_config_error_78() {
    // The ingestion downloader refuses a forbidden literal BEFORE the dial
    // as a `Config` error — the existing typed override decides (78), not
    // the class default.
    let exit = ingestion_exit_for(&crate::error::ScraperError::Config(
        "destination forbidden by the SSRF entry guard".into(),
    ));
    assert!(matches!(exit, CliExit::ConfigError(_)));
    assert_eq!(exit.report(), ExitCode::from(EXIT_CONFIG));
}

#[test]
fn ingestion_permanent_io_maps_to_io_error_74_and_transient_keeps_69() {
    // Rows 21/22 through the ingestion path: a permanent-kind sink write
    // failure overrides to 74; the transient kinds keep the class default.
    let permanent = ingestion_exit_for(&crate::error::ScraperError::Io(std::io::Error::from(
        std::io::ErrorKind::PermissionDenied,
    )));
    assert!(matches!(permanent, CliExit::IoError(_)));
    assert_eq!(permanent.report(), ExitCode::from(EXIT_IO_ERROR));

    let transient = ingestion_exit_for(&crate::error::ScraperError::Io(std::io::Error::from(
        std::io::ErrorKind::TimedOut,
    )));
    assert!(matches!(transient, CliExit::NetworkError(_)));
    assert_eq!(transient.report(), ExitCode::from(EXIT_UNAVAILABLE));
}

#[test]
fn ingestion_payload_too_large_maps_to_data_format_error_65() {
    // Terminal arm / row 14: an oversized re-download is the one
    // ingestion-reachable PermanentFatal with no typed override — 65,
    // never a retryable 69.
    let exit = ingestion_exit_for(&crate::error::ScraperError::PayloadTooLarge);
    assert!(matches!(exit, CliExit::DataFormatError(_)));
    assert_eq!(exit.report(), ExitCode::from(EXIT_DATA_ERROR));
}
