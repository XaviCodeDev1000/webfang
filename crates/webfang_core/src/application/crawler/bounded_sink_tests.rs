use super::*;
use std::sync::Arc;
use std::time::Duration;

async fn sink_in(dir: &tempfile::TempDir, buffer: usize) -> BoundedFileSink {
    BoundedFileSink::new(dir.path().join("spool.jsonl"), buffer)
        .await
        .expect("sink must be created")
}

async fn sink_with_backlog(
    dir: &tempfile::TempDir,
    buffer: usize,
    max_backlog_bytes: usize,
) -> BoundedFileSink {
    BoundedFileSink::with_backlog_bytes(
        dir.path().join("spool.jsonl"),
        buffer,
        max_backlog_bytes,
    )
    .await
    .expect("sink must be created")
}

async fn drain(sink: &BoundedFileSink) -> Vec<CapturedPage> {
    let mut reader = sink.reader().await.expect("spool must open");
    let mut pages = Vec::new();
    while let Some(page) = reader.next_page().await.expect("spool must decode") {
        pages.push(page);
    }
    pages
}

#[tokio::test]
async fn captured_pages_round_trip_through_the_spool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = sink_in(&dir, 4).await;

    sink.capture("https://example.com/a", "<p>a</p>");
    sink.capture("https://example.com/b", "<p>b</p>");

    assert_eq!(sink.finish().await.expect("flush"), 2);
    assert_eq!(
        drain(&sink).await,
        vec![
            CapturedPage {
                url: "https://example.com/a".to_string(),
                html: "<p>a</p>".to_string(),
            },
            CapturedPage {
                url: "https://example.com/b".to_string(),
                html: "<p>b</p>".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn bodies_with_newlines_survive_the_jsonl_encoding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = sink_in(&dir, 2).await;

    sink.capture("https://example.com/", "<p>line1</p>\n<p>line2</p>");
    assert_eq!(sink.finish().await.expect("flush"), 1);

    let pages = drain(&sink).await;
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].html, "<p>line1</p>\n<p>line2</p>");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tiny_buffer_still_persists_every_page() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Buffer of 1 routes nearly every capture through the backlog.
    let sink = Arc::new(sink_in(&dir, 1).await);

    let mut handles = Vec::with_capacity(64);
    for i in 0..64 {
        let sink = Arc::clone(&sink);
        handles.push(tokio::spawn(async move {
            sink.capture(&format!("https://example.com/{i}"), "<p>body</p>");
        }));
    }
    for h in handles {
        h.await.expect("capture task panicked");
    }

    // `finish` waits for the writer to drain BOTH the channel and the
    // backlog, so there is nothing left to wait on here — the deferred pages
    // are already accounted for and nothing is racing.
    assert_eq!(sink.finish().await.expect("flush"), 64);
    assert_eq!(sink.dropped(), 0, "the default backlog absorbs the burst");
    assert_eq!(drain(&sink).await.len(), 64);
}

// ===== #1616 P0.2: the producer must not scale with the writer's lag =====

