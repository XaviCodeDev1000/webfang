use std::future::Future;
use std::pin::Pin;

use crate::domain::note_repository::{
    ChunkToPersist, IndexedNoteMeta, NoteChunkVector, NoteRepository,
};
use crate::domain::text_chunker::ByteSpan;
use crate::domain::Sha256Hex;
use crate::error::ScraperError;

use super::blob::{bytes_to_f32_vec, f32_slice_to_bytes};
use super::vector_repository::SqliteVectorRepository;

// ============================================================================
// NoteRepository impl (#386): vault note chunk persistence
// ============================================================================
impl NoteRepository for SqliteVectorRepository {
    fn save_note<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
        mtime_secs: i64,
    ) -> Pin<Box<dyn Future<Output = Result<i64, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let path_owned = path.to_string();
            let hash_owned = content_hash.to_string();

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            conn.interact(move |c| {
                // UPSERT: insert or update on path conflict, return the row id.
                c.execute(
                    "INSERT INTO notes (path, content_hash, mtime_secs, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, datetime('now'), datetime('now')) \
                     ON CONFLICT(path) DO UPDATE SET \
                       content_hash = excluded.content_hash, \
                       mtime_secs = excluded.mtime_secs, \
                       updated_at = datetime('now')",
                    rusqlite::params![path_owned, hash_owned, mtime_secs],
                )?;
                // Retrieve the id (last_insert_rowid works for both INSERT and UPDATE).
                c.query_row(
                    "SELECT id FROM notes WHERE path = ?1",
                    rusqlite::params![path_owned],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .map_err(|e| ScraperError::persistence(format!("save_note (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("save_note: {e}")))
        })
    }

    fn save_note_chunk<'a>(
        &'a self,
        note_id: i64,
        chunk_index: i64,
        content: &'a str,
        spans: &'a [ByteSpan],
        embedding: Option<&'a [f32]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let blob: Option<Vec<u8>> = embedding.map(f32_slice_to_bytes);
            let content = content.to_string();
            let spans_json = serde_json::to_string(spans)
                .map_err(|e| ScraperError::persistence(format!("serializar spans: {e}")))?;

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            conn.interact(move |c| {
                c.execute(
                    "INSERT INTO note_chunks (note_id, chunk_index, content, embedding_vector, spans_json, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                    rusqlite::params![note_id, chunk_index, content, blob.as_deref(), spans_json],
                )
            })
            .await
            .map_err(|e| ScraperError::persistence(format!("save_note_chunk (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("save_note_chunk: {e}")))?;
            Ok(())
        })
    }

    fn index_note_transactional<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
        mtime_secs: i64,
        chunks: &'a [ChunkToPersist<'a>],
    ) -> Pin<Box<dyn Future<Output = Result<i64, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let path_owned = path.to_string();
            let hash_owned = content_hash.to_string();
            let chunks_owned: Vec<(String, String, Option<Vec<u8>>)> = chunks
                .iter()
                .map(|(text, spans, emb)| {
                    let spans_json = serde_json::to_string(*spans)
                        .map_err(|e| ScraperError::persistence(format!("serializar spans: {e}")))?;
                    Ok(((*text).to_string(), spans_json, emb.map(f32_slice_to_bytes)))
                })
                .collect::<Result<Vec<_>, ScraperError>>()?;

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            conn.interact(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO notes (path, content_hash, mtime_secs, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, datetime('now'), datetime('now')) \
                     ON CONFLICT(path) DO UPDATE SET \
                       content_hash = excluded.content_hash, \
                       mtime_secs = excluded.mtime_secs, \
                       updated_at = datetime('now')",
                    rusqlite::params![&path_owned, &hash_owned, mtime_secs],
                )?;
                let id = tx.query_row(
                    "SELECT id FROM notes WHERE path = ?1",
                    rusqlite::params![&path_owned],
                    |row| row.get::<_, i64>(0),
                )?;
                for (i, (text, spans_json, emb_blob)) in chunks_owned.into_iter().enumerate() {
                    tx.execute(
                        "INSERT INTO note_chunks (note_id, chunk_index, content, embedding_vector, spans_json, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                        rusqlite::params![id, i as i64, &text, &emb_blob.as_deref(), &spans_json],
                    )?;
                }
                tx.commit()?;
                rusqlite::Result::<i64, rusqlite::Error>::Ok(id)
            })
            .await
            .map_err(|e| {
                ScraperError::persistence(format!("index_note_transactional (interact): {e}"))
            })?
            .map_err(|e| ScraperError::persistence(format!("index_note_transactional: {e}")))
        })
    }

    fn load_all_vectors(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NoteChunkVector>, ScraperError>> + Send + '_>> {
        Box::pin(async move {
            type LoadedVectorRow = (String, String, i64, Option<String>, Vec<u8>);
            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            let rows: Vec<LoadedVectorRow> = conn
                .interact(|c| {
                    let mut stmt = c.prepare(
                        "SELECT n.path, nc.content, nc.chunk_index, nc.spans_json, nc.embedding_vector \
                         FROM note_chunks nc \
                         JOIN notes n ON n.id = nc.note_id \
                         WHERE nc.embedding_vector IS NOT NULL",
                    )?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Vec<u8>>(4)?,
                        ))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()
                })
                .await
                .map_err(|e| {
                    ScraperError::persistence(format!("load_all_vectors (interact): {e}"))
                })?
                .map_err(|e| ScraperError::persistence(format!("load_all_vectors: {e}")))?;

            let mut result = Vec::with_capacity(rows.len());
            for (path, content, chunk_index, spans_json, bytes) in rows {
                // Locator provenance must be readable or the read fails: a
                // corrupt spans_json aborts the load instead of ranking
                // chunks with silently lost locators. NULL (pre-migration
                // rows) means "no locators", not corruption.
                let spans = match spans_json {
                    Some(json) => serde_json::from_str::<Vec<ByteSpan>>(&json).map_err(|e| {
                        ScraperError::persistence(format!("spans_json corrupto para {path}: {e}"))
                    })?,
                    None => Vec::new(),
                };
                result.push(NoteChunkVector {
                    note_path: path,
                    content,
                    chunk_index,
                    spans,
                    embedding: bytes_to_f32_vec(&bytes)?,
                });
            }
            Ok(result)
        })
    }

    fn note_needs_reindex<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let path_owned = path.to_string();
            let hash_owned = content_hash.to_string();

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            let row: rusqlite::Result<String> = conn
                .interact(move |c| {
                    c.query_row(
                        "SELECT content_hash FROM notes WHERE path = ?1 LIMIT 1",
                        rusqlite::params![path_owned],
                        |row| row.get::<_, String>(0),
                    )
                })
                .await
                .map_err(|e| {
                    ScraperError::persistence(format!("note_needs_reindex (interact): {e}"))
                })?;
            match row {
                Ok(stored_hash) => Ok(stored_hash != hash_owned),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(true), // not indexed
                Err(e) => Err(ScraperError::persistence(format!(
                    "note_needs_reindex: {e}"
                ))),
            }
        })
    }

    fn delete_note<'a>(
        &'a self,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let path_owned = path.to_string();

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            conn.interact(move |c| {
                // Enable FK cascades for this connection, then delete.
                c.execute_batch("PRAGMA foreign_keys = ON;")?;
                c.execute(
                    "DELETE FROM notes WHERE path = ?1",
                    rusqlite::params![path_owned],
                )
            })
            .await
            .map_err(|e| ScraperError::persistence(format!("delete_note (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("delete_note: {e}")))?;
            Ok(())
        })
    }

    fn list_indexed_notes(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IndexedNoteMeta>, ScraperError>> + Send + '_>> {
        Box::pin(async move {
            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            let rows: Vec<(String, String, i64)> = conn
                .interact(|c| {
                    let mut stmt = c.prepare("SELECT path, content_hash, mtime_secs FROM notes")?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()
                })
                .await
                .map_err(|e| {
                    ScraperError::persistence(format!("list_indexed_notes (interact): {e}"))
                })?
                .map_err(|e| ScraperError::persistence(format!("list_indexed_notes: {e}")))?;
            // Stored hashes are validated on read (#1118): a corrupt
            // content_hash column aborts the listing instead of poisoning
            // the staleness comparison downstream.
            rows.into_iter()
                .map(|(path, hash, mtime_secs)| {
                    Sha256Hex::try_from(hash.as_str())
                        .map(|content_hash| IndexedNoteMeta {
                            path,
                            content_hash,
                            mtime_secs,
                        })
                        .map_err(|e| {
                            ScraperError::persistence(format!(
                                "content_hash almacenado inválido: {e}"
                            ))
                        })
                })
                .collect()
        })
    }
}
