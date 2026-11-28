//! AI chunk export tests: resume gate before batch export (BUG F3-A),
//! #1812 dedup/commit honesty, #2269 drain branch.

use super::ai_plan::AiChunkOutcome;
use super::tests_common::RecordStore;
use super::*;
use crate::domain::entities::ScrapedContent;
use crate::domain::page_state::PageStatus;
use crate::domain::ValidUrl;
use tempfile::TempDir;

use crate::domain::DocumentChunkUnvalidated;
use std::collections::HashMap;

pub(super) fn make_ai_chunk(url: &str, title: &str, content: &str) -> crate::domain::DocumentChunk {
    DocumentChunkUnvalidated {
        id: uuid::Uuid::new_v4(),
        url: url.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        metadata: HashMap::new(),
        timestamp: chrono::Utc::now(),
        embeddings: None,
        correlation_id: None,
        _state: std::marker::PhantomData,
        chunk_spans: None,
    }
}

pub(super) fn jsonl_lines(dir: &std::path::Path, filename: &str) -> Vec<String> {
    std::fs::read_to_string(dir.join(format!("{filename}.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// RED (F3-A): with --clean-ai --resume, chunks of COMMITTED-proven
/// URLs must NOT be re-appended to the output JSONL.
#[test]
fn ai_resume_does_not_append_chunks_for_committed_urls() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("resume-ai.test").with_state_dir(dir.path().to_path_buf());

    // Seed run (fresh): urlA drives + commits; exactly one line.
    let seed_store = store.clone();
    let seed_ctx = ResumeContext::new(&seed_store).with_resume(false);
    process_results_with_chunks(
        &[make_ai_chunk(
            "https://resume-ai.test/a",
            "A",
            "alpha content",
        )],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&seed_ctx),
    )
    .unwrap();
    assert_eq!(jsonl_lines(dir.path(), "export").len(), 1);

    // Resume rerun: urlA is COMMITTED-proven (skip), urlB exports once.
    let resume_store = store.clone();
    let resume_ctx = ResumeContext::new(&resume_store).with_resume(true);
    let processed = process_results_with_chunks(
        &[
            make_ai_chunk("https://resume-ai.test/a", "A", "alpha content"),
            make_ai_chunk("https://resume-ai.test/b", "B", "beta content"),
        ],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&resume_ctx),
    )
    .unwrap()
    .processed_urls;
    assert_eq!(processed.len(), 2);

    let lines = jsonl_lines(dir.path(), "export");
    assert_eq!(
        lines.len(),
        2,
        "--clean-ai --resume must not duplicate chunks for COMMITTED-proven URLs"
    );
    assert_eq!(
        lines.iter().filter(|l| l.contains("alpha")).count(),
        1,
        "the committed-proven chunk appears exactly once"
    );

    let records = store.load().unwrap();
    for url in ["https://resume-ai.test/a", "https://resume-ai.test/b"] {
        assert_eq!(records[url].status, PageStatus::Committed, "{url}");
    }
}

/// Regression guard: a fresh-state first run still exports every chunk.
#[test]
fn ai_fresh_run_exports_all_chunks() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("fresh-ai.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(false);

    let processed = process_results_with_chunks(
        &[
            make_ai_chunk("https://fresh-ai.test/one", "One", "first body"),
            make_ai_chunk("https://fresh-ai.test/two", "Two", "second body"),
        ],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .unwrap()
    .processed_urls;

    assert_eq!(processed.len(), 2);
    assert_eq!(
        jsonl_lines(dir.path(), "export").len(),
        2,
        "fresh run exports every chunk"
    );
    let records = store.load().unwrap();
    assert_eq!(
        records["https://fresh-ai.test/one"].status,
        PageStatus::Committed
    );
    assert_eq!(
        records["https://fresh-ai.test/two"].status,
        PageStatus::Committed
    );
}

/// TRIANGULATE: PromoteFromFlushProof items must be promoted WITHOUT
/// re-appending their chunk (hash_index membership proves the bytes
/// were already flushed).
#[test]
fn ai_flush_proof_promotion_does_not_reappend_chunk() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("flush-ai.test").with_state_dir(dir.path().to_path_buf());

    // Run 1: AI-export commits gamma; one line on disk.
    let first_store = store.clone();
    let first = ResumeContext::new(&first_store).with_resume(false);
    process_results_with_chunks(
        &[make_ai_chunk(
            "https://flush-ai.test/g",
            "G",
            "gamma content",
        )],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&first),
    )
    .unwrap();
    assert_eq!(jsonl_lines(dir.path(), "export").len(), 1);

    // Demote the record to Extracted keeping its content_hash:
    // bytes proven on disk, but the record no longer claims COMMITTED.
    let mut records = store.load().unwrap();
    records
        .get_mut("https://flush-ai.test/g")
        .expect("gamma record exists")
        .status = PageStatus::Extracted;
    store.save(&records).unwrap();

    // Resume rerun: PromoteFromFlushProof promotes WITHOUT appending.
    let second_store = store.clone();
    let second = ResumeContext::new(&second_store).with_resume(true);
    process_results_with_chunks(
        &[make_ai_chunk(
            "https://flush-ai.test/g",
            "G",
            "gamma content",
        )],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&second),
    )
    .unwrap();

    assert_eq!(
        jsonl_lines(dir.path(), "export").len(),
        1,
        "flush-proof promotion must not re-append the chunk"
    );
    let records = store.load().unwrap();
    assert_eq!(
        records["https://flush-ai.test/g"].status,
        PageStatus::Committed,
        "record promoted back to Committed"
    );
}

