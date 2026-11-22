use super::tests_support::*;
use super::*;
use crate::domain::note_repository::NoteChunkVector;
use crate::domain::Sha256Hex;

#[test]
fn test_cosine_similarity_identical() {
    let v = vec![1.0, 0.0, 0.0];
    let score = cosine_similarity_fail_closed(&v, &v).expect("same dims must score");
    assert!((score - 1.0).abs() < 1e-6, "identical vectors → 1.0");
}

#[test]
fn test_cosine_similarity_orthogonal() {
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![0.0, 1.0, 0.0];
    let score = cosine_similarity_fail_closed(&a, &b).expect("same dims must score");
    assert!(score.abs() < 1e-6, "orthogonal vectors → 0.0");
}

#[test]
fn test_cosine_similarity_opposite() {
    let a = vec![1.0, 0.0];
    let b = vec![-1.0, 0.0];
    let score = cosine_similarity_fail_closed(&a, &b).expect("same dims must score");
    assert!((score + 1.0).abs() < 1e-6, "opposite vectors → -1.0");
}

#[test]
fn test_cosine_similarity_empty() {
    assert_eq!(cosine_similarity_fail_closed(&[], &[]), Some(0.0));
}

#[test]
fn test_cosine_similarity_zero_vector() {
    let a = vec![0.0, 0.0, 0.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(
        cosine_similarity_fail_closed(&a, &b),
        Some(0.0),
        "zero vector → Some(0.0)"
    );
}

#[test]
fn test_cosine_similarity_dimension_mismatch() {
    let a = vec![1.0, 2.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(
        cosine_similarity_fail_closed(&a, &b),
        None,
        "mismatched dims → None (skip, never a silent 0.0)"
    );
}

#[test]
fn test_search_result_debug() {
    let result = VaultSearchResult {
        note_path: "vault/rust.md".to_owned(),
        content: "Rust is great".to_owned(),
        score: 0.95,
        chunk_index: 0,
        heading: Some("Introduction".to_owned()),
        spans: Vec::new(),
    };
    let debug = format!("{result:?}");
    assert!(debug.contains("vault/rust.md"));
    assert!(debug.contains("0.95"));
}

#[tokio::test]
async fn search_skips_dimension_mismatched_vectors() {
    // The stub embedder produces 3-dim query vectors: the 5-dim chunk
    // simulates a stale mixed-model index entry. It must be skipped —
    // never ranked with a silent 0.0.
    let repo = Arc::new(InMemoryNoteRepo::default());
    repo.chunks.lock().unwrap().extend([
        NoteChunkVector {
            note_path: "vault/ok.md".to_owned(),
            content: "matching model".to_owned(),
            chunk_index: 0,
            spans: Vec::new(),
            embedding: vec![0.1, 0.2, 0.3],
        },
        NoteChunkVector {
            note_path: "vault/stale.md".to_owned(),
            content: "other model".to_owned(),
            chunk_index: 0,
            spans: Vec::new(),
            embedding: vec![0.1, 0.2, 0.3, 0.4, 0.5],
        },
    ]);
    let service = test_service(repo);

    let results = service.search("query", 10).await.unwrap();
    assert_eq!(
        results.len(),
        1,
        "mismatched vector must be skipped, not ranked"
    );
    assert_eq!(results[0].note_path, "vault/ok.md");
}

#[tokio::test]
async fn search_wholesale_dim_mismatch_fails_closed_with_reindex_guidance() {
    // #1462: the whole index was built by another backend (e.g. vault
    // indexed at 384d now served by an 8d remote adapter) — no stored
    // vector matches the serving dim, so ranking would silently degrade.
    // Fail closed with re-index guidance instead of empty results.
    let repo = Arc::new(InMemoryNoteRepo::default());
    repo.chunks.lock().unwrap().extend([
        NoteChunkVector {
            note_path: "vault/a.md".to_owned(),
            content: "old model".to_owned(),
            chunk_index: 0,
            spans: Vec::new(),
            embedding: vec![0.1, 0.2, 0.3, 0.4, 0.5],
        },
        NoteChunkVector {
            note_path: "vault/b.md".to_owned(),
            content: "old model".to_owned(),
            chunk_index: 0,
            spans: Vec::new(),
            embedding: vec![0.5, 0.4, 0.3, 0.2, 0.1],
        },
    ]);
    let service = test_service(repo);

    let err = service
        .search("query", 10)
        .await
        .expect_err("wholesale dim mismatch must fail closed");
    let rendered = match err {
        ScraperError::Config(msg) => msg,
        other => panic!("wholesale mismatch must be Config, got: {other:?}"),
    };
    assert!(
        rendered.contains("re-indexe"),
        "mismatch must guide to re-index, got: {rendered}"
    );
    assert!(
        rendered.contains('5') && rendered.contains('3'),
        "mismatch must name stored and serving dims, got: {rendered}"
    );
}

#[tokio::test]
async fn search_results_carry_their_chunk_locators() {
    // Locator read-side: a match must expose the byte ranges of the note
    // it came from — what the index persists (#2451) is what the
    // consumer sees, not less.
    let repo = Arc::new(InMemoryNoteRepo::default());
    repo.chunks.lock().unwrap().push(NoteChunkVector {
        note_path: "vault/ok.md".to_owned(),
        content: "matching model".to_owned(),
        chunk_index: 0,
        spans: vec![ByteSpan::try_new(3, 17).expect("valid span")],
        embedding: vec![0.1, 0.2, 0.3],
    });
    let service = test_service(repo);

    let results = service.search("query", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].spans.len(),
        1,
        "the match must carry its persisted locators"
    );
    assert_eq!(results[0].spans[0].start(), 3);
    assert_eq!(results[0].spans[0].end(), 17);
}

#[test]
fn test_sync_summary_default() {
    let summary = SyncSummary::default();
    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.updated, 0);
    assert_eq!(summary.deleted, 0);
    assert_eq!(summary.unchanged, 0);
}

#[tokio::test]
async fn index_note_persists_chunk_locators() {
    // The locator contract at the service seam: whatever the chunker
    // located against the note content must reach the repository — the
    // index records, per chunk, which bytes of the note it came from.
    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = test_service(repo.clone());
    let content = "# Nota\n\nCuerpo de la nota.";
    let hash: String = "a".repeat(Sha256Hex::HEX_LEN);

    let indexed = service
        .index_note("notes/a.md", content, &hash, 0)
        .await
        .expect("index_note");
    assert_eq!(indexed, 2, "two non-blank lines, two chunks");

    let vectors = repo.load_all_vectors().await.expect("load vectors");
    assert_eq!(vectors.len(), 2);
    for vector in &vectors {
        assert_eq!(
            vector.spans.len(),
            1,
            "line chunks are single-span: {vector:?}"
        );
        let span = &vector.spans[0];
        assert_eq!(
            &content[span.start()..span.end()],
            vector.content,
            "each stored span must resolve against the note content"
        );
    }
}
