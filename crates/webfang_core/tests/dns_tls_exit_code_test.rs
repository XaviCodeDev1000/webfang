#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #1824 behavioral regression: DNS resolution failure must surface as the
//! network exit code (69, EX_UNAVAILABLE), never as exit 3 (ScraperFailure).
//!
//! The classifier's fallback arm used to damn every non-transient
//! `ScraperError::Network` — DNS resolution and TLS handshake failures among
//! them — to [`ErrorClass::InternalFatal`], which the CLI boundary maps to
//! exit 3 ("internal error"), pointing the operator at webfang instead of the
//! network. This suite drives the REAL `webfang` binary (debug profile,
//! production posture) against a guaranteed-unresolvable host and pins the
//! observed contract: exit 69.
//!
//! Determinism notes (test-quality rule 6): the target uses the `.invalid`
//! TLD, reserved by RFC 2606 and never delegated. With a resolver the answer
//! is NXDOMAIN; without any resolver, resolution also fails — either way the
//! host is unresolvable and no socket is ever opened. The resolver-dependent
//! payload inside webfang's own "DNS error:" wrapper varies by platform, so
//! the snapshot normalizes it to `<RESOLVER>` while keeping the surrounding
//! stderr shape (status progression, failure summary, exit routing message).

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::{redact_nondeterministic, spawn_budgeted, std_cmd};
use insta::assert_snapshot;

/// Guaranteed-unresolvable host per RFC 2606: `.invalid` is a reserved TLD
/// that no DNS server ever answers authoritatively.
const UNRESOLVABLE_URL: &str = "https://webfang-dns-test.invalid/";

/// Normalize captured stderr: drop ESC bytes and collapse whitespace runs
/// (same defensive floor the SSRF E2E suite applies to tracing output).
fn normalize_captured(s: &str) -> String {
    s.replace('\x1b', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Single-run scrape of a host that cannot resolve must exit 69
/// (`CliExit::NetworkError` / `EXIT_UNAVAILABLE`), NOT 3
/// (`CliExit::ScraperFailure` — the internal-bug bucket, #1824).
#[tokio::test]
async fn unresolvable_host_exits_69_network_unavailable_not_3() {
    let out_dir = tempfile::TempDir::new().expect("temp output dir");
    let mut command = std_cmd();
    command
        .arg("--url")
        .arg(UNRESOLVABLE_URL)
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--output")
        .arg(out_dir.path())
        .arg("--timeout-secs")
        .arg("10")
        .arg("--max-retries")
        .arg("0");
    let output = spawn_budgeted(
        "unresolvable_host_exits_69_network_unavailable_not_3",
        command,
        None,
    )
    .await;

    let stderr = normalize_captured(&String::from_utf8_lossy(&output.stderr));
    let code = output.status.code().unwrap_or(0);
    assert_eq!(
        code, 69,
        "DNS resolution failure must exit 69 (EX_UNAVAILABLE), got {code}. stderr:\n{stderr}"
    );
    // The failure must surface as the network family's message, not
    // as an internal-error report (#1824). The transport wrapper text is
    // webfang's own ("network error: DNS error: …").
    assert!(
        stderr.contains("network error: DNS error:"),
        "DNS failure must surface as the typed network error, got:\n{stderr}"
    );

    // Pin the full redacted stderr shape: status progression, failure
    // summary, and the exit routing message. `redact_nondeterministic` owns
    // every platform-dependent fragment, including the resolver payload,
    // and runs on the multi-line text BEFORE whitespace collapsing so the
    // newline bounds the payload and cannot swallow the trailing lines.
    let redacted = normalize_captured(&redact_nondeterministic(
        out_dir.path(),
        &String::from_utf8_lossy(&output.stderr),
    ));
    assert_snapshot!("dns_failure_redacted_stderr", redacted);
}