/// #1812: the AI path must never commit a chunk it did not write.
///
/// A chunk whose title is empty cannot pass `validate()`, so it never
/// reaches the export file. The non-AI path (`process_single_item`)
/// records that failure and leaves the record at a non-advanced
/// status; the AI path used to drop the chunk SILENTLY and still mark
/// it COMMITTED — the resume gate then skipped the page forever,
/// although nothing was ever written to the file.
#[test]
fn ai_export_does_not_commit_a_chunk_it_never_wrote() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("ai-invalid.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(false);

    // An empty title fails `validate()` — the chunk never reaches
    // the file.
    process_results_with_chunks(
        &[make_ai_chunk(
            "https://ai-invalid.test/a",
            "",
            "content of the chunk that cannot be validated",
        )],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .unwrap();

    assert_eq!(
        jsonl_lines(dir.path(), "export").len(),
        0,
        "an unvalidatable chunk is never written"
    );
    let records = store.load().unwrap();
    let record = &records["https://ai-invalid.test/a"];
    assert_ne!(
        record.status,
        PageStatus::Committed,
        "a chunk that was never written must NOT be committed"
    );
    assert!(
        record.last_error.is_some(),
        "the failure must be recorded, not swallowed"
    );
}

/// The label of a planned outcome, for assertion messages.
fn outcome_label(outcome: &AiChunkOutcome) -> &'static str {
    match outcome {
        AiChunkOutcome::NotDriven => "NotDriven",
        AiChunkOutcome::Exported { .. } => "Exported",
        AiChunkOutcome::Duplicate => "Duplicate",
        AiChunkOutcome::Invalid(_) => "Invalid",
    }
}

/// #1812 correction: a chunk that did NOT validate must never claim
/// the `(url, content_hash)` dedup key.
///
/// The plan used to claim the key BEFORE validating, so an invalid
/// chunk masked a LATER VALID chunk with the same identity: the
/// export wrote nothing at all and the log said "duplicate ...
/// already exported this run" although nothing was ever exported —
/// silent data loss through the public API.
#[test]
fn an_invalid_chunk_does_not_claim_the_dedup_key() {
    let invalid = make_ai_chunk("https://ai-mask.test/a", "", "the shared body");
    let valid = make_ai_chunk(
        "https://ai-mask.test/a",
        "The valid title",
        "the shared body",
    );
    let chunks = vec![invalid, valid];
    let decisions = vec![ItemDecision::DriveAndCommit, ItemDecision::DriveAndCommit];
    // Same URL + same content hash ⇒ the same dedup identity.
    let hashes = vec!["same-content-hash".to_string(); 2];

    let (outcomes, validated) = plan_ai_chunks(&chunks, &decisions, &hashes);

    assert_eq!(
        outcome_label(&outcomes[0]),
        "Invalid",
        "the first chunk does not validate"
    );
    assert_eq!(
        outcome_label(&outcomes[1]),
        "Exported",
        "the valid chunk must not be masked as a duplicate"
    );
    assert_eq!(validated.len(), 1, "exactly one chunk reaches the export");
}

