// Test policy (#1951): tokio::spawn in tests is deliberate harness
// concurrency — blanket exemption; production keeps per-site supervision.
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)
)]

//! Integration tests for SqliteVectorRepository — real I/O with temp dirs.
//!
//! Exercises CRUD operations, deduplication, vector storage, and schema
//! initialization per R-INT-01 and R-INT-03.

#![cfg(feature = "persistence")]

use tempfile::TempDir;
use webfang_core::domain::VectorRepository;
use webfang_core::infrastructure::persistence::pool::Object;
use webfang_core::infrastructure::persistence::sqlite::{
    create_memory_pool, create_pool, setup_schema, SqliteVectorRepository,
};

// ===== SCHEMA INITIALIZATION =====

/// Count rows in `resources` matching a content hash (test inspection only).
async fn count_resources_by_hash(repo: &SqliteVectorRepository, hash: &str) -> i64 {
    let hash = hash.to_string();
    let conn = repo.pool().get().await.expect("get");
    conn.interact(move |c| {
        c.query_row(
            "SELECT COUNT(*) FROM resources WHERE content_hash = ?1",
            [hash],
            |row| row.get::<_, i64>(0),
        )
    })
    .await
    .expect("interact")
    .expect("count")
}

/// Schema creation is idempotent — calling setup_schema twice doesn't fail.
#[tokio::test]
async fn test_setup_schema_is_idempotent() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    setup_schema(&pool).await.unwrap();
}

/// Schema creation on a temp file DB creates the file.
#[tokio::test]
async fn test_setup_schema_creates_db_file() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let pool = create_pool(&db_path, 1).unwrap();
    setup_schema(&pool).await.unwrap();
    assert!(
        db_path.exists(),
        "database file should exist after schema setup"
    );
}

// ===== RESOURCE CRUD =====

/// Save a resource and retrieve its URL by content hash (file-backed DB).
#[tokio::test]
async fn test_save_and_get_resource() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("save-resource.db");
    let pool = create_pool(&db_path, 4).unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let url = repo
        .save_resource("https://example.com/page1", "Page 1", "hash_abc", 1024)
        .await
        .unwrap();

    assert_eq!(url, "https://example.com/page1");

    let existing = repo.resource_exists_by_hash("hash_abc").await.unwrap();
    assert_eq!(existing, Some("https://example.com/page1".to_string()));
    assert_eq!(count_resources_by_hash(&repo, "hash_abc").await, 1);
}

/// resource_exists_by_hash returns None for unknown hash.
#[tokio::test]
async fn test_resource_not_exists() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let result = repo
        .resource_exists_by_hash("nonexistent_hash")
        .await
        .unwrap();
    assert_eq!(result, None);
}

// ===== DEDUPLICATION =====

/// Duplicate content hash returns existing URL without inserting a new row.
#[tokio::test]
async fn test_dedup_skips_duplicate_hash() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("dedup.db");
    let pool = create_pool(&db_path, 4).unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let url1 = repo
        .save_resource("https://a.com/page", "Page", "dedup_hash", 100)
        .await
        .unwrap();

    // Second save with same hash — should return existing URL, not error
    let url2 = repo
        .save_resource("https://b.com/page", "Page v2", "dedup_hash", 200)
        .await
        .unwrap();

    assert_eq!(url1, url2, "dedup should return the original URL");
    assert_eq!(
        count_resources_by_hash(&repo, "dedup_hash").await,
        1,
        "no duplicate row must be inserted on dedup hit"
    );
}

// ===== CHUNK CRUD =====

/// Save a chunk with embedding and retrieve the vector.
#[tokio::test]
async fn test_save_and_get_chunk_with_embedding() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    // Save a resource first (FK constraint)
    repo.save_resource("https://example.com/doc", "Doc", "h1", 100)
        .await
        .unwrap();

    let embedding = vec![0.1, 0.2, 0.3, -0.5];
    repo.save_chunk(
        "chunk-1",
        "https://example.com/doc",
        0,
        "Hello world",
        Some(&embedding),
        None,
    )
    .await
    .unwrap();

    let retrieved = repo.get_vector("chunk-1").await.unwrap();
    assert!(retrieved.is_some(), "should retrieve embedding");
    let vec = retrieved.unwrap();
    assert_eq!(vec.len(), 4);
    assert!((vec[0] - 0.1).abs() < 1e-6);
    assert!((vec[3] - (-0.5)).abs() < 1e-6);
}

