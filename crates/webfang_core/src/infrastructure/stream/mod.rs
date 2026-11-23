//! Headless JSONL vector sink — dependency-free RAG export (core-slimming, T3-C).
//!
//! `StreamRepository` implements [`VectorRepository`] by emitting one JSON line
//! per chunk that carries an embedding. It has **no** SQLite / `rusqlite` /
//! `sys-info` dependency, so it compiles into the lightweight `webfang_core`
//! binary and backs `--output-vectors <path|->` (spec R2 / S2.1).
//!
//! # Record shape
//!
//! Each emitted JSONL line is a [`VectorRecord`]:
//!
//! ```json
//! {"url":"...","sha256_hex":"<64 hex>","title":null,
//!  "chunk_text":"...","embedding":[0.01, … 384 floats …],
//!  "embedding_model":"granite-97m","embedding_dim":384,"chunk_index":0,
//!  "byte_start":1024,"byte_end":4096,
//!  "metadata":null,"timestamp":"2026-07-11T12:00:00Z"}
//! ```
//!
//! The embedding array is the **raw 384-dim** vector (no rounding, no base64) so
//! downstream RAG pipelines can ingest it directly.
//!
//! # Schema compatibility (#2429)
//!
//! `embedding_model`, `embedding_dim` and `chunk_index` are **additive
//! optional fields** (`#[serde(default)]`): lines written before #2429 still
//! deserialize, with the new fields reading as `null`. There is no format
//! version bump. New writers always emit `chunk_index` and `byte_start`/
//! `byte_end` (the enclosing byte range of the chunk's spans in the text the
//! chunker segmented; `null` when the run had no locator information), and
//! emit `embedding_model`/`embedding_dim` whenever the run wired an embedding
//! port into the container; a `null` there means "this run had
//! no model identity available" and downstream consumers must treat such
//! vectors as incomparable with any provenanced vector. A record whose
//! `embedding.len()` contradicts its `embedding_dim` is rejected at both
//! boundaries ([`VectorRecord::from_line`] on read, the sink on write).
//!
//! # Key decisions (T3-C design)
//!
//! - **Q3 (ai OFF):** when a chunk has `embedding = None`, the record is *omitted*
//!   (no line is written) — we never emit a null/zero vector.
//! - **D2 (broken pipe):** a write / flush `io::Error` (incl. `WriteZero` from a
//!   closed pipe) is returned as a fatal [`ScraperError::Io`], which propagates
//!   out of `ElasticIngestion::run` and aborts the crawl.
//! - **Concurrency (#1119):** the writer and title cache live behind `Arc<Mutex<…>>`
//!   and every lock acquisition + blocking `write_all`/`flush` runs inside
//!   `spawn_blocking`, so the JSONL stream stays line-oriented under concurrent
//!   [`crate::application::elastic_ingestion::ElasticIngestion`] processing
//!   without ever parking a Tokio worker on a std lock or a disk syscall.
//!   A poisoned lock maps to a typed `ScraperError::Io` — never a panic inside
//!   the async sink.

use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::domain::clock::{SystemUtcClock, UtcClock};
use crate::domain::repository::VectorRepository;
use crate::domain::text_chunker::ByteSpan;
use crate::domain::{Sha256Hex, ValidUrl};
use crate::error::ScraperError;

mod record;

pub use record::{EmbeddingProvenance, SinkPath, VectorRecord};

/// Headless vector sink that writes [`VectorRecord`] lines to a JSONL stream.
///
/// Construct with [`StreamRepository::new`] from a validated [`SinkPath`].
///
/// # Executor hygiene (#1119)
///
/// Both locks are `std::sync::Mutex`es wrapped in `Arc` so the synchronous
/// critical sections (HashMap ops, `write_all`/`flush` syscalls) can be moved
/// into `spawn_blocking`. No lock is ever acquired on a Tokio worker thread,
/// and no `.await` happens while a guard is held. Poisoned locks surface as
/// `ScraperError::Io`, never as a panic inside the sink future.
pub struct StreamRepository {
    /// Serialized JSONL writer. `Box<dyn Write + Send>` so both `Stdout` and
    /// `File` fit behind one type; the `Arc<Mutex<…>>` keeps the stream
    /// line-oriented under concurrent ingestion and lets the write section
    /// run on the blocking pool (#1119).
    writer: Arc<Mutex<std::io::BufWriter<Box<dyn Write + Send>>>>,
    /// Title cache keyed by resource URL, populated by [`VectorRepository::save_resource`]
    /// and read by [`VectorRepository::save_chunk`] (the chunk call only receives
    /// `resource_url`, not the title).
    titles: Arc<Mutex<HashMap<String, String>>>,
    /// Model identity stamped into every emitted record (#2429). `None` when
    /// the run wired no `EmbeddingPort` — records then carry `null`
    /// provenance fields.
    provenance: Option<Arc<EmbeddingProvenance>>,
    /// Injected clock for deterministic timestamps in tests.
    clock: Arc<dyn UtcClock>,
}

