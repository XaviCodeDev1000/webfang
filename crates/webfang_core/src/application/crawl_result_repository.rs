//! Append-only storage for crawl results
//!
//! Implements [`CrawlResultRepository`] using a binary append-only log file
//! with a [`DashMap`] in-memory index (URL → byte offset). A single background
//! writer runs on the blocking pool and receives writes via
//! [`mpsc::channel`] — no locks on the hot path, and no synchronous disk I/O
//! on the Tokio executor (#1121).
//!
//! ## Storage Format
//!
//! ```text
//! [4 bytes: u32 LE payload_length][N bytes: JSON ScrapedContent][1 byte: \n]
//! ```
//!
//! - `\n` terminator enables corruption detection and manual inspection
//! - Size prefix enables O(1) random access via index offset
//! - Sequential append → HDD-friendly sequential write (~120MB/s)
//!
//! The background writer runs on the blocking pool, so the `DashMap` index and
//! the `AtomicBool` error flag are the only shared state on the hot path — and
//! neither is a guard that may be held across an await. This deny fails the
//! build if a future edit ever holds a `std` lock guard across an `.await` in
//! this module (#1616 CC-S1).

#![deny(clippy::await_holding_lock)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::domain::repositories::CrawlResultRepository;
use crate::domain::{CrawlError, ScrapedContent};

#[path = "crawl_result_repository_writer.rs"]
mod crawl_result_repository_writer;

use crawl_result_repository_writer::{BackgroundWriter, WriteCommand};

#[cfg(test)]
use crawl_result_repository_writer::AppendSink;

/// Append-only storage for crawl results
///
/// Writes are sent to a background writer task via an mpsc channel.
/// Reads use the in-memory DashMap index for O(1) lookups.
pub struct CrawlResultRepositoryImpl {
    tx: mpsc::Sender<WriteCommand>,
    index: Arc<DashMap<String, u64>>,
    log_path: PathBuf,
    /// Set to true if the background writer encounters an I/O error.
    /// Subsequent save() calls will fail explicitly instead of silently
    /// accepting writes that will never be persisted.
    write_error: Arc<AtomicBool>,
    /// Handle of the blocking-pool writer thread (#1121). Taken by
    /// `shutdown()` and joined there; a writer panic is surfaced as a
    /// `JoinError` instead of being silently detached.
    writer_handle: Mutex<Option<JoinHandle<()>>>,
}

impl CrawlResultRepositoryImpl {
    /// Create a new append-only repository.
    ///
    /// Spawns a background writer task and, if the log file exists, rebuilds
    /// the index by scanning existing records.
    ///
    /// # Arguments
    ///
    /// * `log_path` - Path to the append-only log file
    /// * `buffer_capacity` - Capacity of the mpsc channel (backpressure limit)
    pub fn new(log_path: PathBuf, buffer_capacity: usize) -> Result<Self, CrawlError> {
        let (tx, rx) = mpsc::channel(buffer_capacity);
        let index = Arc::new(DashMap::new());
        let write_error = Arc::new(AtomicBool::new(false));

        // Recovery: scan existing log if present
        if log_path.exists() {
            Self::recover_index(&log_path, &index)?;
        }

        // Spawn the background writer on the BLOCKING pool (#1121): its
        // open/write syscalls must never occupy a Tokio worker, and
        // blocking-pool tasks are awaited at runtime shutdown, so buffered
        // records survive process exit. The handle is kept for `shutdown()`.
        //
        // No span is attached: the repository is process-lifetime
        // infrastructure constructed before any run-root exists, so writer
        // diagnostics must not pretend to belong to a startup span. The
        // correlated durability summary is the `shutdown()` join.
        let writer = BackgroundWriter::new(
            log_path.clone(),
            rx,
            Arc::clone(&index),
            Arc::clone(&write_error),
        );
        let writer_handle = tokio::task::spawn_blocking(move || writer.run());

        Ok(Self {
            tx,
            index,
            log_path,
            write_error,
            writer_handle: Mutex::new(Some(writer_handle)),
        })
    }

