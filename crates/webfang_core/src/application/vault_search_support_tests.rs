use super::*;

// --- Stub ports for sync_vault tests ---

use crate::domain::note_repository::{IndexedNoteMeta, NoteChunkVector, VaultNote};
use crate::domain::text_chunker::{ByteSpan, TextChunk};
use crate::domain::Sha256Hex;
use crate::error::SemanticError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Stub embedder: returns a fixed 3-dim vector for any input.
pub(super) struct StubEmbedding;

impl EmbeddingPort for StubEmbedding {
    fn embed<'a>(&'a self, _text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
        Box::pin(async { Ok(vec![0.1, 0.2, 0.3]) })
    }

    fn embedding_dim(&self) -> usize {
        3
    }

    fn model_id(&self) -> &str {
        "stub-embedding"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Stub chunker: one chunk per non-blank line, span covering the raw line.
pub(super) struct StubChunker;

impl TextChunker for StubChunker {
    fn chunk_text(&self, text: &str) -> Result<Vec<TextChunk>, SemanticError> {
        let mut chunks = Vec::new();
        let mut offset = 0;
        for line in text.split('\n') {
            if !line.trim().is_empty() {
                let span = ByteSpan::try_new(offset, offset + line.len())?;
                chunks.push(TextChunk::new(line, vec![span]));
            }
            offset += line.len() + '\n'.len_utf8();
        }
        Ok(chunks)
    }
}

/// In-memory note repository for testing sync logic.
#[derive(Default)]
pub(super) struct InMemoryNoteRepo {
    notes: Mutex<Vec<IndexedNoteMeta>>,
    pub(super) chunks: Mutex<Vec<NoteChunkVector>>,
    next_id: Mutex<i64>,
}

impl NoteRepository for InMemoryNoteRepo {
    fn save_note<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
        mtime_secs: i64,
    ) -> BoxFuture<'a, Result<i64, ScraperError>> {
        Box::pin(async move {
            let hash = Sha256Hex::try_from(content_hash)
                .map_err(|e| ScraperError::persistence(format!("invalid hash: {e}")))?;
            let mut notes = self.notes.lock().unwrap();
            // Update existing or insert new.
            if let Some(existing) = notes.iter_mut().find(|n| n.path == path) {
                existing.content_hash = hash;
                existing.mtime_secs = mtime_secs;
                return Ok(1);
            }
            let mut id = self.next_id.lock().unwrap();
            *id += 1;
            notes.push(IndexedNoteMeta {
                path: path.to_owned(),
                content_hash: hash,
                mtime_secs,
            });
            Ok(*id)
        })
    }

    fn save_note_chunk<'a>(
        &'a self,
        note_id: i64,
        chunk_index: i64,
        content: &'a str,
        spans: &'a [ByteSpan],
        embedding: Option<&'a [f32]>,
    ) -> BoxFuture<'a, Result<(), ScraperError>> {
        Box::pin(async move {
            let _ = note_id;
            self.chunks.lock().unwrap().push(NoteChunkVector {
                note_path: String::new(),
                content: content.to_owned(),
                chunk_index,
                spans: spans.to_vec(),
                embedding: embedding.unwrap_or_default().to_vec(),
            });
            Ok(())
        })
    }

    fn index_note_transactional<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
        mtime_secs: i64,
        chunks: &'a [ChunkToPersist<'a>],
    ) -> BoxFuture<'a, Result<i64, ScraperError>> {
        Box::pin(async move {
            let hash = Sha256Hex::try_from(content_hash)
                .map_err(|e| ScraperError::persistence(format!("invalid hash: {e}")))?;
            let mut notes = self.notes.lock().unwrap();
            let id = if let Some(existing) = notes.iter_mut().find(|n| n.path == path) {
                existing.content_hash = hash;
                existing.mtime_secs = mtime_secs;
                1
            } else {
                let mut id = self.next_id.lock().unwrap();
                *id += 1;
                notes.push(IndexedNoteMeta {
                    path: path.to_owned(),
                    content_hash: hash,
                    mtime_secs,
                });
                *id
            };
            drop(notes);
            let mut stored = self.chunks.lock().unwrap();
            for (i, (text, spans, emb)) in chunks.iter().enumerate() {
                stored.push(NoteChunkVector {
                    note_path: String::new(),
                    content: text.to_string(),
                    chunk_index: i as i64,
                    spans: (*spans).to_vec(),
                    embedding: emb.unwrap_or(&[]).to_vec(),
                });
            }
            Ok(id)
        })
    }

    fn load_all_vectors(&self) -> BoxFuture<'_, Result<Vec<NoteChunkVector>, ScraperError>> {
        Box::pin(async { Ok(self.chunks.lock().unwrap().clone()) })
    }

    fn note_needs_reindex<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
    ) -> BoxFuture<'a, Result<bool, ScraperError>> {
        Box::pin(async move {
            let notes = self.notes.lock().unwrap();
            match notes.iter().find(|n| n.path == path) {
                Some(n) => Ok(n.content_hash.as_str() != content_hash),
                None => Ok(true),
            }
        })
    }

    fn delete_note<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), ScraperError>> {
        Box::pin(async move {
            self.notes.lock().unwrap().retain(|n| n.path != path);
            Ok(())
        })
    }

    fn list_indexed_notes(&self) -> BoxFuture<'_, Result<Vec<IndexedNoteMeta>, ScraperError>> {
        Box::pin(async { Ok(self.notes.lock().unwrap().clone()) })
    }
}

