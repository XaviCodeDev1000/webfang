#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #2610: `--jitter-ms` breaks the metronome at the wire, end to end.
//!
//! Two claims are proved here, both against the SERVER-SIDE arrival log of
//! the shared #1433 fixture ([`start_fixture`] + [`RequestLog`]): the
//! arrivals are recorded when the request head completes, so nothing here
//! measures the clock.
//!
//! 1. **The jitter paces EVERY request, not only the refills.** The token
//!    bucket's burst is the other half of the metronome — N requests leaving
//!    at once is as loud as N requests leaving on a fixed tick — so with the
//!    jitter on, the first request is paced too. The URLs here stay BELOW
//!    the detector-derived burst, so a run without the jitter would land
//!    them back-to-back and this test would fail.
//! 2. **A server-dictated `Retry-After` still wins.** The jitter paces the
//!    NORMAL requests; it never shortens, replaces, or races the wait the
//!    server asked for (fetch guard chain stage 4).
//!
//! The fixture URLs are loopback, so the production SSRF guards refuse
//! them: the spawned binary arms ONLY the documented test-only hatches
//! (`WEBFANG_DISABLE_SSRF_ENTRY_GUARD`, applied by the shared [`cmd`]
//! sanitizer, plus the resolver hatch for the batch path). Nothing global
//! is mutated — child env only — so the tests stay parallel-safe.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::cmd;
use common::fixture_server::{start_fixture, RequestLog, SPACING_TOLERANCE};
use webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV;

/// Base delay under test. Small enough to keep the suite quick, large
/// enough that a gap below it is a real defect and not timer noise.
const DELAY_MS: u64 = 100;

/// Jitter under test. Deliberately large relative to [`DELAY_MS`]: the
/// assertions are about the FLOOR (never faster than the configured delay),
/// which the jitter can only raise.
const JITTER_MS: u64 = 1_000;

/// The `Retry-After` the fixture dictates, in seconds.
const RETRY_AFTER_SECS: u64 = 1;

/// Run `--batch` with `urls` on stdin to completion (blocking — no sleeps).
fn run_batch(urls: &[String], out: &tempfile::TempDir) {
    let mut batch_cmd = cmd();
    batch_cmd
        .env(DISABLE_VALIDATING_RESOLVER_ENV, "1")
        .arg("--batch")
        .arg("--delay-ms")
        .arg(DELAY_MS.to_string())
        .arg("--jitter-ms")
        .arg(JITTER_MS.to_string())
        .arg("--ignore-robots")
        .arg("--output")
        .arg(out.path())
        .write_stdin(urls.join("\n"))
        .timeout(std::time::Duration::from_secs(120))
        .assert()
        .success();
}

/// The arrival instants of the `/p*` page routes, oldest first.
fn page_arrivals(log: &RequestLog) -> Vec<std::time::Instant> {
    let mut instants: Vec<std::time::Instant> = log
        .requests()
        .into_iter()
        .filter(|entry| entry.path.starts_with("/p"))
        .map(|entry| entry.at)
        .collect();
    instants.sort_unstable();
    instants
}

/// Positive: with the jitter on, NO request leaves before the configured
/// delay has passed (#2610, AC1).
///
/// The floor is the contract the jitter must not break: the effective
/// interval is `delay_ms + U[0, jitter_ms]`, so a gap SHORTER than
/// `delay_ms` would mean the jitter broke the rate it was added to.
#[tokio::test]
async fn jitter_paces_every_request_not_only_the_refills() {
    let (base, log) = start_fixture().await;
    // Three URLs: fewer than the detector-derived burst on any machine with
    // more than two cores, so without the jitter they would land together.
    let urls: Vec<String> = (0..3).map(|i| format!("{base}/p{i}")).collect();
    let out = tempfile::TempDir::new().expect("temp output dir");
    run_batch(&urls, &out);

    let arrivals = page_arrivals(&log);
    assert_eq!(
        arrivals.len(),
        urls.len(),
        "every batch URL must arrive exactly once (a skip would fake a pass)"
    );

    let floor = std::time::Duration::from_millis((SPACING_TOLERANCE * DELAY_MS as f64) as u64);
    for pair in arrivals.windows(2) {
        let gap = pair[1].duration_since(pair[0]);
        assert!(
            gap >= floor,
            "server-side gap {gap:?} < {floor:?} — a request dispatched below the configured delay"
        );
    }
}

/// Positive: with the jitter on, the `Retry-After` the server dictated is
/// still respected (#2610, AC2).
///
/// The jitter paces the normal requests; the 429 wait belongs to the
/// downloader's retry loop (fetch guard chain stage 4) and is never
/// shortened by a pacing decision.
#[tokio::test]
async fn retry_after_is_respected_while_the_jitter_is_active() {
    let (base, log) = start_fixture().await;
    let out = tempfile::TempDir::new().expect("temp output dir");

    cmd()
        .arg("--url")
        .arg(format!("{base}/limited-slow"))
        .arg("--output")
        .arg(out.path())
        .arg("--delay-ms")
        .arg(DELAY_MS.to_string())
        .arg("--jitter-ms")
        .arg(JITTER_MS.to_string())
        .arg("--ignore-robots")
        .timeout(std::time::Duration::from_secs(120))
        .assert()
        .success();

    let arrivals: Vec<std::time::Instant> = log
        .requests()
        .into_iter()
        .filter(|entry| entry.path == "/limited-slow")
        .map(|entry| entry.at)
        .collect();
    assert_eq!(
        arrivals.len(),
        2,
        "the 429 must be retried exactly once (the fixture serves 200 after it)"
    );

    let gap = arrivals[1].duration_since(arrivals[0]);
    assert!(
        gap >= std::time::Duration::from_secs(RETRY_AFTER_SECS),
        "the retry waited {gap:?} — the server-dictated `Retry-After: {RETRY_AFTER_SECS}` was not respected"
    );
}