    /// Rebuild the DashMap index by scanning the log file sequentially.
    fn recover_index(path: &PathBuf, index: &DashMap<String, u64>) -> Result<(), CrawlError> {
        use std::io::Read;

        let file = std::fs::File::open(path)
            .map_err(|e| CrawlError::Storage(format!("no se pudo abrir log: {e}")))?;
        let mut reader = std::io::BufReader::new(file);
        let mut offset: u64 = 0;

        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    return Err(CrawlError::Storage(format!(
                        "lectura corrupta en offset {offset}: {e}"
                    )))
                },
            }

            let len = u32::from_le_bytes(len_buf) as usize;

            // If remaining file is too short, discard incomplete trailing record
            let mut payload = vec![0u8; len];
            if reader.read_exact(&mut payload).is_err() {
                // Partial trailing record — crash-safe: skip silently
                break;
            }

            // Skip newline
            let mut newline = [0u8; 1];
            let _ = reader.read_exact(&mut newline);

            // Extract URL from JSON to populate index
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&payload) {
                if let Some(url) = json.get("url").and_then(|u| u.as_str()) {
                    index.insert(url.to_string(), offset);
                }
            }

            offset += 4 + len as u64 + 1;
        }

        Ok(())
    }
}

impl CrawlResultRepository for CrawlResultRepositoryImpl {
    fn save(&self, content: &ScrapedContent) -> Result<(), CrawlError> {
        // Guard: if the background writer is dead, fail explicitly
        if self.write_error.load(Ordering::Relaxed) {
            return Err(CrawlError::Storage(
                "writer caído, datos no persistidos".to_string(),
            ));
        }

        let payload = serde_json::to_vec(content)
            .map_err(|e| CrawlError::Storage(format!("serialización fallida: {e}")))?;

        let url = content.url.as_str().to_string();
        self.tx
            .try_send(WriteCommand::Append { url, payload })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => {
                    CrawlError::Storage("canal lleno, backpressure".to_string())
                },
                mpsc::error::TrySendError::Closed(_) => {
                    // Writer task dropped the receiver — mark as dead
                    self.write_error.store(true, Ordering::Relaxed);
                    CrawlError::Storage("writer caído, canal cerrado".to_string())
                },
            })?;

        Ok(())
    }

    fn find_by_url(&self, url: &str) -> Result<Option<ScrapedContent>, CrawlError> {
        use std::io::{Read, Seek, SeekFrom};

        let offset = match self.index.get(url) {
            Some(entry) => *entry,
            None => return Ok(None),
        };

        let mut file = std::fs::File::open(&self.log_path)
            .map_err(|e| CrawlError::Storage(format!("no se pudo abrir log: {e}")))?;

        file.seek(SeekFrom::Start(offset))
            .map_err(|e| CrawlError::Storage(format!("seek fallido: {e}")))?;

        let mut len_buf = [0u8; 4];
        file.read_exact(&mut len_buf)
            .map_err(|e| CrawlError::Storage(format!("lectura de longitud fallida: {e}")))?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut payload = vec![0u8; len];
        file.read_exact(&mut payload)
            .map_err(|e| CrawlError::Storage(format!("lectura de payload fallida: {e}")))?;

        let result: ScrapedContent = serde_json::from_slice(&payload)
            .map_err(|e| CrawlError::Storage(format!("deserialización fallida: {e}")))?;

        Ok(Some(result))
    }

    fn get_all_urls(&self) -> Result<Vec<String>, CrawlError> {
        Ok(self.index.iter().map(|entry| entry.key().clone()).collect())
    }

    /// Load all persisted content with a single sequential log scan.
    ///
    /// Overrides the default N+1 (`get_all_urls` → `find_by_url`) loop with
    /// one forward pass over the append-only log, reusing the same record
    /// framing as `Self::recover_index`: `[4-byte LE length][JSON payload]
    /// [\n]`. A partial trailing record (torn write) stops the scan cleanly
    /// instead of erroring, preserving crash-safety.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlError::Storage`] if the log cannot be opened, a record
    /// length/payload cannot be read (mid-record corruption), or a payload
    /// fails to deserialize.
    fn load_all(&self) -> Result<Vec<ScrapedContent>, CrawlError> {
        use std::io::Read;

        // A fresh repository has no log file yet (the background writer
        // creates it asynchronously). No file means nothing persisted —
        // return empty, matching the default trait method's behavior for an
        // empty index.
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let file = std::fs::File::open(&self.log_path)
            .map_err(|e| CrawlError::Storage(format!("no se pudo abrir log: {e}")))?;
        let mut reader = std::io::BufReader::new(file);
        let mut results = Vec::with_capacity(self.index.len());
        let mut offset: u64 = 0;

        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    return Err(CrawlError::Storage(format!(
                        "lectura corrupta en offset {offset}: {e}"
                    )))
                },
            }

            let len = u32::from_le_bytes(len_buf) as usize;

            // A trailing record shorter than its declared length is a torn
            // write — stop cleanly (crash-safe) rather than erroring.
            let mut payload = vec![0u8; len];
            if reader.read_exact(&mut payload).is_err() {
                break;
            }

            // Skip the newline terminator.
            let mut newline = [0u8; 1];
            let _ = reader.read_exact(&mut newline);

            let content: ScrapedContent = serde_json::from_slice(&payload)
                .map_err(|e| CrawlError::Storage(format!("deserialización fallida: {e}")))?;
            results.push(content);

            offset += 4 + len as u64 + 1;
        }

        Ok(results)
    }

    /// Drain and join the background writer (#1121).
    ///
    /// Sends a `Shutdown` command through the same bounded channel — `send`
    /// (not `try_send`) waits for capacity, so the request lands after every
    /// buffered append — then awaits the blocking-pool writer so every write
    /// acknowledged by `save` is confirmed handed to the OS (`write(2)`)
    /// before the caller proceeds. A writer panic or I/O failure is reported,
    /// never swallowed.
    /// Idempotent: a second call finds no handle and returns `Ok`.
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = Result<(), CrawlError>> + Send + '_>> {
        Box::pin(async move {
            // If the writer is already gone the request fails; the join
            // below still reports its terminal status.
            let _send_result = self.tx.send(WriteCommand::Shutdown).await;
            // The std guard is dropped before the await — never held across
            // a suspension point.
            // Recovered: a poisoned slot still grants exclusive access, and
            // returning early here would skip waiting for the writer to flush.
            let handle = self
                .writer_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let Some(handle) = handle else {
                return Ok(());
            };
            match handle.await {
                Ok(()) => {
                    // The writer is gone: any later save would land in a dead
                    // queue, so fail explicitly from here on (#1121).
                    let io_failed = self.write_error.swap(true, Ordering::Relaxed);
                    if io_failed {
                        Err(CrawlError::Storage(
                            "el writer reportó errores de I/O durante el cierre".to_string(),
                        ))
                    } else {
                        tracing::info!("crawl-result writer drained and joined");
                        Ok(())
                    }
                },
                Err(e) => {
                    self.write_error.store(true, Ordering::Relaxed);
                    tracing::error!(error = %e, "crawl-result writer task failed during shutdown");
                    Err(CrawlError::Storage(format!(
                        "writer caído durante shutdown: {e}"
                    )))
                },
            }
        })
    }
}

// wait_for_index uses tokio::time::sleep which hangs under Miri. Stacked cfg
// attributes are the exact equivalent of all(test, not(miri)) and let
// split_tests.sh see the cfg(test) marker on the mod declaration.
#[cfg(test)]
#[cfg(not(miri))]
#[path = "crawl_result_repository_tests.rs"]
mod tests;

#[cfg(test)]
#[cfg(not(miri))]
#[path = "crawl_result_repository_writer_tests.rs"]
mod writer_tests;
