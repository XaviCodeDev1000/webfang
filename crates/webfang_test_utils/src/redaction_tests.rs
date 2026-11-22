use super::*;
use std::path::Path;

/// #2242 regression pin: the token-anchored separator rule must cover the
/// WHOLE path run that follows the token, not only the first separator.
/// The platform default config path nests one level (`<base>/webfang/
/// config.toml`), so Windows renders inner `\` separators that a
/// single-separator rewrite left in the snapshot.
#[test]
fn redaction_normalizes_every_separator_after_the_token() {
    let dir = Path::new("C:\\Users\\runner\\tmp\\base");
    let text = "el archivo de configuración C:\\Users\\runner\\tmp\\base\\webfang\\config.toml \
                no es TOML válido";
    let redacted = redact_nondeterministic(dir, text);
    assert!(
        redacted.contains("<OUT_DIR>/webfang/config.toml"),
        "every separator after the token must normalize: {redacted}"
    );
}

#[test]
fn redact_nondeterministic_normalizes_timestamps() {
    let dir = Path::new("/tmp/test");
    let input = "at 2024-03-15T10:30:00.123+01:00 done";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "at <TIMESTAMP> done");
}

#[test]
fn redact_nondeterministic_normalizes_ports() {
    let dir = Path::new("/tmp/test");
    let input = "server at 127.0.0.1:8080 started";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "server at 127.0.0.1:<PORT> started");
}

#[test]
fn redact_nondeterministic_strips_ansi() {
    let dir = Path::new("/tmp/test");
    let input = "\x1b[31merror\x1b[0m: something failed";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "error: something failed");
}

#[test]
fn redact_nondeterministic_replaces_temp_path() {
    let dir = Path::new("/tmp/.tmpABC123");
    let input = "wrote /tmp/.tmpABC123/output.md";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "wrote <OUT_DIR>/output.md");
}

/// #1777: the redaction token stands in for a directory PREFIX, but the
/// separator that FOLLOWED the prefix is the OS's. A message carrying a
/// real Windows path renders `<OUT_DIR>\typo.toml` where POSIX renders
/// `<OUT_DIR>/typo.toml`, so a committed snapshot can only ever match one
/// of them. `override_is_relative` passes on both platforms precisely
/// because its message carries no separator.
#[test]
fn redact_nondeterministic_normalizes_the_separator_after_a_path_token() {
    let dir = Path::new("/tmp/test");
    let input = "no existe el archivo de configuración: <OUT_DIR>\\typo.toml";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(
        result,
        "no existe el archivo de configuración: <OUT_DIR>/typo.toml"
    );
}

/// The counterpart claim: a backslash that is NOT adjacent to a redaction
/// token is content, not a separator, and must survive untouched. Without
/// this the rule above could be widened to "replace every backslash",
/// which would silently corrupt any snapshot whose message happens to
/// contain one (a Windows-style escape, a regex, a literal path).
#[test]
fn redact_nondeterministic_leaves_a_backslash_that_is_not_after_a_token() {
    let dir = Path::new("/tmp/test");
    let input = r"patrón \N no coincide y <OUT_DIR>/ok.toml sí";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, r"patrón \N no coincide y <OUT_DIR>/ok.toml sí");
}

#[test]
fn redact_nondeterministic_normalizes_line_numbers() {
    let dir = Path::new("/tmp/test");
    let input = "see scrape_flow.rs:193 for details";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "see scrape_flow.rs:<LINE> for details");
}

#[test]
fn redact_nondeterministic_normalizes_tracing_module_paths() {
    let dir = Path::new("/tmp/test");
    let input =
        "  2024-03-15T10:30:00+01:00  WARN webfang_core::cli::orchestrator: Unknown profile";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "  <TIMESTAMP>  WARN <MODULE>: Unknown profile");
}

