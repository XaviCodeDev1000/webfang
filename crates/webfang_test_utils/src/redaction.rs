//! Snapshot redaction: normalize every non-deterministic surface of test
//! output so committed snapshots are stable across machines, platforms and
//! runs.

use crate::redaction_network::{collapse_connection_surface, collapse_resolver_payload};
use regex::Regex;
use std::path::Path;

/// Redact the per-run temp-dir path so snapshots stay stable across machines.
#[must_use]
pub fn redact_temp_path(dir: &Path, text: &str) -> String {
    text.replace(dir.to_string_lossy().as_ref(), "<OUT_DIR>")
}

/// Redact common non-deterministic output so snapshots are stable run-to-run:
/// the temp dir, ISO-8601 log timestamps, dynamic wiremock ports, ANSI color
/// escape sequences, source line numbers, tracing module and file paths,
/// trace/correlation UUIDs, the OS-dependent network-failure surface, and the
/// platform-dependent resolver payload.
///
/// This is THE chain: `webfang_core`'s test harness re-exports it rather
/// than keeping a second copy (#1649 — the copies drifted, and only this
/// side had the `#688` `<TRACE_ID>` rule).
///
/// # Panics
///
/// Panics if any of the built-in redaction regular expressions fail to
/// compile. They are static literals, so this only happens if a regression
/// corrupts the pattern.
#[must_use]
pub fn redact_nondeterministic(dir: &Path, text: &str) -> String {
    let text = redact_temp_path(dir, text);
    // #1777: the redaction token replaces a directory PREFIX, but the separators
    // that FOLLOW it are the OS's, and they survive verbatim — Windows renders
    // `<OUT_DIR>\typo.toml` where POSIX renders `<OUT_DIR>/typo.toml`. A
    // committed snapshot can only ever match one of them, which is how four
    // `config_default_contract_test` snapshots passed on Linux and failed on
    // `windows-latest` with nothing but this character between them.
    //
    // The rule is anchored to the token on purpose. Rewriting every backslash
    // would corrupt any message that carries one as content (a regex, a
    // literal path, a Windows-style escape) and silently fork every snapshot
    // that contains one.
    //
    // #2242 extended the anchored run from ONE separator to the WHOLE path
    // that follows the token: a nested platform path such as
    // `<OUT_DIR>\webfang\config.toml` renders its INNER separators verbatim
    // too, and a single-separator rewrite left `<OUT_DIR>/webfang\config.toml`
    // — a snapshot no single platform can produce on both. The run is the
    // token plus every path character up to whitespace, an ANSI escape or a
    // quote, and every backslash inside it becomes `/`; text outside the run
    // is untouched, so a backslash that is message content (not part of the
    // redacted path) never crosses this rule.
    let path_run =
        Regex::new("(<OUT_DIR>|<TEMP_PATH>)[^\\s\\x1b\"']*").expect("valid path run regex");
    let text = path_run
        .replace_all(&text, |caps: &regex::Captures| caps[0].replace('\\', "/"))
        .into_owned();
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").expect("valid ANSI regex");
    let text = ansi.replace_all(&text, "").into_owned();
    let ts = Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)")
        .expect("valid timestamp regex");
    let text = ts.replace_all(&text, "<TIMESTAMP>").into_owned();
    let port = Regex::new(r"127\.0\.0\.1:\d+").expect("valid port regex");
    let text = port.replace_all(&text, "127.0.0.1:<PORT>").into_owned();
    // Normalize source line numbers in tracing spans (e.g. "scrape_flow.rs:193").
    // These shift with `#[cfg(feature = "...")]` blocks and differ across
    // feature sets, so a snapshot that baked one in would pass locally and
    // fail in a differently-featured CI job.
    let line_no = Regex::new(r"(\.rs:)\d+").expect("valid line number regex");
    let text = line_no.replace_all(&text, "$1<LINE>").into_owned();
    // Normalize tracing module paths (e.g. "WARN webfang_core::cli::orchestrator:")
    // so snapshots decouple from source location and survive function moves (#462).
    let module = Regex::new(r"((?:WARN|INFO|ERROR|DEBUG|TRACE)\s+)\w+(?:::\w+)+")
        .expect("valid module regex");
    let text = module.replace_all(&text, "$1<MODULE>").into_owned();
    // Normalize trace/correlation UUIDs emitted by log_scrape_error's trace_id
    // field (#688) so trace snapshots stay deterministic run-to-run. Moved
    // here from webfang_core's private harness copy in #1649: this is the
    // chain every consumer shares, so the rule cannot be half-applied again.
    let trace_id =
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
            .expect("valid trace id regex");
    let text = trace_id.replace_all(&text, "<TRACE_ID>").into_owned();
    // Normalize tracing source file paths (e.g. "at crates/.../orchestrator.rs:<LINE>")
    // so moving a function between files does not break snapshots (#462).
    let file_path = Regex::new(r"(at\s+)\S+\.rs").expect("valid file path regex");
    let text = file_path.replace_all(&text, "$1<FILE>.rs").into_owned();
    // #1631 + #2836: one canonical token for the OS-dependent connection
    // surface and the classified cause. Rules and rationale live with the
    // collapse in the redaction_network module, not here.
    let text = collapse_connection_surface(text);
    // Last rule: the resolver payload sees the same text the committed
    // snapshots carried when the collapse lived in a single test file.
    collapse_resolver_payload(text)
}
