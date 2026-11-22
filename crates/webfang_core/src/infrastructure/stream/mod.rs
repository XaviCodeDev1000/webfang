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

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::clock::{SystemUtcClock, UtcClock};
use crate::domain::repository::VectorRepository;
use crate::domain::text_chunker::ByteSpan;
use crate::domain::{Sha256Hex, ValidUrl};
use crate::error::ScraperError;

/// A single JSONL vector record emitted by [`StreamRepository`].
///
/// Mirrors the fields surfaced by the elastic ingestion pipeline: the source
/// URL, the content `sha256_hex` (content-hash dedup key), an optional title,
/// the cleaned `chunk_text`, the raw `embedding` vector, arbitrary `metadata`,
/// and an RFC3339 `timestamp`.
///
/// Both key fields are validated newtypes (#1118): `url` carries the
/// `ValidUrl` hardening (http(s) only, credentials stripped) and
/// `sha256_hex` is a real 64-char lowercase digest — the malformed dedup
/// keys that used to pass as raw `String`s are rejected at the sink
/// boundary and at (de)serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorRecord {
    /// Source URL the chunk was extracted from.
    pub url: ValidUrl,
    /// Content hash (SHA-256 hex) of the resource — the dedup key.
    pub sha256_hex: Sha256Hex,
    /// Best-effort title (first ≤200 chars of the first chunk line), or `null`.
    pub title: Option<String>,
    /// Cleaned chunk text.
    pub chunk_text: String,
    /// Raw embedding vector (e.g. 384 floats for all-MiniLM-L6-v2). Empty only
    /// when the record would have been omitted (Q3).
    pub embedding: Vec<f32>,
    /// Stable identifier of the model that produced `embedding` (#2429), or
    /// `null` when the run had no model identity available. Vectors from
    /// different models are never comparable: consumers must group by this
    /// field before running similarity search.
    #[serde(default)]
    pub embedding_model: Option<String>,
    /// Dimensionality declared for `embedding_model` (#2429), or `null`.
    /// A record whose `embedding.len()` disagrees with this is rejected at
    /// both the read ([`VectorRecord::from_line`]) and the write boundary.
    #[serde(default)]
    pub embedding_dim: Option<usize>,
    /// Zero-based position of the chunk within its source document (#2429),
    /// as handed to [`VectorRepository::save_chunk`] by the ingestion
    /// pipeline. Lets a consumer reconstruct document order from an unordered
    /// result set.
    #[serde(default)]
    pub chunk_index: Option<i64>,
    /// First byte of the chunk's region inside the source document (#2429
    /// AC2), half-open and paired with `byte_end`: the enclosing range of the
    /// chunk's spans in the text the chunker segmented. `null` when the run
    /// carried no locator information. Offsets are bytes (sliceable, UTF-8
    /// safe), not chars — the same unit the `ByteSpan` contract uses.
    #[serde(default)]
    pub byte_start: Option<u64>,
    /// Byte after the last byte of the chunk's region (#2429 AC2); paired
    /// with `byte_start`. `from_line` rejects a range with `byte_start >
    /// byte_end`, or exactly one of the two present.
    #[serde(default)]
    pub byte_end: Option<u64>,
    /// Optional arbitrary metadata (reserved for future use).
    #[serde(default)]
    pub metadata: Value,
    /// RFC3339 timestamp of emission.
    pub timestamp: String,
}

/// Model identity attached to every record a [`StreamRepository`] emits (#2429).
///
/// Captured once, at sink construction, from the container's
/// [`EmbeddingPort`](crate::domain::embedding_port::EmbeddingPort): one run
/// embeds with exactly one model, so the identity is a property of the sink,
/// not of each chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProvenance {
    /// Stable model identifier (e.g. `"granite-97m"`, or the
    /// provider-declared model name for remote endpoints).
    pub model_id: String,
    /// Dimensionality the model emits. The sink rejects any vector whose
    /// length disagrees with this.
    pub embedding_dim: usize,
}

impl VectorRecord {
    /// Parse and validate one JSONL line — the read boundary of the format.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Serialization`] for malformed JSON, and
    /// [`ScraperError::Ingestion`] when a present `embedding_dim` disagrees
    /// with `embedding.len()` (#2429), or when the byte range is inverted or
    /// half-present (#2429 AC2): such a record cannot name a real region of
    /// any document and must not enter a store.
    pub fn from_line(line: &str) -> Result<Self, ScraperError> {
        let record: Self = serde_json::from_str(line)?;
        if let Some(dim) = record.embedding_dim {
            if record.embedding.len() != dim {
                return Err(ScraperError::Ingestion(format!(
                    "el record declara embedding_dim {dim} pero su vector tiene {} dimensiones",
                    record.embedding.len()
                )));
            }
        }
        match (record.byte_start, record.byte_end) {
            (Some(start), Some(end)) if start > end => {
                return Err(ScraperError::Ingestion(format!(
                    "el record declara un rango de bytes invertido: byte_start {start} > byte_end {end}"
                )));
            },
            (Some(_), None) | (None, Some(_)) => {
                return Err(ScraperError::Ingestion(
                    "el record debe declarar byte_start y byte_end juntos, o ninguno de los dos"
                        .to_string(),
                ));
            },
            _ => {},
        }
        Ok(record)
    }
}

/// Where a [`StreamRepository`] writes (#1118).
///
/// The `"-"` stdout sentinel used to be a magic string compared inside the
/// constructor; it is now a variant, and an empty path is rejected at the
/// boundary instead of reaching `File::create("")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkPath {
    /// Buffered stdout — the wire sentinel is `"-"`.
    Stdout,
    /// A file path, created (truncated) by the sink.
    File(std::path::PathBuf),
}

impl SinkPath {
    /// Parse the CLI wire form (`--output-vectors <path|->`).
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`] for an empty or whitespace-only
    /// path — there is no valid sink there.
    pub fn parse(raw: &str) -> Result<Self, ScraperError> {
        if raw == "-" {
            return Ok(Self::Stdout);
        }
        if raw.trim().is_empty() {
            return Err(ScraperError::Config(
                "la ruta de salida de vectores no puede estar vacía".to_string(),
            ));
        }
        Ok(Self::File(std::path::PathBuf::from(raw)))
    }
}

impl std::str::FromStr for SinkPath {
    type Err = ScraperError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

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
