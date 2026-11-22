use super::*;
use std::process::Termination;

// UT-01: ConfigFile formatting
#[test]
fn test_format_cli_error_config_file() {
    let err = CliError::ConfigFile {
        msg: "invalid TOML".into(),
        suggestion: "Check syntax".into(),
    };
    let formatted = format_cli_error(&err, false);
    // CliError::ConfigFile renders its category in Spanish (user-facing).
    assert!(formatted.contains("Configuración"));
    assert!(formatted.contains("invalid TOML"));
    assert!(formatted.contains("Check syntax"));
}

// UT-01 (no_color): ConfigFile formatting without emoji
#[test]
fn test_format_cli_error_config_file_no_color() {
    let err = CliError::ConfigFile {
        msg: "invalid TOML".into(),
        suggestion: "Check syntax".into(),
    };
    let formatted = format_cli_error(&err, true);
    assert!(formatted.contains("[ERROR]"));
    assert!(!formatted.contains("❌"));
}

// XP-K-04 (#1608): under the test harness stderr is captured (never a
// terminal), so even without no_color the plain prefix is used — the
// decision is pinned here as a deterministic non-TTY case.
#[test]
fn test_format_cli_error_non_terminal_gets_plain_prefix() {
    let err = CliError::ConfigFile {
        msg: "invalid TOML".into(),
        suggestion: "Check syntax".into(),
    };
    assert!(
        !std::io::stderr().is_terminal(),
        "test harness must capture stderr for this pin to be deterministic"
    );
    let formatted = format_cli_error(&err, false);
    assert!(formatted.contains("[ERROR]"));
    assert!(!formatted.contains("❌"));
}

// UT-02: NetworkError formatting
#[test]
fn test_format_cli_error_network_error() {
    let err = CliError::NetworkError {
        msg: "connection refused".into(),
        suggestion: "Check your network".into(),
    };
    let formatted = format_cli_error(&err, false);
    // CliError::NetworkError renders its category in Spanish (user-facing).
    assert!(formatted.contains("Red"));
    assert!(formatted.contains("connection refused"));
}

// UT-03: PartialSuccess exit code
#[test]
fn test_cli_exit_partial_success_exit_code() {
    let exit = CliExit::PartialSuccess {
        success: 5,
        failed: 2,
    };
    let code = exit.report();
    assert_eq!(code, ExitCode::from(69));
}

// UT-04: Success exit code
#[test]
fn test_cli_exit_success_exit_code() {
    let exit = CliExit::Success;
    let code = exit.report();
    assert_eq!(code, ExitCode::from(0));
}

// UT-05: ConfigError exit code
#[test]
fn test_cli_exit_config_error_exit_code() {
    let exit = CliExit::ConfigError("bad config".into());
    let code = exit.report();
    assert_eq!(code, ExitCode::from(78));
}

// UT-06: NetworkError exit code
#[test]
fn test_cli_exit_network_error_exit_code() {
    let exit = CliExit::NetworkError("timeout".into());
    let code = exit.report();
    assert_eq!(code, ExitCode::from(69));
}

// T-1.1: Named constants are accessible and correct
#[test]
fn test_exit_code_constants_values() {
    assert_eq!(EXIT_SUCCESS, 0);
    assert_eq!(EXIT_EMPTY_DISCOVERY, 2);
    assert_eq!(EXIT_SCRAPER_FAILURE, 3);
    assert_eq!(EXIT_USAGE_ERROR, 64);
    assert_eq!(EXIT_DATA_ERROR, 65);
    assert_eq!(EXIT_UNAVAILABLE, 69);
    assert_eq!(EXIT_IO_ERROR, 74);
    assert_eq!(EXIT_PROTOCOL, 76);
    assert_eq!(EXIT_FORBIDDEN, 77);
    assert_eq!(EXIT_CONFIG, 78);
}

// T-1.2: DataFormatError variant maps to exit 65
#[test]
fn test_cli_exit_data_format_error_exit_code() {
    let exit = CliExit::DataFormatError("malformed XML".into());
    let code = exit.report();
    assert_eq!(code, ExitCode::from(EXIT_DATA_ERROR));
}

// T-1.3: EmptyDiscovery variant maps to exit 2
#[test]
fn test_cli_exit_empty_discovery_exit_code() {
    let exit = CliExit::EmptyDiscovery("No URLs found".into());
    let code = exit.report();
    assert_eq!(code, ExitCode::from(EXIT_EMPTY_DISCOVERY));
}

// T-1.3: Forbidden variant maps to exit 77
#[test]
fn test_cli_exit_forbidden_exit_code() {
    let exit = CliExit::Forbidden("all URLs blocked by robots.txt".into());
    let code = exit.report();
    assert_eq!(code, ExitCode::from(EXIT_FORBIDDEN));
}

// T-1.4: All variants map to their named constants (exhaustive)
#[test]
fn test_all_variants_map_to_named_constants() {
    let cases: Vec<(CliExit, u8)> = vec![
        (CliExit::Success, EXIT_SUCCESS),
        (CliExit::UsageError("test".into()), EXIT_USAGE_ERROR),
        (CliExit::DataFormatError("test".into()), EXIT_DATA_ERROR),
        (CliExit::NetworkError("test".into()), EXIT_UNAVAILABLE),
        (CliExit::IoError("test".into()), EXIT_IO_ERROR),
        (CliExit::ProtocolError("test".into()), EXIT_PROTOCOL),
        (CliExit::ConfigError("test".into()), EXIT_CONFIG),
        (CliExit::EmptyDiscovery("test".into()), EXIT_EMPTY_DISCOVERY),
        (CliExit::ScraperFailure("test".into()), EXIT_SCRAPER_FAILURE),
        (CliExit::Forbidden("test".into()), EXIT_FORBIDDEN),
        (
            CliExit::PartialSuccess {
                success: 1,
                failed: 1,
            },
            EXIT_UNAVAILABLE,
        ),
    ];
    for (exit, expected_code) in cases {
        let code = exit.report();
        assert_eq!(
            code,
            ExitCode::from(expected_code),
            "Expected exit code {expected_code}"
        );
    }
}
