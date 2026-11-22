//! SQLite schema, WAL pragmas, and connection pool for elastic ingestion.
//!
//! Schema is frozen per `design.md` (the authoritative frozen artifact). The
//! task-list DDL transcription differed in three places; this implementation
//! follows the frozen design:
//! - `chunks.id` is `TEXT` (required by `VectorRepository::save_vector(id: &str)`
//!   in PR4 — the task's `INTEGER` would force a destructive migration).
//! - `created_at` is `TEXT` (SQLite stores timestamps as TEXT regardless; the
//!   task's `TIMESTAMP` is cosmetic but diverges from design).
//! - the `content_hash` index lives on `resources` (which has the column); the
//!   task's `idx_chunks_hash ON chunks(content_hash)` referenced a non-existent
//!   column and would fail `setup_schema`.

mod blob;
mod connection;
mod note_repository;
mod retention;
mod schema;
mod vector_repository;

pub use connection::{create_memory_pool, create_pool};
pub use retention::prune_older_than;
pub use schema::SCHEMA_VERSION;
pub use vector_repository::{setup_schema, SqliteVectorRepository};

pub(crate) use schema::enforce_schema_version;

#[cfg(test)]
mod tests {
    use super::blob::{bytes_to_f32_vec, f32_slice_to_bytes};
    use super::*;
    use crate::domain::note_repository::NoteRepository;
    use crate::domain::repository::VectorRepository;
    use crate::domain::text_chunker::ByteSpan;
    use crate::infrastructure::persistence::pool::Pool;
    use tempfile::TempDir;

    // ====================================================================
    // Pure serialization round-trips (Miri-safe — no FFI, no I/O)
    // ====================================================================

    #[test]
    fn test_embedding_roundtrip() {
        // Triangulation: non-trivial vectors incl. empty, negatives, zero, and a
        // 384-dim all-MiniLM-L6-v2-shaped vector. Exact equality must hold.
        for v in [
            vec![],
            vec![0.0_f32],
            vec![1.5_f32, -2.25, 3.0, 0.0, -0.0, f32::MIN_POSITIVE],
            vec![-1.0_f32; 384],
        ] {
            let bytes = f32_slice_to_bytes(&v);
            assert_eq!(bytes.len(), v.len() * 4, "byte length must be 4×f32 count");
            let back = bytes_to_f32_vec(&bytes).expect("roundtrip must succeed");
            assert_eq!(back, v, "f32 vector must round-trip exactly");
        }
    }

    #[test]
    fn test_bytes_to_f32_vec_rejects_invalid_blob_length() {
        // 5 bytes is not a multiple of 4 → corrupt BLOB → Persistence error.
        let bad = [1_u8, 2, 3, 4, 5];
        let err = bytes_to_f32_vec(&bad).expect_err("odd-length BLOB must error");
        let msg = err.to_string();
        assert!(msg.contains("corrupto"), "missing Spanish marker: {msg}");
        assert!(msg.contains('5'), "missing length in message: {msg}");
    }

    // ====================================================================
    // SQLite-backed integration tests
    //
    // Miri es un intérprete de MIR, no un emulador de binarios. No puede
    // ejecutar código C nativo (libsqlite3-sys → sqlite3_threadsafe FFI).
    // Estos tests requieren SQLite3 real del sistema operativo.
    //
    // Estrategia: Aislamiento de Capas (Layer Isolation) — el bloque entero
    // se excluye de Miri con #[cfg(not(miri))] en lugar de parchear cada
    // test uno por uno. Misma práctica que tokio, serde, regex.
    // ====================================================================

    #[cfg(not(miri))]
    mod sqlite {
        use super::*;

        /// Build a fresh pool over a temp-file DB and run the schema.
        async fn fresh_pool(size: usize) -> (TempDir, Pool) {
            let dir = TempDir::new().expect("tempdir");
            let db_path = dir.path().join("crawl.db");
            let pool = create_pool(&db_path, size).expect("create_pool");
            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("setup_schema");
            (dir, pool)
        }