pub(super) fn test_service(repo: Arc<InMemoryNoteRepo>) -> VaultSearchService {
    VaultSearchService::new(
        Arc::new(StubEmbedding),
        repo,
        Arc::new(StubChunker),
        // Real fs adapter over TempDir fixtures — ephemeral and
        // deterministic; the sync-behavior tests below keep exercising
        // actual filesystem reads through the port.
        Arc::new(crate::infrastructure::obsidian::VaultFsReader),
    )
}

/// Stub reader serving canned notes — proves `sync_vault` consumes the
/// injected port instead of reaching for the filesystem.
pub(super) struct CannedNoteReader {
    pub(super) notes: Vec<VaultNote>,
}

impl VaultNoteReader for CannedNoteReader {
    fn read_vault_notes(&self, _vault_path: &Path) -> Result<Vec<VaultNote>, ScraperError> {
        Ok(self.notes.clone())
    }
}

/// Stub reader that always fails — exercises error propagation through
/// the port seam.
pub(super) struct FailingNoteReader;

impl VaultNoteReader for FailingNoteReader {
    fn read_vault_notes(&self, _vault_path: &Path) -> Result<Vec<VaultNote>, ScraperError> {
        Err(ScraperError::Io(std::io::Error::other(
            "fixture: vault unreadable",
        )))
    }
}

/// #1104 — a reader that stalls 500 ms walking the vault: the real shape
/// of `WalkDir` + `fs::read` + `fs::metadata` over thousands of notes.
pub(super) struct SlowNoteReader;

impl VaultNoteReader for SlowNoteReader {
    fn read_vault_notes(&self, _vault_path: &Path) -> Result<Vec<VaultNote>, ScraperError> {
        std::thread::sleep(std::time::Duration::from_millis(500));
        Ok(Vec::new())
    }
}

/// Create a synthetic vault with `.obsidian/` marker and notes.
pub(super) fn create_test_vault(tmp: &Path, notes: &[(&str, &str)]) {
    std::fs::create_dir_all(tmp.join(".obsidian")).unwrap();
    for (name, content) in notes {
        if let Some(parent) = Path::new(name).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(tmp.join(parent)).unwrap();
            }
        }
        std::fs::write(tmp.join(name), content).unwrap();
    }
}
