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
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::domain::repositories::CrawlResultRepository;
use crate::domain::{CrawlError, ScrapedContent};

enum WriteCommand {
    Append {
        url: String,
        payload: Vec<u8>,
    },
    /// Graceful stop: drain everything already queued, then exit (#1121).
    Shutdown,
}

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

/// Background writer task that processes write commands sequentially.
struct BackgroundWriter {
    rx: mpsc::Receiver<WriteCommand>,
    index: Arc<DashMap<String, u64>>,
    log_path: PathBuf,
    write_error: Arc<AtomicBool>,
}

/// Byte sink that can also report its current end-of-file size.
///
/// `std::fs::File` satisfies this through `metadata()`; test stubs provide a
/// deterministic in-memory size. `append_record` needs it to resync the
/// tracked offset to the REAL EOF after a frame fails mid-write (partial
/// `write_all`), which raw [`Write`] cannot report (#1143 review).
trait AppendSink: Write {
    /// Current number of bytes in the sink, or `None` if it cannot be
    /// determined (the caller must then fall back to the last known offset).
    fn size(&self) -> Option<u64>;
}

impl AppendSink for std::fs::File {
    fn size(&self) -> Option<u64> {
        self.metadata().ok().map(|m| m.len())
    }
}

impl BackgroundWriter {
    fn new(
        log_path: PathBuf,
        rx: mpsc::Receiver<WriteCommand>,
        index: Arc<DashMap<String, u64>>,
        write_error: Arc<AtomicBool>,
    ) -> Self {
        Self {
            rx,
            index,
            log_path,
            write_error,
        }
    }

    /// Blocking-pool writer loop (#1121).
    ///
    /// Runs on a dedicated blocking thread: `blocking_recv` parks THIS OS
    /// thread (never a Tokio worker) while the channel is empty, and every
    /// filesystem syscall below stays off the executor. The loop exits when
    /// the channel closes — all senders dropped (process exit) or after a
    /// `Shutdown` command has drained the queue — so buffered records are
    /// always written before the writer leaves.
    fn run(mut self) {
        // The log file is opened lazily on the first actual write (issue #606):
        // when nothing is ever persisted we must not litter the CWD with an
        // empty `output/` directory and a 0-byte `crawl_results.bin`.
        let mut file: Option<std::fs::File> = None;
        // Byte offset of the next append, tracked locally (#1121): replaces
        // the per-record `metadata().unwrap_or(0)` lie with one honest stat
        // at open time plus arithmetic per frame.
        let mut offset: u64 = 0;

        while let Some(cmd) = self.rx.blocking_recv() {
            match cmd {
                WriteCommand::Append { url, payload } => {
                    if self
                        .append_cmd(&mut file, &mut offset, url, &payload)
                        .is_err()
                    {
                        return;
                    }
                },
                WriteCommand::Shutdown => {
                    // Drain commands that raced in after the shutdown request,
                    // then exit. `try_recv` never blocks, so the drain is
                    // bounded by what senders have already committed.
                    while let Ok(WriteCommand::Append { url, payload }) = self.rx.try_recv() {
                        if self
                            .append_cmd(&mut file, &mut offset, url, &payload)
                            .is_err()
                        {
                            return;
                        }
                    }
                    break;
                },
            }
        }
        // No exit log: the repository is constructed once per process, before
        // any run-root span exists, so a terminal event here would mint an
        // orphan `trace_id` and break the single-trace invariant asserted by
        // `trace_orphan_spawn_test`. The correlated summary lives in
        // `shutdown()` (caller's span); I/O failures still log from inside
        // the loop.
    }

    /// Ensure the log is open, then append one framed record.
    ///
    /// `Err(())` signals a fatal open/stat failure that was already reported
    /// through `write_error`; the caller must bail out of the loop.
    fn append_cmd(
        &self,
        file: &mut Option<std::fs::File>,
        offset: &mut u64,
        url: String,
        payload: &[u8],
    ) -> Result<(), ()> {
        if file.is_none() {
            match self.open_log() {
                Ok(f) => match f.metadata() {
                    Ok(m) => {
                        *offset = m.len();
                        *file = Some(f);
                    },
                    Err(e) => {
                        tracing::error!("failed to stat log for initial offset: {e}");
                        self.write_error.store(true, Ordering::Relaxed);
                        return Err(());
                    },
                },
                Err(()) => return Err(()),
            }
        }
        if let Some(f) = file.as_mut() {
            self.append_record(f, offset, url, payload);
        }
        Ok(())
    }

    /// Create the parent directory (if any) and open the log file for appending.
    /// On any failure, marks the writer as errored and returns `Err(())` so the
    /// caller can bail out of the writer loop.
    fn open_log(&self) -> Result<std::fs::File, ()> {
        // H6 FIX: Create parent directory before opening log file
        if let Some(parent) = self.log_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::error!("failed to create log directory: {e}");
                self.write_error.store(true, Ordering::Relaxed);
                return Err(());
            }
        }

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
        {
            Ok(f) => Ok(f),
            Err(e) => {
                tracing::error!("failed to open log for writing: {e}");
                self.write_error.store(true, Ordering::Relaxed);
                Err(())
            },
        }
    }

    /// Append a single framed record (`[len][payload][\n]`) to the log and index
    /// the URL at its byte offset. Any write failure marks the writer errored.
    ///
    /// Takes `&mut dyn AppendSink` so the framing is testable against failure
    /// stubs; the offset is tracked by the caller so no per-record `stat`
    /// syscall is needed. On a failed frame the sink's real size is consulted
    /// to resync the offset: a partial `write_all` leaves bytes on disk that
    /// the arithmetic offset would otherwise never account for, corrupting
    /// every subsequent index entry (#1143 review).
    fn append_record(
        &self,
        file: &mut dyn AppendSink,
        offset: &mut u64,
        url: String,
        payload: &[u8],
    ) {
        let len = payload.len() as u32;

        if let Err(e) = Self::write_frame(file, len, payload) {
            tracing::error!(error = %e, path = %self.log_path.display(), "error writing record to log");
            self.write_error.store(true, Ordering::Relaxed);
            // Resync to the real EOF so frames still queued in the channel —
            // which the writer keeps processing — index at honest offsets.
            // If the size cannot be determined, keep the current offset:
            // `save` already refuses new work once `write_error` is set.
            *offset = file.size().unwrap_or(*offset);
            return;
        }

        self.index.insert(url, *offset);
        *offset += 4 + u64::from(len) + 1;
    }

    /// Write one framed record `[len][payload][\n]` and flush the sink.
    ///
    /// Durability note (#1143 review): for the production sink
    /// (`std::fs::File`) `flush` is a documented no-op — process-exit
    /// durability comes from `write(2)` returning Ok (the bytes are in the
    /// kernel page cache), not from flush; power-loss durability would need
    /// `sync_all`, which is deliberately not paid for here. Propagating the
    /// flush error is still required because `append_record` is generic over
    /// the sink and a buffered wrapper could legitimately fail on it: a
    /// failed frame must never be indexed as durable.
    fn write_frame(
        file: &mut dyn AppendSink,
        len: u32,
        payload: &[u8],
    ) -> Result<(), std::io::Error> {
        file.write_all(&len.to_le_bytes())?;
        file.write_all(payload)?;
        file.write_all(b"\n")?;
        file.flush()
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