/// Save a chunk without embedding — get_vector returns None.
#[tokio::test]
async fn test_save_chunk_without_embedding() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    repo.save_resource("https://example.com/doc2", "Doc2", "h2", 100)
        .await
        .unwrap();

    repo.save_chunk(
        "chunk-2",
        "https://example.com/doc2",
        0,
        "No embedding here",
        None,
        None,
    )
    .await
    .unwrap();

    let result = repo.get_vector("chunk-2").await.unwrap();
    assert_eq!(result, None, "no embedding should return None");
}

/// get_vector for nonexistent chunk returns None.
#[tokio::test]
async fn test_get_vector_nonexistent() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let result = repo.get_vector("nonexistent").await.unwrap();
    assert_eq!(result, None);
}

// ===== IN-MEMORY REPOSITORY =====

/// from_memory() creates a working in-memory repository.
#[tokio::test]
async fn test_from_memory_repository() {
    let repo = SqliteVectorRepository::from_memory().unwrap();
    setup_schema(repo.pool()).await.unwrap();

    repo.save_resource("https://test.com/page", "Test", "mem_hash", 50)
        .await
        .unwrap();

    let exists = repo.resource_exists_by_hash("mem_hash").await.unwrap();
    assert!(exists.is_some());
}

// ===== CONCURRENT WRITES =====

/// Multiple concurrent writes via the pool don't deadlock or corrupt data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_writes_via_pool() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let mut handles = Vec::new();
    for i in 0..10 {
        let repo = repo.clone(); // SqliteVectorRepository is Clone (wraps Arc Pool)
        handles.push(tokio::spawn(async move {
            let url = format!("https://example.com/page{i}");
            let hash = format!("hash_{i}");
            repo.save_resource(&url, &format!("Page {i}"), &hash, (i * 100) as u64)
                .await
                .unwrap();
        }));
    }

    for handle in handles {
        handle.await.expect("concurrent write should succeed");
    }

    // Verify all resources were saved
    for i in 0..10 {
        let hash = format!("hash_{i}");
        let exists = repo.resource_exists_by_hash(&hash).await.unwrap();
        assert!(exists.is_some(), "resource {hash} should exist");
    }
}

// ===== STORE INTEGRITY (#1828) =====

/// #1828 — the rows survive a reopen, and SQLite itself says the database is
/// `ok`.
///
/// The acceptance criterion of the issue is that the repository has NO
/// `PRAGMA integrity_check` anywhere: a corrupted store is discovered by the
/// USER, not by the tool. This test pins the invariant on a REAL file
/// database (not a memory pool, where there is nothing to corrupt): write
/// rows, drop the pool (the file is closed), reopen the same file, and ask
/// SQLite two things — that the database is intact, and that the rows are
/// still there.
#[tokio::test]
async fn test_sqlite_rows_pass_integrity_check_after_reopen() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("integrity.db");

    {
        let pool = create_pool(&db_path, 1).unwrap();
        setup_schema(&pool).await.unwrap();
        let repo = SqliteVectorRepository::new(pool);
        for idx in 0..3 {
            repo.save_resource(
                &format!("https://example.com/page{idx}"),
                &format!("Page {idx}"),
                &format!("hash_{idx}"),
                1024,
            )
            .await
            .unwrap();
        }
    } // the pool is dropped here: the file is closed and the rows are on disk

    let pool = create_pool(&db_path, 1).unwrap();
    let conn = pool.get().await.unwrap();
    let integrity: String = conn
        .interact(|c| c.query_row("PRAGMA integrity_check", [], |row| row.get(0)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        integrity, "ok",
        "SQLite must report the reopened store as intact"
    );
    drop(conn);

    // The rows survived the reopen: the store is not merely intact, it is the
    // SAME store.
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);
    for idx in 0..3 {
        assert_eq!(
            repo.resource_exists_by_hash(&format!("hash_{idx}"))
                .await
                .unwrap(),
            Some(format!("https://example.com/page{idx}")),
            "row {idx} must survive the reopen"
        );
    }
}

