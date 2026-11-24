//! Snapshot redaction: normalize every non-deterministic surface of test
//! output so committed snapshots are stable across machines, platforms and
//! runs.

use crate::redaction_network::collapse_connection_surface;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

/// Redact the per-run temp-dir path so snapshots stay stable across machines.
#[must_use]
pub fn redact_temp_path(dir: &Path, text: &str) -> String {
    text.replace(dir.to_string_lossy().as_ref(), "<OUT_DIR>")
}

/// Redact common non-deterministic output so snapshots are stable run-to-run:
/// the temp dir, ISO-8601 log timestamps, dynamic wiremock ports, ANSI color
/// escape sequences, source line numbers, tracing module and file paths,
/// trace/correlation UUIDs, and the OS-dependent network-failure surface.
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
    // Compiled once: the regex crate documents repeated compilation inside a
    // helper as an anti-pattern, and this chain runs on every redacted snapshot.
    static PATH_RUN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new("(<OUT_DIR>|<TEMP_PATH>)[^\\s\\x1b\"']*").expect("valid path run regex")
    });
    let text = PATH_RUN
        .replace_all(&text, |caps: &regex::Captures| caps[0].replace('\\', "/"))
        .into_owned();
    static ANSI: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("valid ANSI regex"));
    let text = ANSI.replace_all(&text, "").into_owned();
    static TS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)")
            .expect("valid timestamp regex")
    });
    let text = TS.replace_all(&text, "<TIMESTAMP>").into_owned();
    static PORT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"127\.0\.0\.1:\d+").expect("valid port regex"));
    let text = PORT.replace_all(&text, "127.0.0.1:<PORT>").into_owned();
    // Normalize source line numbers in tracing spans (e.g. "scrape_flow.rs:193").
    // These shift with `#[cfg(feature = "...")]` blocks and differ across
    // feature sets, so a snapshot that baked one in would pass locally and
    // fail in a differently-featured CI job.
    static LINE_NO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\.rs:)\d+").expect("valid line number regex"));
    let text = LINE_NO.replace_all(&text, "$1<LINE>").into_owned();
    // Normalize tracing module paths (e.g. "WARN webfang_core::cli::orchestrator:")
    // so snapshots decouple from source location and survive function moves (#462).
    static MODULE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"((?:WARN|INFO|ERROR|DEBUG|TRACE)\s+)\w+(?:::\w+)+")
            .expect("valid module regex")
    });
    let text = MODULE.replace_all(&text, "$1<MODULE>").into_owned();
    // Normalize trace/correlation UUIDs emitted by log_scrape_error's trace_id
    // field (#688) so trace snapshots stay deterministic run-to-run. Moved
    // here from webfang_core's private harness copy in #1649: this is the
    // chain every consumer shares, so the rule cannot be half-applied again.
    static TRACE_ID: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
            .expect("valid trace id regex")
    });
    let text = TRACE_ID.replace_all(&text, "<TRACE_ID>").into_owned();
    // Normalize tracing source file paths (e.g. "at crates/.../orchestrator.rs:<LINE>")
    // so moving a function between files does not break snapshots (#462).
    static FILE_PATH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(at\s+)\S+\.rs").expect("valid file path regex"));
    let text = FILE_PATH.replace_all(&text, "$1<FILE>.rs").into_owned();
    // #1631 + #2836: one canonical token for the OS-dependent connection
    // surface and the classified cause. Rules and rationale live with the
    // collapse in the redaction_network module, not here.
    collapse_connection_surface(text)
}
