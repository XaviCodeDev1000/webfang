//! Resume/cancellation tests: SC7/E8 drain ordering and P8-6 (ADR-0016
//! §4) failed-record re-drive semantics.

use super::tests_common::*;
use super::*;
use crate::application::resume::canonical_key;
use crate::domain::page_state::PageStatus;
use crate::domain::persistence::{DomainRecords, LastError, RawRecord};
use crate::infrastructure::export::RecordStore;
use tempfile::TempDir;

#[test]
fn test_process_results_resume_mode_tracks_urls() {
    let temp_dir = TempDir::new().unwrap();
    let content = make_scraped_content("https://example.com/page", "Page", "Body");

    let processed = export_urls(&[content], &temp_dir, ExportFormat::Jsonl);

    assert_eq!(processed.len(), 1);
}

// =========================================================================
// SC7 / E8 - cooperative cancellation: drain-before-final-persist ordering
// =========================================================================

#[test]
fn cancellation_drains_in_flight_item_and_persists_honest_statuses() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("cancel.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();

    // Cancel deterministically at the 2nd EXPORTED checkpoint save: item1
    // finishes its full D3 sequence, item2 reaches its flush-proof point,
    // THEN cancellation is observed between items and the run drains.
    let token = tokio_util::sync::CancellationToken::new();
    let export_count = std::sync::atomic::AtomicUsize::new(0);
    let token_ref = &token;
    let counter_ref = &export_count;
    let observer = move |status: PageStatus| {
        if status == PageStatus::Exported
            && counter_ref.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
        {
            token_ref.cancel();
        }
    };
    let ctx = ResumeContext::new(&ctx_store)
        .with_resume(false)
        .observing_cancel(&token)
        .with_persist_observer(&observer);

    let results = [
        make_scraped_content("https://cancel.test/one", "One", "first distinct body"),
        make_scraped_content("https://cancel.test/two", "Two", "second distinct body"),
        make_scraped_content("https://cancel.test/three", "Three", "third distinct body"),
    ];
    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .unwrap();

    // Drain ordering: items 1-2 completed their ATOMIC D3 sequence
    // (in-flight work never half-done); item 3 never started.
    let records = store.load().unwrap();
    assert_eq!(
        records.len(),
        2,
        "drained run persists exactly the in-flight items"
    );
    for url in ["https://cancel.test/one", "https://cancel.test/two"] {
        let r = &records[url];
        assert_eq!(
            r.status,
            PageStatus::Committed,
            "{url} must be fully committed"
        );
        assert!(r.last_error.is_none(), "{url}: no phantom errors");
    }
    assert!(
        !records.contains_key("https://cancel.test/three"),
        "item after cancellation must NOT exist (no invented state)"
    );
    let lines = std::fs::read_to_string(dir.path().join("export.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    assert_eq!(lines, 2, "exactly the drained items' lines on disk");
}

#[test]
fn resume_after_cancellation_re_drives_non_committed_exactly_once() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("cancel2.test").with_state_dir(dir.path().to_path_buf());

    // Cancelled first run: only item 'a' completes its D3 sequence.
    let token = tokio_util::sync::CancellationToken::new();
    let ctx_store_a = store.clone();
    {
        let token_ref = &token;
        let observer = move |status: PageStatus| {
            if status == PageStatus::Committed {
                token_ref.cancel();
            }
        };
        let ctx = ResumeContext::new(&ctx_store_a)
            .with_resume(false)
            .observing_cancel(&token)
            .with_persist_observer(&observer);
        process_results(
            &[
                make_scraped_content("https://cancel2.test/a", "A", "alpha content"),
                make_scraped_content("https://cancel2.test/b", "B", "beta content"),
            ],
            dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            Some(&ctx),
        )
        .unwrap();
    }
    let lines_after_cancel = std::fs::read_to_string(dir.path().join("export.jsonl"))
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(
        lines_after_cancel, 1,
        "cancelled run exported exactly one line"
    );

    // Resume rerun: Committed 'a' skips; 'b' re-drives fresh - once.
    let ctx_store_b = store.clone();
    let second = ResumeContext::new(&ctx_store_b).with_resume(true);
    process_results(
        &[
            make_scraped_content("https://cancel2.test/a", "A", "alpha content"),
            make_scraped_content("https://cancel2.test/b", "B", "beta content"),
        ],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&second),
    )
    .unwrap();

    let records = store.load().unwrap();
    for url in ["https://cancel2.test/a", "https://cancel2.test/b"] {
        assert_eq!(records[url].status, PageStatus::Committed, "{url}");
    }
    let lines_final = std::fs::read_to_string(dir.path().join("export.jsonl"))
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(lines_final, 2, "no duplicated lines across cancel+resume");
}