/// The same regression through the PUBLIC API: the valid chunk IS
/// written (one line), and the invalid one left a recorded failure.
#[test]
fn an_invalid_chunk_does_not_mask_a_later_valid_chunk_in_the_export() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("ai-mask.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(false);

    // Same URL and same content ⇒ the same (url, content_hash). The
    // FIRST chunk is invalid (empty title); the SECOND one is valid.
    let chunks = [
        make_ai_chunk(
            "https://ai-mask.test/a",
            "",
            "the shared body of both chunks",
        ),
        make_ai_chunk(
            "https://ai-mask.test/a",
            "The valid title",
            "the shared body of both chunks",
        ),
    ];

    process_results_with_chunks(
        &chunks,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .unwrap();

    let lines = jsonl_lines(dir.path(), "export");
    assert_eq!(
        lines.len(),
        1,
        "the valid chunk must be written: an invalid chunk may not mask it"
    );
    assert!(
        lines[0].contains("The valid title"),
        "the written line must be the VALID chunk: {lines:?}"
    );

    // The invalid chunk was recorded as a failed attempt (attempts
    // 1) and the valid one committed the URL (attempts 2) — the
    // failure is persisted, not swallowed.
    let records = store.load().unwrap();
    let record = &records["https://ai-mask.test/a"];
    assert_eq!(
        record.attempts, 2,
        "the invalid chunk was recorded as a failure before the valid one committed"
    );
}

/// PRESERVE (#1215): two IDENTICAL valid chunks in one run still
/// yield ONE line. The plan now validates BEFORE claiming the dedup
/// key (#1812), and that reorder must not weaken the dedup of the
/// chunks that DO reach the file.
#[test]
fn ai_identical_valid_chunks_are_still_deduped_within_the_run() {
    let dir = TempDir::new().unwrap();
    let chunks = [
        make_ai_chunk("https://ai-dup.test/a", "A", "the same body"),
        make_ai_chunk("https://ai-dup.test/a", "A", "the same body"),
    ];

    process_results_with_chunks(
        &chunks,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        None,
    )
    .unwrap();

    assert_eq!(
        jsonl_lines(dir.path(), "export").len(),
        1,
        "one (url, content_hash) pair must produce exactly one line"
    );
}

/// #1812 item 1: the page TITLE survives the `--clean-ai` export.
///
/// The AI chunk is enriched with the page identity before the export,
/// so the title reaches the record — the issue's premise that
/// `--clean-ai` loses it is false. The page-level `author`/`excerpt`
/// are NOT inherited: that is the archived #800 decision (a
/// page-inherited author on a fragment is a FALSE author, obs #162),
/// pinned here so it cannot drift silently in either direction.
#[test]
fn ai_export_keeps_the_page_title_and_does_not_inherit_the_page_author() {
    let dir = TempDir::new().unwrap();
    let page = ScrapedContent {
        title: "The page title".to_string(),
        content: "the page body".to_string(),
        url: ValidUrl::parse("https://ai-title.test/a").expect("valid url"),
        excerpt: Some("the page excerpt".to_string()),
        author: Some("the page author".to_string()),
        date: Some("2026-01-01".to_string()),
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };
    // What the cleaner produces: no identity, only the cleaned body.
    // The export path enriches it with the page identity.
    let chunk = DocumentChunkUnvalidated::new(uuid::Uuid::new_v4(), "", "", "the cleaned body")
        .enrich_from_scraped_content(&page);

    process_results_with_chunks(
        &[chunk],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        None,
    )
    .unwrap();

    let lines = jsonl_lines(dir.path(), "export");
    assert_eq!(lines.len(), 1);
    let record: serde_json::Value =
        serde_json::from_str(&lines[0]).expect("the exported line is JSON");
    assert_eq!(
        record["title"], "The page title",
        "the page title must survive the --clean-ai export"
    );
    let extra = record["extra_metadata"]
        .as_object()
        .expect("extra_metadata object");
    assert!(
        !extra.contains_key("author"),
        "the page author is NOT inherited by an AI chunk (#800)"
    );
    assert!(
        !extra.contains_key("excerpt"),
        "the page excerpt is NOT inherited by an AI chunk (#800)"
    );
}
