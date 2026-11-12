#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! #1433 RC-4 net: 403 rotated-request counts.
//!
//! Pins the externally-observable cost of the 403 recovery path through the
//! REAL `webfang` binary against the shared fixture:
//!
//! | Route                   | Pinned outcome                                    |
//! |-------------------------|---------------------------------------------------|
//! | `/forbidden-always`     | exactly 2 arrivals (seed + one rotation), failure |
//! | `/forbidden-default-ua` | exactly 2 arrivals, distinct UAs, success        |
//!
//! The negative direction is structural: with the rotation neutered, the
//! seed 403 is terminal after a single arrival, so both count assertions
//! go red (verified by temporarily reverting the rotation, then restoring).
//!
//! Posture: the shared `std_cmd()` harness (entry layer disarmed for the
//! loopback fixture, child env only — no global mutation). Determinism:
//! counts only, no timing assertions, no sleeps.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::{spawn_budgeted, std_cmd};
use common::fixture_server::start_fixture;

/// Run a single-page scrape of `url` to completion under the shared spawn
/// budget, returning the captured output.
async fn scrape_once(
    what: &'static str,
    url: &str,
    out: &tempfile::TempDir,
) -> std::process::Output {
    let mut command = std_cmd();
    command
        .arg("--url")
        .arg(url)
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--output")
        .arg(out.path())
        .arg("--max-retries")
        .arg("3")
        .arg("--timeout-secs")
        .arg("10");
    spawn_budgeted(what, command, None).await
}

/// Collect every `.md` artifact under `dir` (files live in domain subdirs).
fn collect_md_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    let mut found = Vec::new();
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                found.push(path);
            }
        }
    }
    found
}

/// always-403 costs seed + ONE rotation: the rotated retry re-fires 403 and
/// the unified loop reports it terminal — no third default-UA request, no
/// 429/5xx backoff (the #1430 amplification fix, pinned from the outside).
#[tokio::test]
async fn always_403_costs_seed_plus_one_rotation() {
    let (base, log) = start_fixture().await;
    let out = tempfile::TempDir::new().expect("temp output dir");

    let output = scrape_once(
        "always_403_costs_seed_plus_one_rotation",
        &format!("{base}/forbidden-always"),
        &out,
    )
    .await;

    assert!(
        !output.status.success(),
        "an always-403 seed must fail the scrape"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("403"),
        "the terminal error must name the 403 status, got:\n{stderr}"
    );
    assert_eq!(
        log.count_path("/forbidden-always"),
        2,
        "always-403 must cost exactly seed + one rotation (paths: {:?})",
        log.paths()
    );
    let agents = log.user_agents();
    assert_eq!(agents.len(), 2, "two arrivals carry two UA identities");
    assert_ne!(
        agents[0], agents[1],
        "the second arrival must carry the rotated UA, not a re-fired default"
    );
}

/// 403-for-default-UA-only recovers via rotation: the seed 403 triggers one
/// rotated retry, which the fixture serves — success on exactly 2 arrivals.
#[tokio::test]
async fn default_ua_403_recovers_on_the_rotated_retry() {
    let (base, log) = start_fixture().await;
    let out = tempfile::TempDir::new().expect("temp output dir");

    let output = scrape_once(
        "default_ua_403_recovers_on_the_rotated_retry",
        &format!("{base}/forbidden-default-ua"),
        &out,
    )
    .await;

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "rotation must clear a default-UA-only 403: exit {:?}\nstderr: {stderr}",
        output.status.code()
    );
    assert_eq!(
        log.count_path("/forbidden-default-ua"),
        2,
        "recovery must cost exactly seed + one rotation (paths: {:?})",
        log.paths()
    );
    let agents = log.user_agents();
    assert_eq!(agents.len(), 2, "two arrivals carry two UA identities");
    assert_ne!(
        agents[0], agents[1],
        "the recovery arrival must carry the rotated UA"
    );
    assert!(
        !collect_md_files(out.path()).is_empty(),
        "the recovered page must write a Markdown artifact"
    );
}