/// Map a poisoned lock to a typed I/O error instead of panicking inside the
/// async sink (#1119 — "0 `.expect` por poison en async").
fn lock_error(lock: &str) -> ScraperError {
    ScraperError::Io(std::io::Error::other(format!(
        "vector sink lock poisoned: {lock}"
    )))
}

impl StreamRepository {
    /// Open the JSONL sink with the system clock.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Io`] if the file cannot be created.
    pub fn new(path: SinkPath) -> Result<Self, ScraperError> {
        Self::with_clock(path, Arc::new(SystemUtcClock))
    }

    /// Open the JSONL sink with an injected clock.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Io`] if the file cannot be created.
    pub fn with_clock(path: SinkPath, clock: Arc<dyn UtcClock>) -> Result<Self, ScraperError> {
        Self::with_clock_and_provenance(path, clock, None)
    }

    /// Open the JSONL sink stamping `provenance` into every record (#2429).
    ///
    /// One run embeds with exactly one model, so the identity is captured
    /// once, here, from the container's `EmbeddingPort`.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Io`] if the file cannot be created.
    pub fn new_with_provenance(
        path: SinkPath,
        provenance: Option<EmbeddingProvenance>,
    ) -> Result<Self, ScraperError> {
        Self::with_clock_and_provenance(path, Arc::new(SystemUtcClock), provenance)
    }

    /// Open the JSONL sink with an injected clock and record provenance.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Io`] if the file cannot be created.
    pub fn with_clock_and_provenance(
        path: SinkPath,
        clock: Arc<dyn UtcClock>,
        provenance: Option<EmbeddingProvenance>,
    ) -> Result<Self, ScraperError> {
        let boxed: Box<dyn Write + Send> = match path {
            SinkPath::Stdout => Box::new(std::io::stdout()),
            SinkPath::File(file) => {
                if let Some(parent) = file.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        ScraperError::Io(std::io::Error::new(
                            e.kind(),
                            format!("no se pudo crear el directorio '{}': {e}", parent.display()),
                        ))
                    })?;
                }
                Box::new(std::fs::File::create(&file).map_err(|e| {
                    ScraperError::Io(std::io::Error::new(
                        e.kind(),
                        format!(
                            "no se pudo crear el archivo de vectores '{}': {e}",
                            file.display()
                        ),
                    ))
                })?)
            },
        };
        Ok(Self {
            writer: Arc::new(Mutex::new(std::io::BufWriter::new(boxed))),
            titles: Arc::new(Mutex::new(HashMap::new())),
            provenance: provenance.map(Arc::new),
            clock,
        })
    }

    /// Build a sink over an arbitrary writer.
    ///
    /// Used by tests to inject deterministic, failure-simulating writers
    /// (e.g. a broken-pipe stub) without touching the filesystem or stdout.
    /// The wrapping matches [`StreamRepository::new`] exactly.
    #[cfg(test)]
    pub(crate) fn from_writer(w: Box<dyn Write + Send>) -> Self {
        Self::from_writer_with_provenance(w, None)
    }

    /// Build a sink over an arbitrary writer with record provenance (#2429).
    #[cfg(test)]
    pub(crate) fn from_writer_with_provenance(
        w: Box<dyn Write + Send>,
        provenance: Option<EmbeddingProvenance>,
    ) -> Self {
        Self {
            writer: Arc::new(Mutex::new(std::io::BufWriter::new(w))),
            titles: Arc::new(Mutex::new(HashMap::new())),
            provenance: provenance.map(Arc::new),
            clock: Arc::new(SystemUtcClock),
        }
    }
}