// =========================================================================
// P8-6 (ADR-0016 §4): failed records re-drive; the classified error
// survives crash + resume and is cleared on success.
// =========================================================================

/// A page that failed a previous run persists its classified error at a
/// NON-advanced status (SC6 / `fail_item`). Decision (a) of #1292: resume
/// RE-DRIVES the failed page — no permanent `Failed` status — and success
/// clears the error and commits. The record's truth (attempts, error class)
/// survives across runs.
#[test]
fn p86_failed_record_is_redriven_and_error_cleared_on_success() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("p86.test").with_state_dir(dir.path().to_path_buf());
    let url = "https://p86.test/a";
    let key = canonical_key(url);

    // Seed: the previous run recorded a classified failure at a
    // non-advanced status (the SC6 shape `fail_item` persists).
    let mut seeded = DomainRecords::new();
    seeded.insert(
        key.clone(),
        RawRecord {
            url: url.to_string(),
            canonical_url: key.clone(),
            run_id: "previous-run".to_string(),
            content_hash: None,
            attempts: 2,
            status: PageStatus::Fetched,
            last_error: Some(LastError {
                class: crate::domain::error::ErrorClass::TransientBackoff,
                message: "429 rate-limited; retry with backoff".to_string(),
            }),
            output_location: None,
            updated_at: 1_760_000_000_000,
        },
    );
    store.save(&seeded).unwrap();

    // Resume rerun: the failed record is not committed-proven, so the
    // gate re-drives it and success commits it, clearing the error.
    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(true);
    let processed = process_results(
        &[make_scraped_content(url, "A", "alpha content")],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .unwrap()
    .processed_urls;
    assert_eq!(processed.len(), 1, "the failed record must be re-driven");

    let records = store.load().unwrap();
    let record = &records[&key];
    assert_eq!(record.status, PageStatus::Committed, "re-driven to commit");
    assert!(
        record.last_error.is_none(),
        "success clears last_error (SC6)"
    );
    assert!(record.attempts >= 3, "attempts keep counting across runs");
}

/// The re-drive decision is honest in BOTH directions: a page that fails
/// AGAIN keeps its classified error at a non-advanced status (never a
/// silent skip, never a fake commit), and the next resume re-drives it
/// once more until it succeeds.
#[test]
fn p86_repeated_failure_stays_nonadvanced_and_redrives_again() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("p86b.test").with_state_dir(dir.path().to_path_buf());
    let url = "https://p86b.test/bad";
    let key = canonical_key(url);

    // Run 1: invalid (empty) content fails validation — SC6 records the
    // classified failure and the run continues.
    let ctx_store_1 = store.clone();
    let ctx_1 = ResumeContext::new(&ctx_store_1).with_resume(true);
    process_results(
        &[make_scraped_content(url, "Bad", "")],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx_1),
    )
    .unwrap();

    let records = store.load().unwrap();
    let failed = &records[&key];
    assert_ne!(
        failed.status,
        PageStatus::Committed,
        "failure never commits"
    );
    let first_error = failed
        .last_error
        .as_ref()
        .expect("classified failure must be persisted (SC6)");
    assert_eq!(
        first_error.class,
        crate::domain::error::ErrorClass::DomainRecoverable,
        "item-data problems are DomainRecoverable"
    );

    // Run 2 (--resume): the failed page re-drives; good content commits
    // and clears the stale error.
    let ctx_store_2 = store.clone();
    let ctx_2 = ResumeContext::new(&ctx_store_2).with_resume(true);
    process_results(
        &[make_scraped_content(
            url,
            "Bad",
            "now with real content that passes",
        )],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx_2),
    )
    .unwrap();

    let records = store.load().unwrap();
    let recovered = &records[&key];
    assert_eq!(recovered.status, PageStatus::Committed);
    assert!(recovered.last_error.is_none(), "success clears last_error");
}

// =========================================================================
// #1812: the export path's data integrity
// =========================================================================
