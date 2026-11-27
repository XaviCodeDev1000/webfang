//! Vault search service — semantic search over Obsidian vault notes (#386).
//!
//! Orchestrates the search pipeline: load indexed vectors → embed query →
//! rank by cosine similarity → return top-N results. Also provides note
//! indexing: chunk markdown → embed chunks → persist to NoteRepository.
//!
//! # Architecture
//!
//! Concrete application struct (following the `ElasticIngestion` pattern).
//! Injects domain ports (`EmbeddingPort`, `NoteRepository`, `TextChunker`,
//! `VaultNoteReader`) — no dependency on `webfang_ai` concrete types and no
//! direct infrastructure calls (ADR-0012-B sub-slice 3.I, #1071).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, instrument, warn};

use crate::domain::embedding_port::{cosine_similarity_fail_closed, EmbeddingPort};
use crate::domain::note_repository::{ChunkToPersist, NoteRepository, VaultNoteReader};
use crate::domain::text_chunker::{ByteSpan, TextChunker};
use crate::error::ScraperError;

/// A search result with relevance score and source metadata.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VaultSearchResult {
    /// Filesystem path of the source note.
    pub note_path: String,
    /// The matching chunk's text content.
    pub content: String,
    /// Cosine similarity score (0.0 to 1.0).
    pub score: f32,
    /// Zero-based chunk index within the note.
    pub chunk_index: i64,
    /// Heading context (if available from chunking metadata).
    pub heading: Option<String>,
    /// Byte spans of the note this chunk was cut from, in order
    /// (half-open `[start, end)`). Empty for chunks indexed before
    /// locators existed.
    pub spans: Vec<ByteSpan>,
}

/// Summary of a vault sync operation.
///
/// Returned by [`VaultSearchService::sync_vault`] after reconciling
/// the filesystem vault against the persistent index.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct SyncSummary {
    /// Notes newly indexed (not previously in the index).
    pub indexed: usize,
    /// Notes re-indexed because their content hash changed.
    pub updated: usize,
    /// Notes removed from the index (deleted from the vault).
    pub deleted: usize,
    /// Notes already indexed with matching content hash (skipped).
    pub unchanged: usize,
}

/// Semantic search service for Obsidian vault notes.
///
/// Orchestrates domain ports to provide:
/// - **Search**: embed query → load indexed vectors → rank → top-N
/// - **Indexing**: chunk markdown → embed chunks → persist
///
/// Construct with [`VaultSearchService::new`]; all dependencies are
/// injected as `Arc<dyn Port>` for testability.
pub struct VaultSearchService {
    embedding: Arc<dyn EmbeddingPort>,
    repository: Arc<dyn NoteRepository>,
    chunker: Arc<dyn TextChunker>,
    note_reader: Arc<dyn VaultNoteReader>,
}

impl VaultSearchService {
    /// Wire the four pipeline components.
    ///
    /// `note_reader` is the domain port for filesystem vault reads
    /// (#1071): production callers inject the infrastructure `VaultFsReader`
    /// (directly or via the `Container`), tests inject a stub.
    #[must_use]
    pub fn new(
        embedding: Arc<dyn EmbeddingPort>,
        repository: Arc<dyn NoteRepository>,
        chunker: Arc<dyn TextChunker>,
        note_reader: Arc<dyn VaultNoteReader>,
    ) -> Self {
        Self {
            embedding,
            repository,
            chunker,
            note_reader,
        }
    }