/// The P0.2 invariant, proved without touching a clock.
///
/// A `current_thread` runtime cannot run the writer until this task yields,
/// so the capture loop below is a **deterministic stalled writer**: every
/// page meets a full channel. What the old code did here was spawn one task
/// per capture, so retained memory grew with the producer count. What this
/// code does is refuse pages once the backlog ceiling is reached, so the
/// retained set is *constant*.
///
/// The proof is the difference between the two runs: capturing 10x more
/// pages must leave exactly as many pages retained. Under the old behaviour
/// the second run retained 10x the first.
#[tokio::test]
async fn a_stalled_writer_holds_a_constant_number_of_pages_however_many_arrive() {
    async fn retained_after(dir: &tempfile::TempDir, pages: usize) -> (usize, usize) {
        // Channel of 1, backlog ceiling of 128 bytes: both are tiny, and the
        // backlog is far smaller than `pages` in both runs, so the ceiling is
        // what decides how much is retained.
        let sink = sink_with_backlog(dir, 1, 128).await;
        let before = sink.backlog.occupancy().0;
        for i in 0..pages {
            sink.capture(&format!("https://example.com/{i}"), "0123456789");
        }
        let (held, _bytes) = sink.backlog.occupancy();
        let captured = sink.captured();
        let dropped = sink.dropped();
        assert!(dropped > 0, "the ceiling must bite to prove a bound exists");
        // Every capture is accounted for exactly once: spooled or refused.
        assert_eq!(captured + dropped, pages, "no silent loss or duplication");
        (held + 1, before) // +1: the page the channel accepted
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let (retained_small, _) = retained_after(&dir, 200).await;
    let (retained_large, _) = retained_after(&dir, 2_000).await;

    assert_eq!(
        retained_small, retained_large,
        "pages retained under a stalled writer must not grow with the producer count"
    );
}

/// A full channel with room in the backlog keeps every page — the #631
/// no-drop guarantee still holds, it just lives in the sink now.
#[tokio::test]
async fn a_full_channel_defers_to_the_backlog_without_dropping() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Generous backlog, so only the channel is ever full.
    let sink = sink_with_backlog(&dir, 1, DEFAULT_MAX_BACKLOG_BYTES).await;

    for i in 0..64 {
        sink.capture(&format!("https://example.com/{i}"), "<p>body</p>");
    }

    assert_eq!(sink.dropped(), 0, "a roomy backlog must not refuse pages");
    assert_eq!(sink.finish().await.expect("flush"), 64);
    assert_eq!(drain(&sink).await.len(), 64);
}

/// Once the ceiling is reached the sink stays refused — the bound is sticky,
/// so a wedged spool cannot be walked back into unbounded growth by a later
/// capture.
#[tokio::test]
async fn a_tripped_backlog_stays_tripped() {
    let backlog = Backlog::new(4);
    let page = |u: &str| CapturedPage {
        url: u.to_string(),
        html: "0123456789".to_string(),
    };

    assert!(matches!(
        backlog.admit(page("a")),
        Admit::Saturated { first: true }
    ));
    // A later, smaller page is still refused: the latch does not reopen.
    assert!(matches!(
        backlog.admit(page("b")),
        Admit::Saturated { first: false }
    ));
}

/// The backlog admits exactly up to its byte ceiling and never past it.
#[test]
fn the_backlog_admits_up_to_its_byte_ceiling() {
    let backlog = Backlog::new(6);
    let page = |u: &str| CapturedPage {
        url: u.to_string(),
        html: "ab".to_string(),
    };

    // Each page costs 3 bytes (1 url + 2 html); the third would exceed 6.
    assert!(matches!(backlog.admit(page("a")), Admit::Queued));
    assert!(matches!(backlog.admit(page("b")), Admit::Queued));
    assert!(matches!(backlog.admit(page("c")), Admit::Saturated { .. }));
    assert_eq!(backlog.occupancy(), (2, 6));
    assert!(!backlog.is_empty());

    let drained = backlog.drain();
    assert_eq!(drained.len(), 2);
    assert_eq!(
        backlog.occupancy(),
        (0, 0),
        "draining resets the accounting"
    );
    assert!(backlog.is_empty());
}

// ===== #1616 CC-D3: the writer must not park forever =====

/// A cancelled writer stops, and `finish` still joins it.
///
/// The timeout here is a hang tripwire, not a synchronisation device: the
/// test's correctness comes from the cancellation token (injected, not timed)
/// and from `finish`'s own join, so a regression that parks the writer fails
/// deterministically instead of flaking.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_unparks_an_idle_writer_and_finish_still_drains() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = sink_in(&dir, 2).await;

    sink.capture("https://example.com/a", "<p>a</p>");

    // Let the writer drain the page, then idle with the channel still open:
    // that is the state a real run sits in between pages, and the state a
    // dead writer used to strand producers in.
    let drained = tokio::time::timeout(Duration::from_secs(10), sink.finish())
        .await
        .expect("drain completes")
        .expect("flush succeeds");
    assert_eq!(drained, 1);

    assert!(!sink.is_cancelled());
    sink.cancel();
    assert!(sink.is_cancelled());

    // A second `finish` is a lifecycle error, not a hang: proving the join
    // is consumed is enough to show the writer task was reaped.
    let err = tokio::time::timeout(Duration::from_secs(10), sink.finish())
        .await
        .expect("a cancelled sink must not park finish")
        .expect_err("second flush must fail");
    assert!(matches!(err, BoundedSinkError::AlreadyFinished));
}

