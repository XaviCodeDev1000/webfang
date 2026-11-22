//! Background writer and append-only framing for the crawl-result log.
//!
//! The writer owns the `mpsc` receiver, the byte offset and the framed record
//! format. It runs on the blocking pool; all lock/offset decisions here are
//! local to this module so the repository hub keeps only the port surface and
//! the shutdown rendezvous.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::mpsc;

pub(super) enum WriteCommand {
    Append {
        url: String,
        payload: Vec<u8>,
    },
    /// Graceful stop: drain everything already queued, then exit (#1121).
    Shutdown,
}

/// Background writer task that processes write commands sequentially.
pub(super) struct BackgroundWriter {
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
pub(super) trait AppendSink: Write {
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
    pub(super) fn new(
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
    pub(super) fn run(mut self) {
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
    pub(super) fn append_record(
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