        /// Count rows in `sqlite_master` matching `type` and `name`.
        async fn master_count(pool: &Pool, kind: &str, name: &str) -> i64 {
            // Own the strings: the `interact` closure must be `Send + 'static`, so it
            // cannot capture borrowed `&str` from this function's frame.
            let kind = kind.to_string();
            let name = name.to_string();
            let conn = pool.get().await.expect("get");
            conn.interact(move |c| {
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    rusqlite::params![kind, name],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .expect("interact")
            .expect("count")
        }

        async fn pragma_string(pool: &Pool, pragma: &str) -> String {
            let sql = format!("PRAGMA {pragma}");
            let conn = pool.get().await.expect("get");
            conn.interact(move |c| c.query_row(&sql, [], |row| row.get::<_, String>(0)))
                .await
                .expect("interact")
                .expect("pragma row")
        }

        async fn pragma_int(pool: &Pool, pragma: &str) -> i64 {
            let sql = format!("PRAGMA {pragma}");
            let conn = pool.get().await.expect("get");
            conn.interact(move |c| c.query_row(&sql, [], |row| row.get::<_, i64>(0)))
                .await
                .expect("interact")
                .expect("pragma row")
        }

        #[tokio::test]
        async fn test_setup_schema_creates_resources_table() {
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(master_count(&pool, "table", "resources").await, 1);
        }

        // ================================================================
        // #1617 M4/D4 — the persisted schema version marker
        // ================================================================

        /// The marker is real: a fresh database carries it, and it survives a
        /// reopen. Without this, `PRAGMA user_version` on a SQLite file is
        /// indistinguishable from a field nothing reads — which is exactly the
        /// defect D5 describes for `metadata_version`.
        #[tokio::test]
        async fn setup_schema_stamps_the_schema_version_marker() {
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(
                pragma_int(&pool, "user_version").await,
                SCHEMA_VERSION,
                "a fresh database must be stamped with the current schema version"
            );
            // Idempotent second pass must neither fail nor re-stamp.
            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("re-setup is a no-op");
            assert_eq!(pragma_int(&pool, "user_version").await, SCHEMA_VERSION);
        }

        /// A database stamped by ANOTHER schema version is refused, and — the
        /// part that matters — refused BEFORE the DDL runs. If the tables were
        /// created first, this build would already have written to a file it
        /// cannot read, which is the corruption the marker exists to prevent.
        #[tokio::test]
        async fn setup_schema_refuses_a_foreign_schema_version_without_writing() {
            let (dir, pool) = fresh_pool(4).await;
            let db_path = dir.path().join("crawl.db");
            assert!(db_path.exists(), "fresh_pool materializes the DB file");

            {
                let conn = pool.get().await.expect("get");
                conn.interact(|c| {
                    c.execute_batch("PRAGMA user_version = 99; DROP TABLE resources;")
                })
                .await
                .expect("interact")
                .expect("seed foreign version");
            }
            assert_eq!(pragma_int(&pool, "user_version").await, 99);

            let err = SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect_err("a foreign schema version must fail closed");
            let msg = err.to_string();
            assert!(
                msg.contains("99") && msg.contains(&SCHEMA_VERSION.to_string()),
                "the refusal must name both versions: {msg}"
            );
            assert!(
                master_count(&pool, "table", "resources").await == 0,
                "the gate must run BEFORE the DDL: a refused database is never written to"
            );
            assert_eq!(
                pragma_int(&pool, "user_version").await,
                99,
                "a refused database keeps its own marker"
            );
        }

        /// An unversioned database (0) is ADOPTED, not refused: that is every
        /// database written before this marker existed, and refusing them
        /// would make the marker a breaking change for all existing installs.
        #[tokio::test]
        async fn setup_schema_adopts_an_unversioned_database() {
            let (dir, pool) = fresh_pool(4).await;
            {
                let conn = pool.get().await.expect("get");
                conn.interact(|c| c.execute_batch("PRAGMA user_version = 0;"))
                    .await
                    .expect("interact")
                    .expect("reset marker");
            }
            assert_eq!(pragma_int(&pool, "user_version").await, 0);

            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("an unversioned database must be adopted");
            assert_eq!(pragma_int(&pool, "user_version").await, SCHEMA_VERSION);
            assert_eq!(master_count(&pool, "table", "resources").await, 1);
            drop(dir);
        }

        /// A v1 database (pre-locators) is migrated in place: the note_chunks
        /// table gains `spans_json` and the marker moves to the current
        /// version, without touching existing rows. This is the one in-place
        /// migration the version gate tolerates.
        #[tokio::test]
        async fn setup_schema_migrates_a_v1_database_to_the_current_marker() {
            let dir = TempDir::new().expect("tempdir");
            let db_path = dir.path().join("crawl.db");
            let pool = create_pool(&db_path, 4).expect("create_pool");
            {
                let conn = pool.get().await.expect("get");
                conn.interact(|c| {
                    c.execute_batch(
                        "CREATE TABLE notes (\
                             id INTEGER PRIMARY KEY AUTOINCREMENT,\
                             path TEXT UNIQUE NOT NULL,\
                             content_hash TEXT NOT NULL,\
                             mtime_secs INTEGER NOT NULL,\
                             created_at TEXT,\
                             updated_at TEXT);\
                         CREATE TABLE note_chunks (\
                             id INTEGER PRIMARY KEY AUTOINCREMENT,\
                             note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,\
                             chunk_index INTEGER NOT NULL,\
                             content TEXT NOT NULL,\
                             embedding_vector BLOB,\
                             created_at TEXT);\
                         INSERT INTO notes (path, content_hash, mtime_secs) \
                             VALUES ('v1.md', 'hash-v1', 0);\
                         INSERT INTO note_chunks (note_id, chunk_index, content) \
                             VALUES (1, 0, 'v1 body');\
                         PRAGMA user_version = 1;",
                    )
                })
                .await
                .expect("interact")
                .expect("seed v1 database");
            }

            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("a v1 database must migrate in place");
            assert_eq!(
                pragma_int(&pool, "user_version").await,
                SCHEMA_VERSION,
                "the marker must move to the current version"
            );

            // The migrated column is real — it accepts a locator write — and
            // the v1 row survived the migration untouched.
            let conn = pool.get().await.expect("get");
            let (inserted, v1_rows): (i64, i64) = conn
                .interact(|c| {
                    c.execute(
                        "INSERT INTO note_chunks (note_id, chunk_index, content, spans_json, created_at) \
                         VALUES (1, 1, 'post-migration', '[{\"start\":0,\"end\":4}]', datetime('now'))",
                        [],
                    )?;
                    c.query_row(
                        "SELECT COUNT(*) FROM note_chunks WHERE content = 'v1 body'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|count| (1, count))
                })
                .await
                .expect("interact")
                .expect("post-migration probe");
            assert_eq!(inserted, 1, "the new column must accept locator JSON");
            assert_eq!(v1_rows, 1, "the pre-migration row must survive");
            drop(dir);
        }

        #[tokio::test]
        async fn test_setup_schema_creates_chunks_table() {
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(master_count(&pool, "table", "chunks").await, 1);
        }

        #[tokio::test]
        async fn test_setup_schema_creates_indexes() {
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(
                master_count(&pool, "index", "idx_chunks_resource").await,
                1,
                "idx_chunks_resource missing"
            );
            assert_eq!(
                master_count(&pool, "index", "idx_resources_content_hash").await,
                1,
                "idx_resources_content_hash missing"
            );
        }

        #[tokio::test]
        async fn test_wal_pragmas_applied_to_connection() {
            // A freshly-created pooled connection must carry all three pragmas
            // (proves the post_create hook runs on each connection).
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(pragma_string(&pool, "journal_mode").await, "wal");
            // synchronous: 0=OFF,1=NORMAL,2=FULL,3=EXTRA — NORMAL must be 1.
            assert_eq!(pragma_int(&pool, "synchronous").await, 1);
            assert_eq!(pragma_int(&pool, "cache_size").await, -4000);
        }

        #[tokio::test]
        async fn test_db_file_created_when_absent() {
            let dir = TempDir::new().expect("tempdir");
            let db_path = dir.path().join("nested").join("crawl.db");
            assert!(!db_path.exists());
            let pool = create_pool(&db_path, 4).expect("create_pool");
            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("setup_schema");
            // setup_schema materializes the DB file (parent dir created by create_pool).
            assert!(db_path.exists(), "DB file should exist after setup_schema");
        }

        #[tokio::test]
        async fn test_pool_size_is_configurable() {
            let (_dir, pool) = fresh_pool(8).await;
            let status = pool.status();
            assert_eq!(status.max_size, 8, "pool max_size must honor configuration");
        }

        #[tokio::test]
        async fn test_setup_schema_is_idempotent() {
            let (_dir, pool) = fresh_pool(4).await;
            // Running setup_schema twice must not error (CREATE ... IF NOT EXISTS).
            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("second setup_schema should be a no-op");
            assert_eq!(master_count(&pool, "table", "resources").await, 1);
            assert_eq!(master_count(&pool, "table", "chunks").await, 1);
        }

        #[tokio::test]
        async fn test_repository_wraps_pool() {
            let (_dir, pool) = fresh_pool(4).await;
            let repo = SqliteVectorRepository::new(pool.clone());
            assert_eq!(repo.pool().status().max_size, 4);
        }

        // ================================================================
        // PR4: VectorRepository CRUD + content-hash dedup (Strict TDD)
        // ================================================================

        /// Build a fresh schema-initialized repo over a temp-file DB.
        async fn fresh_repo(size: usize) -> (TempDir, SqliteVectorRepository) {
            let (dir, pool) = fresh_pool(size).await;
            let repo = SqliteVectorRepository::new(pool);
            (dir, repo)
        }

        // --- resource_exists_by_hash ---

        #[tokio::test]
        async fn test_resource_exists_by_hash_returns_none_when_empty() {
            let (_dir, repo) = fresh_repo(4).await;
            let got = repo.resource_exists_by_hash("nope").await.expect("query");
            assert!(got.is_none(), "fresh DB has no resources");
        }

        // --- save_resource + dedup (frozen decision #3) ---

        // --- save_chunk + get_vector ---

        #[tokio::test]
        async fn test_corrupt_blob_returns_error() {
            let (_dir, repo) = fresh_repo(4).await;
            repo.save_resource("https://example.com/bad", "Bad", "hash-bad", 1)
                .await
                .expect("save_resource");
            let url = "https://example.com/bad".to_string();
            let conn = repo.pool().get().await.expect("get");
            conn.interact(move |c| {
                c.execute(
                    "INSERT INTO chunks \
                     (id, resource_url, chunk_index, content, embedding_vector, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                    rusqlite::params!["bad-chunk", url, 0_i64, "", vec![1_u8, 2, 3, 4, 5]],
                )
            })
            .await
            .expect("interact")
            .expect("raw insert");
            let err = repo
                .get_vector("bad-chunk")
                .await
                .expect_err("corrupt BLOB must surface as an error");
            let msg = err.to_string();
            assert!(msg.contains("corrupto"), "missing Spanish marker: {msg}");
            assert!(msg.contains('5'), "missing length in message: {msg}");
        }

        #[tokio::test]
        async fn test_concurrent_writes_via_pool() {
            let (_dir, repo) = fresh_repo(4).await;
            let futures = (0..4_u8).map(|i| {
                let repo = repo.clone();
                async move {
                    repo.save_resource(
                        &format!("https://example.com/c{i}"),
                        "Concurrent",
                        &format!("hash-c{i}"),
                        1,
                    )
                    .await
                }
            });
            let results: Vec<_> = futures::future::join_all(futures).await;
            assert_eq!(results.len(), 4, "all 4 concurrent saves must complete");
            for r in &results {
                assert!(r.is_ok(), "concurrent save must not error: {r:?}");
            }
            for i in 0..4_u8 {
                let hash = format!("hash-c{i}");
                let url = format!("https://example.com/c{i}");
                let found = repo.resource_exists_by_hash(&hash).await.expect("query");
                assert_eq!(
                    found.as_deref(),
                    Some(url.as_str()),
                    "row for {hash} must exist after concurrent writes"
                );
            }
        }

        // ================================================================
        // Note chunk locators (chunk-locators slice)
        // ================================================================

        /// Locator round-trip through the port: what the chunker located must
        /// come back identical from `load_all_vectors`. A raw row written
        /// without `spans_json` (the pre-migration shape) loads with empty
        /// locators instead of failing the read.
        #[tokio::test]
        async fn note_chunks_round_trip_their_locators() {
            let (_dir, repo) = fresh_repo(4).await;
            let hash = "a".repeat(64);
            let spans = vec![
                ByteSpan::try_new(0, 5).expect("valid span"),
                ByteSpan::try_new(9, 14).expect("valid span"),
            ];
            let emb = vec![0.1_f32, 0.2, 0.3, 0.4];
            let chunks = [("# Nota", spans.as_slice(), Some(emb.as_slice()))];
            repo.index_note_transactional("notes/a.md", &hash, 0, &chunks)
                .await
                .expect("index_note_transactional");

            // Legacy row in the pre-locator shape: no spans_json at all.
            let note_id: i64 = {
                let conn = repo.pool().get().await.expect("get");
                conn.interact(|c| {
                    c.query_row(
                        "SELECT id FROM notes WHERE path = 'notes/a.md'",
                        [],
                        |row| row.get(0),
                    )
                })
                .await
                .expect("interact")
                .expect("note id")
            };
            {
                let conn = repo.pool().get().await.expect("get");
                conn.interact(move |c| {
                    c.execute(
                        "INSERT INTO note_chunks (note_id, chunk_index, content, embedding_vector, created_at) \
                         VALUES (?1, 1, 'legacy', ?2, datetime('now'))",
                        rusqlite::params![note_id, f32_slice_to_bytes(&emb)],
                    )
                })
                .await
                .expect("interact")
                .expect("legacy insert");
            }

            let vectors = repo.load_all_vectors().await.expect("load vectors");
            assert_eq!(vectors.len(), 2, "both chunks must load");
            let migrated = vectors
                .iter()
                .find(|v| v.chunk_index == 0)
                .expect("locator chunk");
            assert_eq!(
                migrated.spans, spans,
                "locators must round-trip through the BLOB/JSON boundary exactly"
            );
            let legacy = vectors
                .iter()
                .find(|v| v.chunk_index == 1)
                .expect("legacy chunk");
            assert!(
                legacy.spans.is_empty(),
                "a pre-locator row means 'no locators', never corruption"
            );
        }
    }
}