impl VectorRepository for StreamRepository {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        _content_hash: &'a str,
        _size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        let titles = Arc::clone(&self.titles);
        let url = url.to_string();
        let title = title.to_string();
        Box::pin(async move {
            if !title.is_empty() {
                // #1119: the HashMap insert runs on the blocking pool — the
                // std lock is never acquired on a Tokio worker, and a
                // poisoned lock is a typed error, not a panic.
                let key = url.clone();
                let join = tokio::task::spawn_blocking(move || -> Result<(), ScraperError> {
                    let mut cache = titles.lock().map_err(|_| lock_error("title cache"))?;
                    cache.insert(key, title);
                    Ok(())
                })
                .await;
                join.map_err(|e| {
                    ScraperError::Io(std::io::Error::other(format!(
                        "title cache blocking task join failed: {e}"
                    )))
                })??;
            }
            Ok(url)
        })
    }

    fn save_chunk<'a>(
        &'a self,
        id: &'a str,
        resource_url: &'a str,
        chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
        spans: Option<&'a [ByteSpan]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        // Q3: without an embedding there is nothing to vectorize — omit the
        // record rather than emit a null/zero vector.
        let embedding = match embedding {
            Some(e) => e.to_vec(),
            None => return Box::pin(async { Ok(()) }),
        };

        // #2429 AC2: the record's byte range is the enclosing range of the
        // chunk's spans (first span start, last span end). An empty span set
        // carries no locator information — emitted as `null`, never as a
        // fabricated 0.
        let byte_range = spans.and_then(|s| match (s.first(), s.last()) {
            (Some(first), Some(last)) => Some((first.start() as u64, last.end() as u64)),
            _ => None,
        });

        // #2429: a vector whose length contradicts the sink's declared model
        // dimension is silently unusable for similarity search — reject it
        // here instead of writing an incomparable record.
        if let Some(provenance) = &self.provenance {
            if embedding.len() != provenance.embedding_dim {
                return Box::pin(async move {
                    Err(ScraperError::Ingestion(format!(
                        "el chunk {chunk_index} de '{resource_url}' tiene un vector de {} dimensiones \
                         pero el modelo '{}' declara {}",
                        embedding.len(),
                        provenance.model_id,
                        provenance.embedding_dim
                    )))
                });
            }
        }

        let titles = Arc::clone(&self.titles);
        let writer = Arc::clone(&self.writer);
        let clock = Arc::clone(&self.clock);
        let provenance = self.provenance.clone();
        let id = id.to_string();
        let resource_url = resource_url.to_string();
        let content = content.to_string();

        Box::pin(async move {
            // #1119: title lookup, record build, JSON serialization and the
            // `write_all`/`flush` syscalls all run inside ONE blocking-pool
            // section. The writer `Mutex` serializes the two `write_all` +
            // `flush` calls so the JSONL stream stays line-oriented even
            // under concurrent `ElasticIngestion`, and no std lock or disk
            // write ever parks a Tokio worker.
            tokio::task::spawn_blocking(move || -> Result<(), ScraperError> {
                // The chunk id is formatted as "{sha256_hex}-{index}" by
                // `ElasticIngestion::run`, so the hash is the segment before the
                // first '-' (a SHA-256 hex string contains no '-'). Both key
                // fields are validated at this boundary (#1118): a malformed
                // dedup key or a non-fetchable URL aborts the write instead of
                // corrupting the stream.
                let sha256_hex = Sha256Hex::try_from(id.split('-').next().unwrap_or(id.as_str()))?;
                let url = ValidUrl::parse(&resource_url)?;

                let title = titles
                    .lock()
                    .map_err(|_| lock_error("title cache"))?
                    .get(&resource_url)
                    .cloned();

                let record = VectorRecord {
                    url,
                    sha256_hex,
                    title,
                    chunk_text: content,
                    embedding,
                    embedding_model: provenance.as_ref().map(|p| p.model_id.clone()),
                    embedding_dim: provenance.as_ref().map(|p| p.embedding_dim),
                    chunk_index: Some(chunk_index),
                    byte_start: byte_range.map(|r| r.0),
                    byte_end: byte_range.map(|r| r.1),
                    metadata: Value::Null,
                    timestamp: clock.now().to_rfc3339(),
                };

                let line = serde_json::to_string(&record)?;
                // D2: a broken pipe / WriteZero must surface as a fatal Io error so
                // the crawl aborts. `?` converts io::Error → ScraperError::Io.
                let mut writer = writer.lock().map_err(|_| lock_error("vector stream"))?;
                writer.write_all(line.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
                Ok(())
            })
            .await
            .map_err(|e| {
                ScraperError::Io(std::io::Error::other(format!(
                    "vector sink blocking task join failed: {e}"
                )))
            })?
        })
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        _content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        // No dedup for the stream sink — every chunk is emitted.
        Box::pin(async move { Ok(None) })
    }

    fn get_vector<'a>(
        &'a self,
        _chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(None) })
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