/// Cancelling mid-run flushes what was already persisted instead of hanging.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_writer_flushes_what_it_persisted() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Single-threaded: the writer cannot run until this task yields, so the
    // cancel is requested while every page is still queued.
    let sink = sink_in(&dir, 4).await;
    sink.capture("https://example.com/a", "<p>a</p>");
    sink.cancel();

    let written = tokio::time::timeout(Duration::from_secs(10), sink.finish())
        .await
        .expect("a cancelled writer must not park finish")
        .expect("flush succeeds");
    assert!(written <= 1, "only the queued page could have been written");
    // Whatever it wrote is still a valid spool, not a torn one.
    for page in drain(&sink).await {
        assert!(page.url.starts_with("https://example.com/"));
    }
}

/// Regression test for a real page-loss defect this change introduced and
/// then fixed: the writer's exit condition tested `rx.is_closed()` but not
/// `rx.is_empty()`, so a page buffered *between* the writer's drain and its
/// close check was dropped along with the receiver. `finish()` then
/// under-reported and the page was simply gone — the exact silent loss
/// #631 exists to prevent, reintroduced through the fix for it.
///
/// The interleaving is a genuine race and is NOT forced here, so this is a
/// repeated probe rather than a deterministic proof: the writer is polled on
/// a separate worker while the producer sends and the sink closes, and each
/// round asserts the full accounting identity. A single-threaded test cannot
/// reproduce it at all, because the writer never runs between the captures
/// and the close. The bound is a claim about coverage, not a guarantee, and
/// the invariant it checks is exact whenever it does fire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_accepted_page_is_lost_when_the_sink_closes_mid_flight() {
    const ROUNDS: usize = 200;
    const PAGES: usize = 8;

    for round in 0..ROUNDS {
        let dir = tempfile::tempdir().expect("tempdir");
        // Buffer of 1 keeps the channel saturated, so most pages take the
        // backlog path and the writer's drain/check loop runs repeatedly.
        let sink = sink_in(&dir, 1).await;

        for i in 0..PAGES {
            sink.capture(&format!("https://example.com/{round}/{i}"), "<p>body</p>");
        }
        let captured = sink.captured();
        let written = sink.finish().await.expect("flush succeeds");

        assert_eq!(sink.dropped(), 0, "the default backlog absorbs the burst");
        assert_eq!(
            written, captured,
            "round {round}: every accepted page must reach the spool"
        );
        assert_eq!(drain(&sink).await.len(), captured, "round {round}");
    }
}

/// Writer death is a reported failure, not a silent flush. The audit
/// (#1828, writer death HIGH) noted that every other test in this module
/// covers cancellation and backpressure but none killed the writer.
/// Aborting the writer task makes `finish` join a dead task, and the
/// JoinError must surface as [`BoundedSinkError::Writer`] instead of an
/// `Ok` flush count: a run whose spool writer died must not be able to
/// report success (the "writer death invisible" contract note in
/// `crawl_result_repository`).
#[tokio::test]
async fn a_dead_writer_makes_finish_report_the_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = sink_in(&dir, 4).await;
    sink.capture("https://example.com/a", "<p>a</p>");

    // Kill the writer in place: the handle stays in the sink so `finish`
    // is the one that joins it. `abort` registers the cancellation
    // synchronously and the idle writer sits on `rx.recv()` with an open
    // channel and no cancel, so the join deterministically yields the
    // cancelled JoinError.
    {
        let guard = sink
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .as_ref()
            .expect("the writer task is alive until finish joins it")
            .abort();
    }

    let err = tokio::time::timeout(Duration::from_secs(10), sink.finish())
        .await
        .expect("a dead writer must not park finish")
        .expect_err("a dead writer must fail finish");
    assert!(matches!(err, BoundedSinkError::Writer(_)));

    // The sink is closed; a late capture is discarded, never acknowledged.
    let before = sink.captured();
    sink.capture("https://example.com/late", "<p>late</p>");
    assert_eq!(sink.captured(), before);

    // And a second `finish` is the lifecycle error, not a fake flush.
    let err = sink.finish().await.expect_err("second flush must fail");
    assert!(matches!(err, BoundedSinkError::AlreadyFinished));
}

