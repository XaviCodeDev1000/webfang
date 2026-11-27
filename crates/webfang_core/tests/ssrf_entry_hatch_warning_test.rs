#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! The SSRF entry hatch must announce itself — and only itself (T1A, #3080).
//!
//! `WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1` is read by a SHIPPED binary: unlike its
//! two siblings (redirect guard, validating resolver) it is not confined to
//! `cfg(test)`, so an operator can arm it in release and get exactly what it
//! says — no warning at all. This file pins the three halves of the fix:
//!
//! 1. **It speaks, once.** An armed hatch produces ONE human-facing warning per
//!    run naming its exact scope: the literal-IP ENTRY rejection only.
//! 2. **It stays quiet otherwise.** A run without the hatch adds no such output,
//!    so the line is tied to the hatch and not to the startup path.
//! 3. **It disarms nothing else.** With the hatch armed, a `localhost` target
//!    (hostname → connect-time validating resolver) and a `302` to a cloud
//!    metadata literal (sync redirect policy) BOTH still end in exit 69
//!    through the real binary. The warning is a statement of scope, never a
//!    licence to reach an internal network.
//!
//! Where the line comes from: `arm_from_env` runs in `webfang_cli::main`
//! BEFORE `init_logging_dual`, so a `warn!` raised at the arming site would be
//! swallowed (#796). The notice is therefore emitted from the CLI
//! orchestration entry (`orchestrator::run`), which is downstream of the
//! subscriber and upstream of every route — dry-run, batch and scrape alike.
//!
//! Every assertion reads the spawned binary's own stderr; the harness
//! (`common::cli_harness`) arms the entry hatch for every spawned child
//! because wiremock binds a forbidden literal, so the unarmed case opts back
//! out with `env_remove` exactly like the SSRF matrix does.

#[path = "common/mod.rs"]
mod common;

use std::process::Output;

use common::cli_harness::{
    assert_snapshot_redacted, assert_spawn_within_output_blocking, mount_single_page_site,
    std_scraper_cmd, BehavioralTest, SPAWN_LATENCY_BUDGET,
};
use webfang_core::cli::error::EXIT_USAGE_ERROR;
use webfang_core::cli::ssrf_hatch_notice::SSRF_ENTRY_HATCH_NOTICE_MARKER as NOTICE_MARKER;
use webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// How many times `marker` appears in the captured stderr.
///
/// A plain substring count, and the strongest form available: the notice is a
/// single `tracing::warn!` whose prose appears once, as the event message, with
/// the facts travelling separately in structured fields. So a count of 1 is one
/// notice, full stop.
fn occurrences(stderr: &str, marker: &str) -> usize {
    stderr.matches(marker).count()
}

/// Scrape `url` with the spawned binary, optionally leaving the entry hatch
/// armed exactly as the harness arms it.
///
/// The closure moves onto the blocking pool, so it carries the URL, the output
/// path and the hatch decision BY VALUE instead of borrowing the harness.
async fn scrape_with(
    url: String,
    out: std::path::PathBuf,
    test_name: &'static str,
    hatch_armed: bool,
    extra: Vec<String>,
) -> Output {
    assert_spawn_within_output_blocking(SPAWN_LATENCY_BUDGET, test_name, move |child| {
        let mut cmd = std_scraper_cmd(&url, &out);
        if !hatch_armed {
            cmd.env_remove(DISABLE_ENTRY_GUARD_ENV);
        }
        cmd.arg("--single-page")
            .arg("--ignore-robots")
            .arg("--timeout-secs")
            .arg("5")
            .arg("--max-retries")
            .arg("0");
        for arg in &extra {
            cmd.arg(arg);
        }
        child.output(cmd)
    })
    .await
}

/// One armed scrape of the harness's own mock site.
async fn armed_scrape(t: &BehavioralTest, test_name: &'static str) -> Output {
    scrape_with(
        t.server.uri(),
        t.out.path().to_path_buf(),
        test_name,
        true,
        Vec::new(),
    )
    .await
}

/// Armed hatch → ONE warning that names literal-IP-entry-only scope, states
/// that the resolver and the redirect policy stay enforced, and says the hatch
/// survives into release builds.
#[tokio::test]
async fn hatch_armed_warns_exactly_once_with_its_exact_scope() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    let output = armed_scrape(&t, "hatch_armed_warns_exactly_once_with_its_exact_scope").await;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        output.status.success(),
        "the armed run must still scrape its target; stderr:\n{stderr}"
    );
    assert_eq!(
        occurrences(&stderr, NOTICE_MARKER),
        1,
        "an armed entry hatch must warn EXACTLY once per run, naming the \
         narrowed scope; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(DISABLE_ENTRY_GUARD_ENV),
        "the warning must name the variable an operator has to unset; \
         stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("validating DNS resolver"),
        "the warning must say the connect-time resolver stays armed; \
         stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("redirect policy"),
        "the warning must say the redirect policy stays armed; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("release builds"),
        "the warning must state that the hatch survives into release builds; \
         stderr:\n{stderr}"
    );

    assert_snapshot_redacted("hatch_armed_stderr", t.out.path(), stderr);
}

