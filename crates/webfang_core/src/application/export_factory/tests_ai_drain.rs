//! #2269 — the AI drain branch must be EXECUTED, not merely present:
//! cancelled-drain semantics through `process_results_with_chunks`.

use super::tests_ai_chunks::{jsonl_lines, make_ai_chunk};
use super::*;
use crate::domain::page_state::PageStatus;
use crate::infrastructure::export::RecordStore;
use tempfile::TempDir;

// =====================================================================
// #2269 — the AI drain branch must be EXECUTED, not merely present.
// =====================================================================

/// A token already cancelled when the export starts commits the FIRST
/// chunk (the in-flight item completes its whole sequence) and leaves
/// every later chunk untouched: no invented records, no torn line, no
/// duplicated hash.
#[test]
fn ai_cancelled_drain_commits_the_first_chunk_and_leaves_the_rest_untouched() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("drain-ai.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();

    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let ctx = ResumeContext::new(&ctx_store)
        .with_resume(false)
        .observing_cancel(&token);

    let chunks = [
        make_ai_chunk("https://drain-ai.test/one", "One", "first distinct body"),
        make_ai_chunk("https://drain-ai.test/two", "Two", "second distinct body"),
        make_ai_chunk(
            "https://drain-ai.test/three",
            "Three",
            "third distinct body",
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

    // Drain ordering: the first chunk finished its full D3 sequence;
    // the ones after the cancellation was observed never started.
    let records = store.load().unwrap();
    assert_eq!(
        records.len(),
        1,
        "the drain persists exactly the in-flight chunk"
    );
    let first = &records["https://drain-ai.test/one"];
    assert_eq!(first.status, PageStatus::Committed);
    assert!(first.content_hash.is_some(), "committed requires a hash");
    assert!(
        first.output_location.is_some(),
        "committed requires the output location"
    );
    assert!(first.last_error.is_none(), "committed carries no error");
    for url in ["https://drain-ai.test/two", "https://drain-ai.test/three"] {
        assert!(
            !records.contains_key(url),
            "{url} must NOT exist after the drain (no invented state)"
        );
    }

    // Nothing half-written: the batch is flushed as a whole before the
    // commit loop, so every planned line is complete, parses, and
    // appears exactly once.
    let body = std::fs::read_to_string(dir.path().join("export.jsonl")).unwrap();
    assert!(
        body.ends_with('\n'),
        "the drain must leave a newline-terminated file"
    );
    let lines = jsonl_lines(dir.path(), "export");
    assert_eq!(
        lines.len(),
        chunks.len(),
        "the batch write flushes the planned chunks; the drain governs COMMITS"
    );
    let mut hashes: Vec<String> = lines
        .iter()
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).expect("every line is JSON");
            value["checksum_sha256"]
                .as_str()
                .expect("line carries checksum_sha256")
                .to_string()
        })
        .collect();
    let unique = {
        let mut copy = hashes.clone();
        copy.sort();
        copy.dedup();
        copy
    };
    hashes.sort();
    assert_eq!(hashes, unique, "no chunk may be written twice");
}

/// The mirrored cavity (#2269): when EVERY chunk is AlreadyCommitted
/// the resume gate skips each one BEFORE any cancellation check, so the
/// run drains with nothing to drive. That path must stay honest — no
/// record re-driven, no line re-appended, no attempt invented.
#[test]
fn ai_cancelled_drain_with_every_chunk_already_committed_invents_nothing() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("drain-ai-cavity.test").with_state_dir(dir.path().to_path_buf());
    let chunks = [
        make_ai_chunk("https://cavity-ai.test/a", "A", "alpha body"),
        make_ai_chunk("https://cavity-ai.test/b", "B", "beta body"),
    ];

    // Seed: a normal fresh run commits both chunks and writes both
    // lines — the state a crashed-and-resumed run leaves behind.
    let seed_store = store.clone();
    let seed = ResumeContext::new(&seed_store).with_resume(false);
    process_results_with_chunks(
        &chunks,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&seed),
    )
    .unwrap();
    let seeded = store.load().unwrap();
    assert_eq!(seeded.len(), 2, "seed run commits both chunks");
    assert_eq!(jsonl_lines(dir.path(), "export").len(), 2);

    // Resume with the token ALREADY cancelled: every chunk is skipped
    // by the gate, so the loop reaches neither a drive nor the
    // cancellation arm. The drain must still be a no-op, not a rewrite.
    let resume_store = store.clone();
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let resume = ResumeContext::new(&resume_store)
        .with_resume(true)
        .observing_cancel(&token);
    process_results_with_chunks(
        &chunks,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&resume),
    )
    .unwrap();

    let after = store.load().unwrap();
    assert_eq!(after.len(), 2, "no record may be invented");
    for url in ["https://cavity-ai.test/a", "https://cavity-ai.test/b"] {
        assert_eq!(after[url].status, PageStatus::Committed, "{url}");
        assert_eq!(
            after[url].attempts, seeded[url].attempts,
            "{url}: a skipped chunk must not be re-driven"
        );
    }
    assert_eq!(
        jsonl_lines(dir.path(), "export").len(),
        2,
        "a fully committed resume must not re-append"
    );
}