/// #688 pin: `log_scrape_error` stamps a `trace_id` UUID on every error
/// event, so any snapshot of stderr would bake a fresh UUID per run
/// without this rule. The rule lives here (the shared chain) since
/// #1649, so EVERY consumer redacts it — previously only
/// `webfang_core`'s private harness copy had it, and this distributed
/// copy did not.
#[test]
fn redact_nondeterministic_normalizes_trace_ids() {
    let dir = Path::new("/tmp/test");
    let input =
        "error de red: failed trace_id=3f2504e0-4f89-11d3-9a0c-0305e82c3301 correlation=ABCDEF01-2345-6789-ABCD-EF0123456789";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(
        result,
        "error de red: failed trace_id=<TRACE_ID> correlation=<TRACE_ID>"
    );
}

#[test]
fn redact_nondeterministic_normalizes_tracing_file_paths() {
    let dir = Path::new("/tmp/test");
    let input = "    at crates/webfang_core/src/cli/orchestrator.rs:42";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "    at <FILE>.rs:<LINE>");
}

/// INT-1 (#1631) pin, Unix shape: the errno is OS text (`os error 111`
/// on Linux, `61` on macOS) and a snapshot must not bake it in. The
/// surrounding Spanish error prefix is product-owned and survives; the
/// `I/O error: ` layer does NOT, because the Windows timeout path has no
/// such layer and keeping it made the two platforms disagree — see
/// `redact_nondeterministic_makes_both_platforms_agree`.
#[test]
fn redact_nondeterministic_collapses_unix_connection_refused() {
    let dir = Path::new("/tmp/test");
    let input = "error de red: I/O error: Connection refused (os error 111)";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "error de red: <NET_ERR>");
}

/// INT-1 (#1631) pin, Windows shape: the refusal is WSA prose plus the
/// same errno parenthetical, so the WHOLE phrase must collapse — a
/// narrower rule that only replaced the number would still leave
/// Windows-only text in a Linux-authored snapshot.
#[test]
fn redact_nondeterministic_collapses_windows_wsa_prose() {
    let dir = Path::new("/tmp/test");
    let input = "No connection could be made because the target machine actively refused it. (os error 10061)";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "<NET_ERR>");
}

/// INT-1 (#1631) pin, Windows timeout-degradation shape: an unreachable
/// host on Windows never refuses, it times out, so the snapshot has to
/// see the SAME token the Unix refusal produces.
#[test]
fn redact_nondeterministic_collapses_windows_timeout_degradation() {
    let dir = Path::new("/tmp/test");
    let input = "error de red: request timed out after 2s";
    let result = redact_nondeterministic(dir, input);
    assert_eq!(result, "error de red: <NET_ERR>");
}

/// The pin that would have caught the `I/O error: ` layer the first two
/// pins missed. Both strings below are copied BYTE-EXACT from the stderr
/// each platform produced for `unreachable_host_stderr_mentions_failure`
/// (Windows lane, run 36356048341) — not reconstructed from memory:
///
/// ```text
/// linux:   error de red: I/O error: Connection refused (os error 111)
/// windows: error de red: request timed out after 2s
/// ```
///
/// Written as two separate pins, they both passed while the RULE was wrong,
/// because each pin asserted only its own shape: the Unix pin expected the
/// `I/O error: ` layer to SURVIVE, and the Windows pin had no layer to
/// begin with. The defect was only visible when the two are compared, so
/// this asserts the comparison itself — the whole point of the collapse.
#[test]
fn redact_nondeterministic_makes_both_platforms_agree() {
    let dir = Path::new("/tmp/test");
    let unix = redact_nondeterministic(
        dir,
        "Failed to scrape http://x/: error de red: I/O error: Connection refused (os error 111)",
    );
    let windows = redact_nondeterministic(
        dir,
        "Failed to scrape http://x/: error de red: request timed out after 2s",
    );
    assert_eq!(unix, windows, "the two platforms must redact identically");
    assert_eq!(
        unix, "Failed to scrape http://x/: error de red: <NET_ERR>",
        "and both must land on the documented token"
    );
}
