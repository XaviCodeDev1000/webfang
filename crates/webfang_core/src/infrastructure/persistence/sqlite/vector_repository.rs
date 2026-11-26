use std::future::Future;
use std::pin::Pin;

use crate::domain::repository::{ChunkVector, VectorRepository};
use crate::domain::text_chunker::ByteSpan;
use crate::error::ScraperError;
use crate::infrastructure::persistence::pool::Pool;

use super::blob::{bytes_to_f32_vec, f32_slice_to_bytes};
use super::connection::create_memory_pool;
use super::schema::{enforce_schema_version, SCHEMA_DDL};

// ============================================================================
// SqliteVectorRepository (PR1: schema init; PR4 adds CRUD + dedup)
// ============================================================================
/// SQLite-backed vector repository for the elastic ingestion pipeline.
///
/// PR1 provides explicit schema initialization only; the `VectorRepository`
/// trait CRUD (`save_vector`, `get_vector`) and content-hash deduplication land
/// in PR4. Per frozen design decision #1, schema creation is **explicit** —
/// [`SqliteVectorRepository::setup_schema`] MUST be called at startup. There is
/// no implicit schema creation on first connection.
#[derive(Debug, Clone)]
pub struct SqliteVectorRepository {
    pub(super) pool: Pool,
}

impl SqliteVectorRepository {
    /// Wrap an existing pool. The pool should already have had
    /// [`setup_schema`] run against it.
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Create a repository backed by an in-memory SQLite database.
    ///
    /// Intended for integration tests — no disk I/O, automatic cleanup on drop.
    /// Does **not** call [`setup_schema`]; the caller must do that explicitly
    /// if tables are needed.
    pub fn from_memory() -> Result<Self, ScraperError> {
        let pool = create_memory_pool()?;
        Ok(Self::new(pool))
    }

