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
    // summary, and the exit routing message. `redact_nondeterministic`
    // normalizes timestamps, trace IDs, module paths and .rs line numbers;
    // the resolver payload after webfang's "DNS error:" wrapper is the one
    // platform-dependent fragment, so the extra filter normalizes it — on
    // the multi-line text, BEFORE whitespace collapsing, so the newline
    // bounds the payload and cannot swallow the trailing summary lines.
    let redacted_line_level = redact_resolver_payload(&redact_nondeterministic(
        out_dir.path(),
        &String::from_utf8_lossy(&output.stderr),
    ));
    let redacted = normalize_captured(&redacted_line_level);
    assert_snapshot!("dns_failure_redacted_stderr", redacted);
}

/// Normalize the platform-dependent resolver payload that follows webfang's
/// own "DNS error:" wrapper (`DownloadError::Dns`), keeping the category and
/// everything around it stable across resolver stacks.
///
/// A comma is NOT a safe boundary: macOS `getaddrinfo` reports EAI_NONAME as
/// "nodename nor servname provided, or not known", so stopping at the first
/// comma used to leave ", or not known" in the snapshot. The payload ends at
/// `, url: ` (error line), `]` (`[Failed: ...]` summary), ` at ` (the retry
/// WARN added by #2835) or a newline (the final `Failed to scrape ...` line).
/// `regex` has no lookahead, so the delimiter is captured and re-emitted by
/// the replacement.
fn redact_resolver_payload(text: &str) -> String {
    match regex::Regex::new(r"(DNS error: )[^\]\n]*?(, url: |\]|\s+at\s|\n|$)") {
        Ok(re) => re.replace_all(text, "${1}<RESOLVER>${2}").into_owned(),
        Err(_) => text.to_owned(),
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::redact_resolver_payload;

    /// macOS `getaddrinfo` puts a comma inside the resolver payload
    /// ("... provided, or not known"): the redaction must swallow all of it.
    #[test]
    fn resolver_payload_with_internal_comma_is_fully_redacted() {
        let input = "DNS error: nodename nor servname provided, or not known, url: https://webfang-dns-test.invalid/, stage: scrape";
        assert_eq!(
            redact_resolver_payload(input),
            "DNS error: <RESOLVER>, url: https://webfang-dns-test.invalid/, stage: scrape"
        );
    }

    /// The `[Failed: ...]` summary bound is a closing bracket, not a comma.
    #[test]
    fn resolver_payload_in_failed_summary_is_fully_redacted() {
        let input = "[Failed: Other error: error de red: DNS error: nodename nor servname provided, or not known] Finished: 1 total";
        assert_eq!(
            redact_resolver_payload(input),
            "[Failed: Other error: error de red: DNS error: <RESOLVER>] Finished: 1 total"
        );
    }

    /// #2835 adds retry WARN lines whose payload ends in ` at <FILE>.rs:<LINE>`.
    #[test]
    fn resolver_payload_in_retry_warning_is_fully_redacted() {
        let input = "WARN: name resolution failed — retrying, error: DNS error: nodename nor servname provided, or not known at <FILE>.rs:<LINE>";
        assert_eq!(
            redact_resolver_payload(input),
            "WARN: name resolution failed — retrying, error: DNS error: <RESOLVER> at <FILE>.rs:<LINE>"
        );
    }

    /// The final `Failed to scrape ...` line ends at a newline, not at `, url: `.
    #[test]
    fn resolver_payload_at_end_of_line_is_fully_redacted() {
        let input = "Failed to scrape https://webfang-dns-test.invalid/: error de red: DNS error: nodename nor servname provided, or not known\nNo pages were successfully scraped";
        assert_eq!(
            redact_resolver_payload(input),
            "Failed to scrape https://webfang-dns-test.invalid/: error de red: DNS error: <RESOLVER>\nNo pages were successfully scraped"
        );
    }

    /// The Linux payload has no internal comma; its redacted shape must stay
    /// byte-identical so existing snapshots (including #2835's) remain valid.
    #[test]
    fn linux_payload_keeps_the_committed_snapshot_shape() {
        let input = "DNS error: Name or service not known, url: https://webfang-dns-test.invalid/, cause: \"dns_resolution\"";
        assert_eq!(
            redact_resolver_payload(input),
            "DNS error: <RESOLVER>, url: https://webfang-dns-test.invalid/, cause: \"dns_resolution\""
        );
    }
}