// ===== POOL LIFECYCLE =====

/// A pool with `max_size = 1` must recycle the single connection: two
/// sequential `get` calls hand out the same pooled object, not a fresh one.
/// If `Manager::recycle` rejects the connection, deadpool creates a new object
/// and the `ObjectId` changes — that is the regression this pins.
#[tokio::test]
async fn test_pool_recycles_connection_with_max_size_one() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();

    let first = pool.get().await.unwrap();
    let first_id = Object::id(&first);
    drop(first);

    let second = pool.get().await.unwrap();
    assert_eq!(
        Object::id(&second),
        first_id,
        "max_size=1 must recycle and reuse the same pooled object"
    );
}

/// With more than one connection, the `post_create` hook must run for every
/// connection, not just the first one. Hold all four objects at once so each is
/// created fresh, then read `journal_mode` from each.
#[tokio::test]
async fn test_wal_pragmas_applied_to_every_pooled_connection() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("pool-pragmas.db");
    let pool = create_pool(&db_path, 4).unwrap();
    setup_schema(&pool).await.unwrap();

    let mut conns = Vec::new();
    for _ in 0..4 {
        conns.push(pool.get().await.unwrap());
    }

    for conn in &conns {
        let mode: String = conn
            .interact(|c| c.query_row("PRAGMA journal_mode", [], |row| row.get(0)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mode, "wal", "every pooled connection must be in WAL mode");
    }
}

// ===== SAME URL, SAME HASH (#2654) =====

/// #2654: `resources.url` is the primary key, but the dedup check is by
/// `content_hash`, which is NOT unique. Two concurrent writers can both miss
/// the SELECT and race the INSERT, and the loser fails with
/// `UNIQUE constraint failed: resources.url`.
///
/// The race is probabilistic, so the round is repeated with a fresh URL per
/// round: on the current code it fails within a few rounds.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn save_resource_same_url_same_hash_is_idempotent() {
    use std::sync::Arc;
    use tokio::sync::Barrier;

    const TASKS: usize = 32;
    const ROUNDS: usize = 64;

    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("same-url.db");
    let pool = create_pool(&db_path, TASKS).unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = Arc::new(SqliteVectorRepository::new(pool.clone()));

    for round in 0..ROUNDS {
        let url = format!("https://example.com/race/{round}");
        let hash = format!("same-content-hash-{round}");
        let barrier = Arc::new(Barrier::new(TASKS));

        let mut handles = Vec::new();
        for _ in 0..TASKS {
            let repo = Arc::clone(&repo);
            let barrier = Arc::clone(&barrier);
            let url = url.clone();
            let hash = hash.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                repo.save_resource(&url, "Race", &hash, 1024).await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(
                result.is_ok(),
                "round {round}: concurrent save_resource must succeed, got: {result:?}"
            );
        }

        let conn = pool.get().await.unwrap();
        let count: i64 = conn
            .interact({
                let url = url.clone();
                move |c| {
                    c.query_row(
                        "SELECT COUNT(*) FROM resources WHERE url = ?1",
                        [url],
                        |row| row.get(0),
                    )
                }
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count, 1, "round {round}: the same URL must have one row");
    }
}

/// The different-hash case is OUT of scope for #2654 and must keep today's
/// behavior: the same URL with a different `content_hash` is still an error,
/// never a silent overwrite. This pins the pending product decision for
/// whoever takes it.
#[tokio::test]
async fn save_resource_same_url_different_hash_keeps_the_unique_error() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("same-url-different-hash.db");
    let pool = create_pool(&db_path, 2).unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    repo.save_resource("https://example.com/same", "First", "hash-one", 1)
        .await
        .unwrap();

    let err = repo
        .save_resource("https://example.com/same", "Second", "hash-two", 2)
        .await
        .expect_err("the same URL with a different hash must keep failing");
    let msg = err.to_string();
    assert!(
        msg.contains("UNIQUE constraint failed: resources.url"),
        "the different-hash case must keep today's error, got: {msg}"
    );
}