    /// Borrow the underlying pool (used by PR2+ wiring and tests).
    #[must_use]
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Explicitly initialize the database schema (tables + indexes).
    ///
    /// **MUST** be called once at startup (frozen design decision #1). Acquires
    /// a pooled connection — which also materializes the database file and runs
    /// the per-connection pragmas via the `post_create` hook — making this the
    /// fail-fast point if the configured `db_path` is not writable.
    ///
    /// The DDL is idempotent (`CREATE ... IF NOT EXISTS`), so calling it on an
    /// already-initialized database is a safe no-op.
    ///
    /// The [`crate::infrastructure::persistence::sqlite::SCHEMA_VERSION`] gate runs FIRST, before any DDL: a database
    /// stamped by another schema version is refused without being opened for
    /// writing at all (M4/D4, #1617).
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] if the pool's startup
    /// `PRAGMA integrity_check` fails (surfaced by the first `pool.get()`),
    /// the schema marker belongs to another version, or the DDL batch fails.
    pub async fn setup_schema(pool: &Pool) -> Result<(), ScraperError> {
        let conn = pool
            .get()
            .await
            .map_err(|e| ScraperError::persistence(format!("obtener conexión del pool: {e}")))?;
        conn.interact(|c| {
            // Gate before DDL: refusing after the tables were created would
            // mean this build had already written to a file it cannot read.
            enforce_schema_version(c)?;
            c.execute_batch(SCHEMA_DDL)
                .map_err(|e| ScraperError::persistence(format!("ddl schema: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| ScraperError::persistence(format!("ddl (interact): {e}")))?
    }
}

/// Insert the resource row, or update it in place when that URL already
/// carries a different content.
///
/// `resources.url` is the primary key, but the dedup check upstream is by
/// `content_hash`, which is NOT unique: two concurrent writers can both miss
/// the check and race the INSERT, and the loser used to fail with
/// `UNIQUE constraint failed: resources.url`. `INSERT OR IGNORE` makes the
/// insert idempotent instead of failing.
///
/// An ignored row that carries the SAME `content_hash` is the same resource:
/// nothing to do. A DIFFERENT one means the content of that URL changed: the
/// row is updated in place (#2663 — the row of a URL reflects its most recent
/// content) and the chunks of the PREVIOUS content are deleted in the SAME
/// transaction. The chunk ids carry the content hash (`{hash}-{index}`), so
/// the new chunks would NOT replace the old ones: leaving them answers a
/// vector search with stale fragments of the old content, which fails
/// silently — worse than the error this replaces.
fn upsert_resource_and_drop_stale_chunks(
    conn: &mut rusqlite::Connection,
    url: &str,
    title: &str,
    content_hash: &str,
    size_bytes: u64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "INSERT OR IGNORE INTO resources \
         (url, title, content_hash, size_bytes, status, created_at, updated_at, metadata_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'), NULL)",
        rusqlite::params![url, title, content_hash, size_bytes as i64, "active"],
    )?;
    if changed == 0 {
        let stored_hash: String = tx.query_row(
            "SELECT content_hash FROM resources WHERE url = ?1",
            rusqlite::params![url],
            |row| row.get(0),
        )?;
        if stored_hash != content_hash {
            // The content of this URL changed: the row takes the new content
            // (`created_at` is preserved, `updated_at` is refreshed) and the
            // chunks of the PREVIOUS content go away with it, atomically.
            tx.execute(
                "UPDATE resources SET title = ?2, content_hash = ?3, size_bytes = ?4, \
                 updated_at = datetime('now') WHERE url = ?1",
                rusqlite::params![url, title, content_hash, size_bytes as i64],
            )?;
            tx.execute(
                "DELETE FROM chunks WHERE resource_url = ?1",
                rusqlite::params![url],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

// ============================================================================
// VectorRepository impl (PR4): CRUD + content-hash dedup
// ============================================================================
impl VectorRepository for SqliteVectorRepository {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        content_hash: &'a str,
        size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            // Dedup short-circuit (frozen decision #3): if the content_hash already
            // exists, return the existing URL and skip the INSERT (I/O saved).
            if let Some(existing) = self.resource_exists_by_hash(content_hash).await? {
                tracing::debug!(
                    content_hash,
                    existing_url = %existing,
                    "dedup: recurso ya persistido, omitiendo inserción"
                );
                return Ok(existing);
            }

            // Own all borrowed inputs: the `interact` closure must be `Send + 'static`
            // and cannot borrow `&str` from this function's frame.
            let url_owned = url.to_string();
            let title_owned = title.to_string();
            let hash_owned = content_hash.to_string();
            let url_for_row = url_owned.clone();

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            conn.interact(move |c| {
                upsert_resource_and_drop_stale_chunks(
                    c,
                    &url_for_row,
                    &title_owned,
                    &hash_owned,
                    size_bytes,
                )
            })
            .await
            .map_err(|e| ScraperError::persistence(format!("save_resource (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("save_resource: {e}")))?;
            Ok(url_owned)
        })
    }

    fn save_chunk<'a>(
        &'a self,
        id: &'a str,
        resource_url: &'a str,
        chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
        _spans: Option<&'a [ByteSpan]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            // Pre-serialize the embedding to owned bytes: the `interact` closure must
            // be `Send + 'static`, so it cannot borrow `&[f32]` from this frame.
            let blob: Option<Vec<u8>> = embedding.map(f32_slice_to_bytes);
            let id = id.to_string();
            let resource_url = resource_url.to_string();
            let content = content.to_string();

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            conn.interact(move |c| {
                // INSERT OR IGNORE: the chunks table enforces uniqueness via
                // `id TEXT PRIMARY KEY`. Under concurrent batch processing a
                // TOCTOU race can occur between the dedup check and the insert;
                // OR IGNORE makes the insert idempotent instead of erroring.
                c.execute(
                    "INSERT OR IGNORE INTO chunks \
                     (id, resource_url, chunk_index, content, embedding_vector, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                    rusqlite::params![id, resource_url, chunk_index, content, blob.as_deref()],
                )
            })
            .await
            .map_err(|e| ScraperError::persistence(format!("save_chunk (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("save_chunk: {e}")))?;
            Ok(())
        })
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let hash = content_hash.to_string();
            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            let row: rusqlite::Result<String> = conn
                .interact(move |c| {
                    c.query_row(
                        "SELECT url FROM resources WHERE content_hash = ?1 LIMIT 1",
                        rusqlite::params![hash],
                        |row| row.get::<_, String>(0),
                    )
                })
                .await
                .map_err(|e| {
                    ScraperError::persistence(format!("resource_exists_by_hash (interact): {e}"))
                })?;
            match row {
                Ok(url) => Ok(Some(url)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(ScraperError::persistence(format!(
                    "resource_exists_by_hash: {e}"
                ))),
            }
        })
    }

    fn get_vector<'a>(
        &'a self,
        chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let id = chunk_id.to_string();
            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            let row: rusqlite::Result<Option<Vec<u8>>> = conn
                .interact(move |c| {
                    c.query_row(
                        "SELECT embedding_vector FROM chunks WHERE id = ?1 LIMIT 1",
                        rusqlite::params![id],
                        |row| row.get::<_, Option<Vec<u8>>>(0),
                    )
                })
                .await
                .map_err(|e| ScraperError::persistence(format!("get_vector (interact): {e}")))?;
            match row {
                Ok(Some(bytes)) => Ok(Some(bytes_to_f32_vec(&bytes)?)),
                Ok(None) => Ok(None),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(ScraperError::persistence(format!("get_vector: {e}"))),
            }
        })
    }

    fn load_chunk_vectors<'a>(
        &'a self,
        resource_url: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ChunkVector>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            type LoadedChunkRow = (String, String, Vec<u8>);
            const ALL_CHUNKS: &str = "SELECT id, content, embedding_vector FROM chunks \
                                      WHERE embedding_vector IS NOT NULL";
            const RESOURCE_CHUNKS: &str = "SELECT id, content, embedding_vector FROM chunks \
                                           WHERE embedding_vector IS NOT NULL AND resource_url = ?1";

            // Own the filter before `interact`: its closure must be `Send + 'static`.
            let url = resource_url.map(str::to_owned);
            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            let rows: Vec<LoadedChunkRow> = conn
                .interact(move |c| {
                    let (sql, params): (&str, Vec<&str>) = match url.as_deref() {
                        Some(url) => (RESOURCE_CHUNKS, vec![url]),
                        None => (ALL_CHUNKS, Vec::new()),
                    };
                    let mut stmt = c.prepare(sql)?;
                    let rows = stmt.query_map(
                        rusqlite::params_from_iter(params.iter().copied()),
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Vec<u8>>(2)?,
                            ))
                        },
                    )?;
                    rows.collect::<Result<Vec<_>, _>>()
                })
                .await
                .map_err(|e| {
                    ScraperError::persistence(format!("load_chunk_vectors (interact): {e}"))
                })?
                .map_err(|e| ScraperError::persistence(format!("load_chunk_vectors: {e}")))?;

            let mut result = Vec::with_capacity(rows.len());
            for (id, content, bytes) in rows {
                result.push(ChunkVector {
                    id,
                    content,
                    embedding: bytes_to_f32_vec(&bytes)?,
                });
            }
            Ok(result)
        })
    }
}

/// Free-function alias for [`SqliteVectorRepository::setup_schema`] so callers
/// that only hold a `&Pool` (without constructing the repository) can init the
/// schema at startup.
pub async fn setup_schema(pool: &Pool) -> Result<(), ScraperError> {
    SqliteVectorRepository::setup_schema(pool).await
}