/// Without the hatch the run owes no such notice: the line belongs to the
/// hatch, not to the startup path every run walks.
#[tokio::test]
async fn hatch_absent_emits_no_such_notice() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    // The seed is a forbidden literal, so with the hatch gone the entry guard
    // cuts it — but only AFTER the subscriber exists, which is all this test
    // needs to compare against the armed case.
    let output = scrape_with(
        t.server.uri(),
        t.out.path().to_path_buf(),
        "hatch_absent_emits_no_such_notice",
        false,
        Vec::new(),
    )
    .await;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(69),
        "the armed entry guard must reject the literal seed; stderr:\n{stderr}"
    );
    assert_eq!(
        occurrences(&stderr, NOTICE_MARKER),
        0,
        "a run without the hatch must add no hatch notice at all; \
         stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("ssrf_entry_hatch_armed"),
        "the structured notice event must be absent too; stderr:\n{stderr}"
    );
}

/// The warning describes a NARROWER guard, not a missing one: with the hatch
/// armed, a hostname that answers loopback is still stopped at connect time by
/// the validating DNS resolver.
#[tokio::test]
async fn hatch_armed_still_blocks_a_hostname_resolving_to_loopback() {
    let server = MockServer::start().await;
    let url = format!("http://localhost:{}/", server.address().port());
    let out = tempfile::TempDir::new().expect("temp output dir");
    let output = scrape_with(
        url,
        out.path().to_path_buf(),
        "hatch_armed_still_blocks_a_hostname_resolving_to_loopback",
        true,
        Vec::new(),
    )
    .await;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(69),
        "the connect-time validating resolver must stay armed under the \
         hatch; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("name resolution failed"),
        "the rejection must be the resolver's own typed DNS failure; \
         stderr:\n{stderr}"
    );
    assert_eq!(
        occurrences(&stderr, NOTICE_MARKER),
        1,
        "the armed hatch must still announce its scope on this run; \
         stderr:\n{stderr}"
    );
}

/// The other half of the scope: the hatch disarms the ENTRY literal check, not
/// the synchronous redirect stop. A `302` to a cloud-metadata literal is
/// stopped before any socket opens, and the seed is the only request served.
#[tokio::test]
async fn hatch_armed_still_stops_a_redirect_to_a_metadata_literal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redir"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", "http://169.254.169.254/latest/meta-data/iam/"),
        )
        .mount(&server)
        .await;
    let output = scrape_with(
        format!("{}/redir", server.uri()),
        tempfile::TempDir::new()
            .expect("temp output dir")
            .path()
            .to_path_buf(),
        "hatch_armed_still_stops_a_redirect_to_a_metadata_literal",
        true,
        Vec::new(),
    )
    .await;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(69),
        "a blocked redirect must surface as a typed scrape failure; \
         stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("302"),
        "the 302 must surface as the terminal classification — proof the hop \
         was stopped, not followed; stderr:\n{stderr}"
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("wiremock request journal")
            .len(),
        1,
        "only the seed request may reach the mock: the hop never dials"
    );
}

// ============================================================================
// The boundary — deliberate silence, pinned
// ============================================================================

/// A run that never reaches the scraper does not announce the hatch.
///
/// This is the CONTRACT, not a gap: the hatch configures the SCRAPER's
/// literal-IP entry guard, so a clap usage error — which exits inside argv
/// parsing, before the orchestrator, before any fetch and before the CLI even
/// installs its tracing subscriber — never consults that guard and has no
/// scraper behaviour to qualify. Announcing it there would describe a decision
/// the run never takes. The assertion below fails the day that silence turns
/// accidental (a notice leaking into pre-scrape paths) or the day the hatch
/// starts changing argv validation's exit code.
#[tokio::test]
async fn a_clap_usage_error_never_announces_the_hatch() {
    let unarmed = usage_error_run("a_clap_usage_error_never_announces_the_hatch", false).await;
    let armed = usage_error_run("a_clap_usage_error_never_announces_the_hatch", true).await;
    let unarmed_status = unarmed.status.code();
    let armed_stderr = String::from_utf8_lossy(&armed.stderr).into_owned();

    assert_eq!(
        unarmed_status,
        Some(i32::from(EXIT_USAGE_ERROR)),
        "the reference run must be the usage contract (64)"
    );
    assert_eq!(
        armed.status.code(),
        unarmed_status,
        "the hatch must not change argv validation's exit code; armed stderr:\n{armed_stderr}"
    );
    assert_eq!(
        occurrences(&armed_stderr, NOTICE_MARKER),
        0,
        "THE SILENCE IS THE CONTRACT: a clap usage error exits before any \
         scrape, before the orchestrator and before the subscriber, so it must \
         never announce the hatch; stderr:\n{armed_stderr}"
    );
}

/// One `--no-such-flag` invocation of the real binary, hatch armed or not.
///
/// No mock server and no output directory: argv validation fails before either
/// would be touched, which is exactly the boundary this pins.
async fn usage_error_run(test_name: &'static str, hatch_armed: bool) -> Output {
    let out = tempfile::TempDir::new().expect("temp output dir");
    assert_spawn_within_output_blocking(SPAWN_LATENCY_BUDGET, test_name, move |child| {
        let mut cmd = std_scraper_cmd("https://example.invalid/", out.path());
        if !hatch_armed {
            cmd.env_remove(DISABLE_ENTRY_GUARD_ENV);
        }
        cmd.arg("--no-such-flag");
        child.output(cmd)
    })
    .await
}
