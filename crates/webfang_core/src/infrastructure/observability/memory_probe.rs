//! Task 5.1 — bounded-memory measurement harness (change
//! stabilization-concurrency-budget, decision Q3 MEASURE FIRST).
//!
//! RSS reader over `/proc/self/status` `VmRSS` (Linux CI) plus a tiny
//! append-only report writer. Probe tests co-located with each measured
//! structure write one line per scenario so the committed BEFORE/AFTER tables
//! are produced by the same code path on any machine.
//!
//! Design constraints honored here:
//! - No absolute-byte assertions on live measurements (coarse plateau
//!   detection only); parser fixtures are exact by construction.
//! - No new dependencies and no `unsafe` (plain std fs reads).
//! - Report destination is env-driven: `WEBFANG_MEMORY_REPORT_PATH`
//!   (default `target/memory-report.md`).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// Current resident set size in bytes, read from `/proc/self/status` `VmRSS`.
///
/// The kernel reports the value in kB, so no page-size API is needed: no
/// `unsafe` call and no new dependency. Returns `None` outside Linux or when
/// the field is absent or malformed.
#[must_use]
pub fn rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_vmrss_bytes(&status)
}

/// Parse the current-RSS field of a `/proc/self/status` snapshot.
///
/// `VmHWM` (peak) is deliberately ignored: callers want current RSS.
fn parse_vmrss_bytes(status: &str) -> Option<u64> {
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kb = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kb.saturating_mul(1024))
}

/// Destination of the measurement report. `None` when
/// `WEBFANG_MEMORY_REPORT_PATH` is unset: probes then print to stdout instead
/// of touching the filesystem, so ordinary test runs never litter the working
/// tree or `target/`.
#[must_use]
pub fn report_path() -> Option<PathBuf> {
    std::env::var("WEBFANG_MEMORY_REPORT_PATH")
        .ok()
        .map(PathBuf::from)
}

/// Append one probe line under `section`. With the env var set, creates the
/// file (with a header) when missing; without it, prints the line instead.
/// Never panics on I/O failure — measurement must not break the test that
/// hosts it; failures surface as a stderr note instead.
pub fn append_report(section: &str, line: &str) {
    let Some(path) = report_path() else {
        println!("memory_probe [{section}]: {line}");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let needs_header = !path.exists();
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        eprintln!("memory_probe: cannot open report at {}", path.display());
        return;
    };
    if needs_header {
        let _ = writeln!(file, "# Memory probe report\n");
    }
    let _ = writeln!(file, "## {section}\n{line}\n");
}

/// Format an optional byte count for reports (`n/a` off-Linux).
#[must_use]
pub fn fmt_rss(rss: Option<u64>) -> String {
    match rss {
        Some(b) => format!("{b} bytes ({:.1} MiB)", b as f64 / (1024.0 * 1024.0)),
        None => "n/a (non-Linux)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vmrss_is_parsed_from_proc_status() {
        let status = "Name:\twrapper\nVmRSS:\t  12345 kB\nVmHWM:\t 99999 kB\n";
        assert_eq!(parse_vmrss_bytes(status), Some(12_345 * 1024));
    }

    #[test]
    fn missing_vmrss_is_none() {
        assert_eq!(parse_vmrss_bytes("Name:\twrapper\nVmHWM:\t 9 kB\n"), None);
    }

    #[test]
    fn malformed_vmrss_is_none() {
        assert_eq!(parse_vmrss_bytes("VmRSS:\t nope kB\n"), None);
    }

    #[test]
    fn live_rss_is_available_on_linux() {
        if std::path::Path::new("/proc/self/status").exists() {
            assert!(rss_bytes().is_some(), "VmRSS must be readable on Linux");
        }
    }
}
