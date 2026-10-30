#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! #2490: the sitemap parser's fetch client must honor the operator's
//! `--timeout-secs`.
//!
//! The sitemap discovery probes build their client through
//! `build_discovery_client`, which inherits the configured deadline under the
//! #281 connect-cap policy. The parser's fetch client did not: it built its
//! own client on a private default, so an explicit `--sitemap-url` against a
//! peer that accepts connections but never answers kept the run alive for the
//! parser's default deadline no matter what the operator configured (measured
//! at 113 s in #2490 against a 1 s `--timeout-secs`).
//!
//! The pin is the deadline PROPERTY, not a latency figure: the budget below is
//! wide enough for the healthy path (the configured deadline spent a handful
//! of times by the parser's own retry policy) and far tighter than the buggy
//! regime, so the harness's `assert_spawn_within` kill is the failure signal
//! and no passing run is ever timed.

use std::time::Duration;

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::{assert_spawn_within, std_cmd, BudgetChild};
use tempfile::TempDir;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Upper bound separating the two regimes: with the inheritance fixed, the
/// parser's client times out at the configured 1 s per attempt and the
/// parser's retry policy (3 attempts, 1 s + 2 s backoffs) exhausts in ~8.5 s
/// on a healthy runner; with the bug, every attempt burns the parser's
/// private default and the run measures minutes (#2490 measured 113 s).
///
/// 60 s is the session's standard margin (~7x the healthy path). The original
/// 15 s was 1.76x, and #2533's windows-latest FLKY-FL measured the healthy
/// path consuming the entire budget on both consecutive attempts under runner
/// load (latencies [15.02 s, 15.01 s]) — real work, not environment noise.
/// Two 60 s attempts (120 s) stay under nextest's per-attempt ceilings
/// (default 180 s, ci 240 s), and the budget still sits far below the buggy
/// regime, so a regression keeps producing the clean `Overran` signal and no
/// passing run is ever timed.
const SITEMAP_FETCH_DEADLINE_BUDGET: Duration = Duration::from_secs(60);

/// A peer partitioned at the sitemap fetch: it accepts every connection and
/// holds it for 10 minutes before answering, so no client deadline can win a
/// race against the response — the run's duration depends only on the client's
/// own deadline (the #1828 partition fixture, aimed at the parser fetch).
async fn mount_stalled_sitemap_peer() -> MockServer {
    let stalled = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(600)))
        .mount(&stalled)
        .await;
    stalled
}

/// The operator's `--timeout-secs` reaches the sitemap fetch itself: a stalled
/// explicit-URL sitemap must fail the run at the configured deadline, never
/// after the parser's private default.
#[tokio::test]
async fn sitemap_fetch_with_explicit_timeout_secs_fails_at_the_operator_deadline() {
    let stalled = mount_stalled_sitemap_peer().await;
    let state_dir = TempDir::new().expect("state tempdir");
    let out_dir = TempDir::new().expect("output tempdir");
    let sitemap_url = format!("{}/sitemap.xml", stalled.uri());

    assert_spawn_within(
        SITEMAP_FETCH_DEADLINE_BUDGET,
        "sitemap fetch honors the operator's --timeout-secs",
        move |child: &BudgetChild| {
            let mut command = std_cmd();
            command
                .args([
                    "--resume",
                    "--url",
                    stalled.uri().as_str(),
                    "--use-sitemap",
                    "--sitemap-url",
                    sitemap_url.as_str(),
                    "--timeout-secs",
                    "1",
                    "--state-dir",
                ])
                .arg(state_dir.path())
                .arg("--output")
                .arg(out_dir.path())
                .arg("--quiet");
            let output = child.output(command)?;

            if output.status.success() {
                return Err(format!(
                    "a stalled sitemap peer must fail the run at the configured deadline, got a \
                     clean exit; stderr: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(())
        },
    );
}
