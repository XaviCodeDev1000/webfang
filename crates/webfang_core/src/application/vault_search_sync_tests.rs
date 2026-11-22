use super::tests_support::*;
use super::*;
use crate::domain::note_repository::VaultNote;
use crate::domain::Sha256Hex;

/// #1104 — `sync_vault` must run the vault walk on the blocking pool.
///
/// On a current-thread runtime a synchronous walk parks the only worker:
/// the 100 ms timer cannot fire until the 500 ms walk ends, so the timer
/// timestamp lands at >=500 ms (pre-fix behaviour). With `spawn_blocking`
/// the walk yields the reactor and the timer fires at ~100 ms. The 400 ms
/// threshold separates the two modes with a wide margin.
#[tokio::test]
async fn sync_vault_runs_walk_off_the_executor() {
    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = VaultSearchService::new(
        Arc::new(StubEmbedding),
        repo,
        Arc::new(StubChunker),
        Arc::new(SlowNoteReader),
    );
    let tmp = tempfile::tempdir().unwrap();

    let start = std::time::Instant::now();
    let mut timer_at: Option<std::time::Duration> = None;
    let ((), summary) = tokio::join!(
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            timer_at = Some(start.elapsed());
        },
        service.sync_vault(tmp.path()),
    );
    summary.expect("slow walk must still succeed");

    let timer_at = timer_at.expect("timer must have fired");
    assert!(
        timer_at < std::time::Duration::from_millis(400),
        "#1104: the 100ms timer fired at {timer_at:?} — a synchronous walk \
         on the executor would delay it past the 500ms read, freezing a \
         worker for the whole vault"
    );
}

#[tokio::test]
async fn sync_vault_indexes_new_notes() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_vault(
        tmp.path(),
        &[("a.md", "# A\nContent A"), ("b.md", "# B\nContent B")],
    );

    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = test_service(repo.clone());

    let summary = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(summary.indexed, 2);
    assert_eq!(summary.updated, 0);
    assert_eq!(summary.deleted, 0);
    assert_eq!(summary.unchanged, 0);

    // Verify notes are in the repo.
    let indexed = repo.list_indexed_notes().await.unwrap();
    assert_eq!(indexed.len(), 2);
}

#[tokio::test]
async fn sync_vault_skips_unchanged_notes() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_vault(tmp.path(), &[("a.md", "# A\nContent A")]);

    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = test_service(repo.clone());

    // First sync: indexes the note.
    let s1 = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(s1.indexed, 1);

    // Second sync: note unchanged.
    let s2 = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(s2.indexed, 0);
    assert_eq!(s2.updated, 0);
    assert_eq!(s2.unchanged, 1);
}

#[tokio::test]
async fn sync_vault_detects_changed_notes() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_vault(tmp.path(), &[("a.md", "# A\nOriginal")]);

    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = test_service(repo.clone());

    // First sync.
    let s1 = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(s1.indexed, 1);

    // Modify the note.
    std::fs::write(tmp.path().join("a.md"), "# A\nModified content").unwrap();

    // Second sync: detects change.
    let s2 = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(s2.indexed, 0);
    assert_eq!(s2.updated, 1);
    assert_eq!(s2.unchanged, 0);
}

#[tokio::test]
async fn sync_vault_deletes_removed_notes() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_vault(
        tmp.path(),
        &[("a.md", "# A"), ("b.md", "# B"), ("c.md", "# C")],
    );

    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = test_service(repo.clone());

    // First sync: 3 notes.
    let s1 = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(s1.indexed, 3);

    // Delete one note.
    std::fs::remove_file(tmp.path().join("b.md")).unwrap();

    // Second sync: detects deletion.
    let s2 = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(s2.deleted, 1);
    assert_eq!(s2.unchanged, 2);

    // Verify only 2 notes remain.
    let indexed = repo.list_indexed_notes().await.unwrap();
    assert_eq!(indexed.len(), 2);
}

#[tokio::test]
async fn sync_vault_empty_vault() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_vault(tmp.path(), &[]);

    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = test_service(repo);

    let summary = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(summary, SyncSummary::default());
}

#[tokio::test]
async fn sync_vault_mixed_operations() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_vault(
        tmp.path(),
        &[
            ("keep.md", "# Keep"),
            ("change.md", "# Old"),
            ("delete.md", "# Gone"),
        ],
    );

    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = test_service(repo.clone());

    // First sync: 3 notes.
    service.sync_vault(tmp.path()).await.unwrap();

    // Modify one, delete one, add one.
    std::fs::write(tmp.path().join("change.md"), "# New content").unwrap();
    std::fs::remove_file(tmp.path().join("delete.md")).unwrap();
    std::fs::write(tmp.path().join("new.md"), "# Brand new").unwrap();

    let summary = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(summary.indexed, 1, "new.md");
    assert_eq!(summary.updated, 1, "change.md");
    assert_eq!(summary.deleted, 1, "delete.md");
    assert_eq!(summary.unchanged, 1, "keep.md");
}

#[tokio::test]
async fn sync_vault_reads_notes_through_injected_port() {
    // On-disk vault is EMPTY: any indexed note must come from the stub,
    // proving the service reads through `VaultNoteReader` (#1071).
    let tmp = tempfile::tempdir().unwrap();
    create_test_vault(tmp.path(), &[]);

    let repo = Arc::new(InMemoryNoteRepo::default());
    let service = VaultSearchService::new(
        Arc::new(StubEmbedding),
        repo.clone(),
        Arc::new(StubChunker),
        Arc::new(CannedNoteReader {
            notes: vec![VaultNote {
                path: "stub.md".to_owned(),
                content: "# Stub".to_owned(),
                mtime_secs: 1_700_000_000,
                content_hash: Sha256Hex::from_digest([0x5eu8; 32]),
            }],
        }),
    );

    let summary = service.sync_vault(tmp.path()).await.unwrap();
    assert_eq!(summary.indexed, 1, "the stub note must be indexed");

    let indexed = repo.list_indexed_notes().await.unwrap();
    assert_eq!(indexed.len(), 1);
    assert_eq!(indexed[0].path, "stub.md");
    assert_eq!(
        indexed[0].content_hash,
        Sha256Hex::from_digest([0x5eu8; 32])
    );
}

#[tokio::test]
async fn sync_vault_propagates_reader_io_error() {
    // A valid on-disk vault must NOT mask the stub's failure: the port
    // error must bubble up unchanged from step 1 of the pipeline.
    let tmp = tempfile::tempdir().unwrap();
    create_test_vault(tmp.path(), &[("a.md", "# A")]);

    let service = VaultSearchService::new(
        Arc::new(StubEmbedding),
        Arc::new(InMemoryNoteRepo::default()),
        Arc::new(StubChunker),
        Arc::new(FailingNoteReader),
    );

    let err = service.sync_vault(tmp.path()).await.unwrap_err();
    assert!(
        matches!(err, ScraperError::Io(_)),
        "reader failure must surface as ScraperError::Io, got {err:?}"
    );
}
