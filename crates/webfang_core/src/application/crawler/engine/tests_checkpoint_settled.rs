//! #3079 / E13 — the settled (terminal-verdict) half of checkpoint resume.
//!
//! `CrawlCheckpoint.visited` is marked at DISPATCH, so it cannot be read as
//! "done": the resume decision is `visited ∩ settled` (skip) versus the rest
//! (re-drive), and a settled URL is never queued again. These three tests drive
//! the engine + scheduler surface a resumed run actually uses; the binary-level
//! probe lives in `tests/checkpoint_resume_crash_test.rs`.
//!
//! Split from `tests_checkpoint.rs` by the file-size ratchet (#2584): the
//! scenarios there cover the verdict/close contract, these cover the resume
//! decision itself.

use super::tests_checkpoint::{checkpoint_file_for, checkpoint_session_engine};
use super::tests_common::{entry_off, session_engine};
use super::*;
use url::Url;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

// ——— #3079 / E13: `visited` is dispatch, not completion ———

/// A checkpoint whose `visited` set holds pages that were never durably
/// exported must leave them PENDING on resume.
///
/// `CrawlScheduler::next_url` marks a URL visited when it HANDS IT OUT, so a
/// page still in memory when the process died is in `visited` and was never
/// exported (E13: 11 visited, 6 exported, exit 0). Reading `visited` as
/// "done" silently drops those pages; the truthful reading restores only the
/// URLs whose outcome is terminal (`visited ∩ settled`) and re-drives the
/// rest. Asserted through the scheduler surface a resume actually uses:
/// `next_url` must hand the page out again once it is rediscovered.
#[tokio::test]
#[cfg_attr(
    miri,
    ignore = "btls-sys FFI (wreq Client via session_engine) not supported by Miri: TLS_method"
)]
async fn visited_but_not_settled_pages_are_pending_on_resume() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).build();
    let mut engine = session_engine(config);

    let never_exported = "http://127.0.0.1:9/never-exported";
    let mut checkpoint = CrawlCheckpoint::new();
    checkpoint.visited.insert(never_exported.to_string());
    checkpoint.pages_crawled = 1;
    engine.checkpoint_state = Some(checkpoint);

    engine.restore_checkpoint_state();

    assert!(
        !engine.scheduler.snapshot_visited().contains(never_exported),
        "a visited-but-unsettled URL must NOT be restored as visited: it was \
         dispatched, never exported (#3079)"
    );

    // Discovery re-adds it on the resumed run; the scheduler must hand it out.
    engine
        .scheduler
        .restore_pending(&[never_exported.to_string()]);
    let next = engine
        .scheduler
        .next_url(0)
        .expect("the never-exported page must be re-driven by the resume");
    assert_eq!(next.url.as_str(), never_exported);
}

/// A settled URL is filtered out of the restored frontier and restored as
/// visited, so a resume can neither dispatch it nor replay it.
#[tokio::test]
#[cfg_attr(
    miri,
    ignore = "btls-sys FFI (wreq Client via session_engine) not supported by Miri: TLS_method"
)]
async fn settled_urls_are_restored_as_visited_and_never_queued() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).build();
    let mut engine = session_engine(config);

    let gone = "http://127.0.0.1:9/gone";
    let mut checkpoint = CrawlCheckpoint::new();
    checkpoint.visited.insert(gone.to_string());
    checkpoint.settled.insert(gone.to_string());
    checkpoint.queued = vec![gone.to_string()];
    engine.checkpoint_state = Some(checkpoint);

    engine.restore_checkpoint_state();

    assert!(
        engine.scheduler.snapshot_visited().contains(gone),
        "a settled URL must be restored as visited, or rediscovery replays it"
    );
    assert_eq!(
        engine.scheduler.pending_len(),
        0,
        "a settled URL must be filtered out of the restored frontier"
    );
    assert!(
        engine.scheduler.next_url(0).is_none(),
        "a settled URL must never be dispatched by a resumed run"
    );
    assert!(
        engine.scheduler.snapshot_settled().contains(gone),
        "the verdict must survive the restore so later saves keep it"
    );
}

/// Mount a seed page linking to one live page and one 404. The bodies clear
/// the minimum-content guard so the live page is exported.
async fn mount_site_with_missing_page(server: &MockServer) {
    Mock::given(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Seed</h1>\
             <p>The seed page carries enough substantive text to clear the minimum content guard.</p>\
             <a href=\"/page-1\">live</a><a href=\"/missing\">missing</a>\
             </article></body></html>",
        ))
        .mount(server)
        .await;
    Mock::given(path("/page-1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Live</h1>\
             <p>The live page carries enough substantive text to clear the minimum content guard.</p>\
             </article></body></html>",
        ))
        .mount(server)
        .await;
    Mock::given(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
}

/// A 404-class outcome is a terminal verdict: the run records it as settled,
/// the checkpoint carries it, and a later `--resume` does not fetch it again.
///
/// This is the half of #3079 the old `visited`-only checkpoint broke in the
/// OTHER direction: without a terminal marker, a resume re-drove every 404 /
/// filtered / unsupported URL as if it could still produce a page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(
    miri,
    ignore = "btls-sys FFI (wreq Client via session_engine) not supported by Miri: TLS_method"
)]
async fn terminal_404_is_settled_and_not_redriven_by_resume() {
    use crate::application::crawler::checkpoint::CheckpointStore;

    let _entry_off = entry_off();
    let server = MockServer::start().await;
    mount_site_with_missing_page(&server).await;

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let port = server.address().port();
    let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
    let missing = format!("http://127.0.0.1:{port}/missing");
    let config = || {
        CrawlerConfig::builder(seed.clone())
            .max_depth(1)
            .max_pages(10)
            .delay_ms(1)
            .timeout_secs(5)
            .ignore_robots(true)
            .build()
    };

    // Leg 1: the 404 completes as a terminal failure of the real task path.
    let mut engine = checkpoint_session_engine(config(), tmp.path(), 1000);
    let result = engine.run().await.expect("crawl must complete");
    assert_eq!(result.errors, 1, "the 404 is the run's only error");
    assert!(
        engine.scheduler.snapshot_settled().contains(&missing),
        "a 404 must be recorded as a terminal verdict"
    );

    // The completed run deletes its checkpoint; persist the state it decided
    // so leg 2 resumes from exactly what the run knew.
    engine.save_checkpoint().await;
    let file = checkpoint_file_for(tmp.path(), &seed);
    let stored = BincodeCheckpoint::new()
        .load(&file)
        .expect("leg-1 state must persist");
    assert!(
        stored.settled.contains(&missing),
        "the settled verdict must reach the checkpoint payload"
    );

    // Leg 2: the resume follows the terminal verdict instead of retrying it.
    let mut resumed = checkpoint_session_engine(config(), tmp.path(), 1000);
    let _ = resumed.run().await.expect("resume must complete");
    let fetches = server
        .received_requests()
        .await
        .expect("request log")
        .iter()
        .filter(|r| r.url.path() == "/missing")
        .count();
    assert_eq!(
        fetches, 1,
        "a terminal 404 must be fetched exactly once across the resume"
    );
}
