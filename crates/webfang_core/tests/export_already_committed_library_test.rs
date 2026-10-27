#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! `ItemDecision::AlreadyCommitted` as a LIBRARY-level guarantee (#2326,
//! decision A).
//!
//! The CLI never reaches this decision: `apply_resume_mode` drops every
//! COMMITTED URL before the crawl (`scrape_flow`), and a fully-committed
//! list short-circuits as "nothing pending" before the export phase runs
//! (`orchestrator::resume_nothing_pending`). The branch is therefore
//! deliberately NOT a row of the crash matrix; it is exercised here through
//! the public export port (`process_results` + `ResumeContext`), which any
//! library consumer can call directly.

#[path = "common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{committed_record, jsonl_lines, mock_scraped_content};
use tempfile::TempDir;
use webfang_core::application::export_factory::{process_results, ResumeContext};
use webfang_core::domain::page_state::PageStatus;
use webfang_core::infrastructure::export::RecordStore;
use webfang_core::ExportFormat;

#[test]
fn fully_committed_list_skips_without_reexport_and_stays_committed() {
    let url = "https://exp.test/already-done";
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("exp.test").with_state_dir(dir.path().to_path_buf());
    store
        .save(&BTreeMap::from([(url.to_string(), committed_record(url))]))
        .expect("seed committed record");

    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(true);
    let outcome = process_results(
        &[mock_scraped_content(
            url,
            "T",
            "fresh body for the same url",
        )],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .expect("export succeeds");

    assert!(
        outcome.processed_urls.is_empty(),
        "a COMMITTED-proven record is skipped, never re-driven"
    );
    assert_eq!(
        jsonl_lines(&dir),
        0,
        "the skip happens before the body read: no line is appended"
    );
    let after = store.load().unwrap();
    assert_eq!(after[url].status, PageStatus::Committed);
}

#[test]
fn mixed_batch_drives_only_the_fresh_item() {
    let committed = "https://exp.test/already-done";
    let fresh = "https://exp.test/never-seen";
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("exp.test").with_state_dir(dir.path().to_path_buf());
    store
        .save(&BTreeMap::from([(
            committed.to_string(),
            committed_record(committed),
        )]))
        .expect("seed committed record");

    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(true);
    let outcome = process_results(
        &[
            mock_scraped_content(committed, "T", "stale body"),
            mock_scraped_content(fresh, "T", "fresh body"),
        ],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .expect("export succeeds");

    assert_eq!(
        outcome.processed_urls,
        vec![fresh.to_string()],
        "only the item without a COMMITTED-proven record is driven"
    );
    assert_eq!(jsonl_lines(&dir), 1);
    let after = store.load().unwrap();
    assert_eq!(after[committed].status, PageStatus::Committed);
    assert_eq!(after[fresh].status, PageStatus::Committed);
}