    /// Search the vault index for notes matching the query.
    ///
    /// # Pipeline
    ///
    /// 1. Embed the query text via [`EmbeddingPort`]
    /// 2. Load all indexed chunk vectors via [`NoteRepository`]
    /// 3. Rank by cosine similarity (SIMD-friendly, inline implementation)
    /// 4. Return top-`limit` results sorted by descending score
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError`] if embedding or database access fails.
    #[instrument(skip(self), fields(query_len = query.len(), limit))]
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<VaultSearchResult>, ScraperError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Step 1: Embed the query.
        // Vault lookup is the QUERY side of an asymmetric model: use the
        // `query_extra_body` overlay instead of the indexing body (#3074).
        let query_embedding = self
            .embedding
            .embed_query(query)
            .await
            .map_err(ScraperError::Semantic)?;

        // Step 2: Load all indexed vectors.
        let chunks = self.repository.load_all_vectors().await?;

        if chunks.is_empty() {
            debug!("vault index is empty — no results");
            return Ok(Vec::new());
        }

        // #1462: wholesale dim mismatch fail-closed. When NO stored vector
        // matches the serving dim, the whole index was built by another
        // backend (e.g. a 384d vault now served by an 8d remote adapter) and
        // ranking would silently degrade to nothing — fail with re-index
        // guidance instead of empty results. A MIXED index keeps the
        // skip-with-warn path below (search is the read path that must not
        // serve corrupt ranking; `sync_vault` stays the repair path that
        // re-indexes stale vectors).
        let serving_dim = query_embedding.len();
        if !chunks
            .iter()
            .any(|chunk| chunk.embedding.len() == serving_dim)
        {
            let stored_dim = chunks.first().map_or(0, |chunk| chunk.embedding.len());
            return Err(ScraperError::Config(format!(
                "la bóveda usa dim {stored_dim} pero el adaptador de embeddings sirve \
                 dim {serving_dim}: re-indexe la bóveda con el modelo actual"
            )));
        }

        // Step 3: Rank by cosine similarity. Chunks whose vector
        // dimension differs from the query embedding (mixed-model index
        // corruption) are skipped — never ranked with a silent 0.0 — and
        // counted for the summary warn below.
        let expected_dim = query_embedding.len();
        let mut skipped = 0usize;
        let mut first_actual_dim = 0usize;
        let mut scored: Vec<VaultSearchResult> = chunks
            .into_iter()
            .filter_map(|chunk| {
                match cosine_similarity_fail_closed(&query_embedding, &chunk.embedding) {
                    Some(score) => Some(VaultSearchResult {
                        note_path: chunk.note_path,
                        content: chunk.content,
                        score,
                        chunk_index: chunk.chunk_index,
                        heading: None, // TODO: extract from chunk metadata when NoteChunkVector carries it
                        spans: chunk.spans,
                    }),
                    None => {
                        skipped += 1;
                        if skipped == 1 {
                            first_actual_dim = chunk.embedding.len();
                        }
                        None
                    },
                }
            })
            .collect();

        if skipped > 0 {
            warn!(
                expected_dim,
                actual_dim = first_actual_dim,
                skipped,
                "vault search skipped vectors with dimension mismatch (mixed-model index?)"
            );
        }

        // Sort descending by score.
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Step 4: Truncate to limit.
        scored.truncate(limit);

        info!(
            results = scored.len(),
            top_score = scored.first().map_or(0.0, |r| r.score),
            "vault search completed"
        );

        Ok(scored)
    }

    /// Index a single note: chunk → embed → persist.
    ///
    /// The caller is responsible for reading the file and providing its
    /// content. This method handles chunking, embedding, and storage.
    ///
    /// # Pipeline
    ///
    /// 1. Chunk the markdown content via [`TextChunker`]
    /// 2. Embed all chunks via [`EmbeddingPort::embed_batch`]
    /// 3. Register the note via [`NoteRepository::save_note`]
    /// 4. Persist each chunk with its embedding
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError`] if chunking, embedding, or persistence fails.
    #[instrument(skip(self, content), fields(path, content_len = content.len()))]
    pub async fn index_note(
        &self,
        path: &str,
        content: &str,
        content_hash: &str,
        mtime_secs: i64,
    ) -> Result<usize, ScraperError> {
        // Step 1: Chunk the markdown.
        let chunks = self
            .chunker
            .chunk_text(content)
            .map_err(ScraperError::Semantic)?;

        // Step 2: Embed all chunks in a batch (skip when empty).
        let embeddings = if chunks.is_empty() {
            Vec::new()
        } else {
            let texts: Vec<String> = chunks.iter().map(|c| c.text().to_owned()).collect();
            self.embedding
                .embed_batch(&texts)
                .await
                .map_err(ScraperError::Semantic)?
        };

        // Step 3: Atomically register the note and persist every chunk in one
        // transaction, locators included — the index keeps, per chunk, which
        // byte ranges of the note it was cut from. Empty `chunks` still
        // registers the note so sync_vault won't re-index it on every run
        // (#577 follow-up: atomicity).
        let chunks_with_embeddings: Vec<ChunkToPersist> = chunks
            .iter()
            .zip(embeddings.iter())
            .map(|(chunk, emb)| (chunk.text(), chunk.spans(), Some(emb.as_slice())))
            .collect();
        let _note_id = self
            .repository
            .index_note_transactional(path, content_hash, mtime_secs, &chunks_with_embeddings)
            .await?;

        debug!(path, chunks = chunks.len(), "note indexed");
        Ok(chunks.len())
    }

    /// Check whether a note needs re-indexing.
    ///
    /// Delegates to [`NoteRepository::note_needs_reindex`] which compares
    /// the stored content hash against the provided one.
    pub async fn needs_reindex(
        &self,
        path: &str,
        content_hash: &str,
    ) -> Result<bool, ScraperError> {
        self.repository.note_needs_reindex(path, content_hash).await
    }

    /// Remove a note and all its chunks from the index.
    pub async fn remove_note(&self, path: &str) -> Result<(), ScraperError> {
        self.repository.delete_note(path).await
    }

    /// Synchronize the vault filesystem against the persistent index.
    ///
    /// # Pipeline
    ///
    /// 1. Read all `.md` notes from the vault via the injected
    ///    [`VaultNoteReader`] domain port
    /// 2. Load all indexed note metadata via [`NoteRepository::list_indexed_notes`]
    /// 3. Compare content hashes:
    ///    - **New** note (not indexed) → [`index_note`](Self::index_note)
    ///    - **Changed** note (hash differs) → re-index
    ///    - **Deleted** note (indexed but gone from vault) → [`remove_note`](Self::remove_note)
    ///    - **Unchanged** → skip
    /// 4. Return a [`SyncSummary`] with counts
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError`] if vault reading, database access,
    /// embedding, or persistence fails. Individual note indexing errors
    /// are logged and skipped (best-effort sync).
    #[instrument(skip(self), fields(vault_path = %vault_path.display()))]
    pub async fn sync_vault(&self, vault_path: &Path) -> Result<SyncSummary, ScraperError> {
        let start = Instant::now();

        // Step 1: Read all notes through the injected vault reader port.
        // #1104: the production adapter walks the whole vault (WalkDir +
        // std::fs read/metadata per note) — synchronous I/O that can take
        // seconds on large vaults. Run it on the blocking pool with the port
        // shared via Arc so the future never parks a Tokio worker.
        let path = vault_path.to_owned();
        let reader = Arc::clone(&self.note_reader);
        let notes = tokio::task::spawn_blocking(move || reader.read_vault_notes(&path))
            .await
            .map_err(|e| ScraperError::ingestion(format!("leer vault (join): {e}")))??;

        // Step 2: Load all indexed note metadata.
        let indexed = self.repository.list_indexed_notes().await?;

        // Build lookup structures.
        let indexed_map: HashMap<&str, &str> = indexed
            .iter()
            .map(|m| (m.path.as_str(), m.content_hash.as_str()))
            .collect();
        let vault_paths: HashSet<&str> = notes.iter().map(|n| n.path.as_str()).collect();

        let mut summary = SyncSummary::default();

        // Step 3a: Index new and changed notes.
        for note in &notes {
            match indexed_map.get(note.path.as_str()) {
                None => {
                    // New note — never indexed.
                    match self
                        .index_note(
                            &note.path,
                            &note.content,
                            note.content_hash.as_str(),
                            note.mtime_secs,
                        )
                        .await
                    {
                        Ok(_) => summary.indexed += 1,
                        Err(e) => {
                            warn!(path = %note.path, "failed to index note: {e}");
                        },
                    }
                },
                Some(stored_hash) if *stored_hash != note.content_hash.as_str() => {
                    // Changed note — content hash differs.
                    match self
                        .index_note(
                            &note.path,
                            &note.content,
                            note.content_hash.as_str(),
                            note.mtime_secs,
                        )
                        .await
                    {
                        Ok(_) => summary.updated += 1,
                        Err(e) => {
                            warn!(path = %note.path, "failed to re-index note: {e}");
                        },
                    }
                },
                Some(_) => {
                    // Unchanged — hash matches.
                    summary.unchanged += 1;
                },
            }
        }

        // Step 3b: Delete notes that are no longer in the vault.
        for meta in &indexed {
            if !vault_paths.contains(meta.path.as_str()) {
                match self.remove_note(&meta.path).await {
                    Ok(()) => summary.deleted += 1,
                    Err(e) => {
                        warn!(path = %meta.path, "failed to delete stale note: {e}");
                    },
                }
            }
        }

        let duration = start.elapsed();
        info!(
            indexed = summary.indexed,
            updated = summary.updated,
            deleted = summary.deleted,
            unchanged = summary.unchanged,
            duration_ms = duration.as_millis() as u64,
            "vault sync completed"
        );

        Ok(summary)
    }
}

#[cfg(test)]
#[path = "vault_search_search_tests.rs"]
mod tests_search;

#[cfg(test)]
#[path = "vault_search_support_tests.rs"]
mod tests_support;

#[cfg(test)]
#[path = "vault_search_sync_tests.rs"]
mod tests_sync;