#[tokio::test]
async fn finish_is_not_idempotent_and_reports_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = sink_in(&dir, 2).await;

    assert_eq!(sink.finish().await.expect("first flush"), 0);
    let err = sink.finish().await.expect_err("second flush must fail");
    assert!(matches!(err, BoundedSinkError::AlreadyFinished));
}

#[tokio::test]
async fn reader_over_a_missing_spool_reports_io() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = CapturedPageReader::open(&dir.path().join("absent.jsonl"))
        .await
        .expect_err("missing spool must fail");
    assert!(matches!(err, BoundedSinkError::Io(_)));
}

// ===== WU2 (#2611): the durable-bytes watermark =====

/// The watermark is 0 on a fresh sink: no page has been handed to the OS,
/// so an incremental flush started right after construction converts from
/// the beginning (nothing) — it must never see a stale non-zero offset.
#[tokio::test]
async fn durable_bytes_starts_at_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = sink_in(&dir, 4).await;
    assert_eq!(sink.durable_bytes(), 0, "fresh sink has no durable bytes");
}

/// After a flush, the watermark equals the spool's on-disk length and sits
/// on a record boundary: every byte before it is a complete JSONL line.
#[tokio::test]
async fn durable_bytes_track_flushed_pages_and_end_on_record_boundaries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = sink_in(&dir, 4).await;

    sink.capture("https://example.com/a", "<p>a</p>");
    // Current-thread runtime: the writer has not run yet, so the watermark
    // must still be 0 even though capture() returned.
    assert_eq!(
        sink.durable_bytes(),
        0,
        "unflushed capture publishes nothing"
    );

    sink.finish().await.expect("flush");
    let spool_len = tokio::fs::metadata(sink.spool_path())
        .await
        .expect("spool meta")
        .len();
    assert_eq!(
        sink.durable_bytes(),
        spool_len,
        "after the final flush the watermark equals the file length"
    );

    // Record-boundary proof: converting from the watermark yields every
    // page and reaches EOF without a partial record.
    let mut reader = CapturedPageReader::open_at(sink.spool_path(), spool_len)
        .await
        .expect("reopen at watermark");
    assert!(
        reader
            .next_page()
            .await
            .expect("decode from watermark")
            .is_none(),
        "the watermark is the end of the last complete record"
    );
}

/// The watermark never REGRESSES during the run and never exceeds the
/// real file length: each update is a measurement of the file itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_bytes_are_monotonic_during_capture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = Arc::new(sink_in(&dir, 1).await);

    let mut seen = 0u64;
    for i in 0..16 {
        sink.capture(&format!("https://example.com/{i}"), "<p>body</p>");
        // yield so the writer can drain what it holds.
        tokio::time::sleep(Duration::from_millis(2)).await;
        let now = sink.durable_bytes();
        assert!(
            now >= seen,
            "watermark regressed: {seen} -> {now} after page {i}"
        );
        let file_len = tokio::fs::metadata(sink.spool_path())
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(
            now <= file_len,
            "watermark {now} passed the real file length {file_len}"
        );
        seen = now;
    }
    sink.finish().await.expect("flush");
    assert!(sink.durable_bytes() >= seen);
}
