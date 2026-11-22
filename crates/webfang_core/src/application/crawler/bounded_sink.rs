//! Bounded, disk-backed [`CrawlContentSink`] for batch crawls.
//!
//! [`InMemoryContentSink`](super::content_sink::InMemoryContentSink) keeps every
//! fetched body in a `Vec` until the whole batch finishes, so a large batch of
//! heavy pages grows the resident set without any ceiling (#653).
//!
//! [`BoundedFileSink`] replaces that unbounded buffer with a bounded
//! [`tokio::sync::mpsc`] channel plus a background writer task that appends each
//! page to a JSONL spool file. Peak memory is therefore
//! `buffer_size * average_page_size` instead of `total_pages * average_page_size`,
//! and consumers stream the spool back one page at a time through
//! [`CapturedPageReader`].
//!
//! ```text
//! crawl worker ── capture() ──► mpsc(buffer_size) ─┐
//!                                                 ├─► writer task ──► spool.jsonl
//! crawl worker ── capture() ──► Backlog(max_bytes) ─┘        │
//!                                            CapturedPageReader ◄┘
//! ```
//!
//! # Where boundedness lives (#1616)
//!
//! [`CrawlContentSink::capture`] is **synchronous by contract** — it runs inline
//! on the per-page crawl worker and must not block the runtime. A synchronous
//! producer therefore cannot simply `await` its way through backpressure, and
//! the previous implementation spawned **one deferred task per full
//! `try_send`** to do it. That made the channel bound a lie: 32 bounded the
//! queue, never the number of live tasks, so a spool that could not keep up grew
//! one task — and one retained page body — per capture, forever. The resulting
//! `send().await` was also the only unguarded await in the module, so a writer
//! that stopped draining parked that task indefinitely (audit P0.2 / CC-D3).
//!
//! Both bounds now live **here, in the sink**, and nowhere else:
//!
//! | bound | owned by | limits |
//! |---|---|---|
//! | `buffer_size` | [`mpsc`] channel capacity | pages waiting to be spooled |
//! | `DEFAULT_MAX_BACKLOG_BYTES` | `backlog` | page bytes a full channel defers to |
//!
//! A full channel routes the page into the sink's own byte-bounded
//! `backlog`, which the writer drains alongside the channel. The producer
//! spawns **no task at all**, so the number of in-flight producer tasks is
//! *constant* — zero — no matter how far the writer falls behind, and there is
//! no per-send await left to park. When even the backlog ceiling is reached the
//! page is refused, counted, and reported once (see [`BoundedFileSink::dropped`]),
//! which is the same bounded-and-observed tradeoff
//! [`InMemoryContentSink`](super::content_sink::InMemoryContentSink) already
//! makes for its byte cap: correctness is preserved, memory is proven bounded.

#![deny(clippy::await_holding_lock)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn, Instrument};

use super::content_sink::{CapturedPage, CrawlContentSink};

use backlog::{Admit, Backlog};
use spool_writer::spool_writer;

/// Default number of pages held in flight before the writer applies backpressure.
pub const DEFAULT_SINK_BUFFER: usize = 32;

/// Byte ceiling for pages a full channel defers to the sink's `backlog`.
///
/// This is the second half of the sink's memory bound (#1616). Real pages
/// average tens of KiB, so 8 MiB holds a few hundred pages of slack while
/// still putting a hard, measured ceiling on a spool that cannot keep up — the
/// same order of magnitude, and the same reasoning, as
/// [`InMemoryContentSink`](super::content_sink::DEFAULT_CAPTURE_BYTE_CAP).
///
/// Deliberately **not** derived from the crawl budget tier the way the channel
/// capacity is (`build_batch_sink`, #2.5c): that tier sizes *concurrency*, while
/// this is a *memory* ceiling, which is a property of the machine rather than of
/// the crawl. Keeping it a sink-internal constant is what keeps the bound from
/// being re-decided at every call site.
pub const DEFAULT_MAX_BACKLOG_BYTES: usize = 8 * 1024 * 1024;

/// Failures of the disk-backed capture pipeline.
#[derive(Debug, thiserror::Error)]
pub enum BoundedSinkError {
    /// The spool file could not be created, written, or read.
    #[error("no se pudo escribir el archivo temporal de captura: {0}")]
    Io(#[from] std::io::Error),

    /// A spool line could not be encoded or decoded.
    #[error("registro de captura corrupto: {0}")]
    Codec(#[from] serde_json::Error),

    /// The background writer task panicked or was cancelled.
    #[error("la tarea de escritura de capturas falló: {0}")]
    Writer(String),

    /// [`BoundedFileSink::finish`] was called more than once.
    #[error("el sumidero de capturas ya fue cerrado")]
    AlreadyFinished,
}

/// Bounded, disk-backed content sink.
///
/// `capture` is synchronous (the [`CrawlContentSink`] contract) and never
/// blocks the runtime: the fast path is a non-blocking `try_send`, and a full
/// channel hands the page to the sink's own byte-bounded `backlog` instead of
/// to a task. Backpressure is therefore absorbed without dropping the body
/// (silent content loss is the failure mode #631 fixed) and without letting the
/// producer spawn work it never reclaims (#1616 P0.2).
pub struct BoundedFileSink {
    tx: Mutex<Option<mpsc::Sender<CapturedPage>>>,
    writer: Mutex<Option<JoinHandle<Result<usize, BoundedSinkError>>>>,
    backlog: Arc<Backlog>,
    cancel: CancellationToken,
    spool_path: PathBuf,
    captured: AtomicUsize,
    dropped: AtomicUsize,
    buffer: usize,
    /// Bytes of the spool handed to the OS by the writer's `flush()` — the
    /// durable-bytes watermark (WU2, #2611). Updated by the writer task after
    /// every per-page flush; read by the incremental flush to decide how far
    /// conversion may proceed. `flush()` is an OS handoff, NOT an fsync, so
    /// this watermark is best-effort by contract (design S6).
    durable_bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl std::fmt::Debug for BoundedFileSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundedFileSink")
            .field("spool_path", &self.spool_path)
            .field("captured", &self.captured.load(Ordering::Relaxed))
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl BoundedFileSink {
    /// Create a sink that spools captured pages to `spool_path`.
    ///
    /// `buffer_size` is the number of pages held in memory before the producer
    /// is slowed down; it is clamped to at least 1 because a zero-capacity
    /// `mpsc` channel is invalid. Deferred pages are bounded separately by
    /// [`DEFAULT_MAX_BACKLOG_BYTES`].
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] if the spool file (or its parent
    /// directory) cannot be created.
    pub async fn new(spool_path: PathBuf, buffer_size: usize) -> Result<Self, BoundedSinkError> {
        Self::with_backlog_bytes(spool_path, buffer_size, DEFAULT_MAX_BACKLOG_BYTES).await
    }

    /// Create a sink with an explicit byte ceiling for the deferred backlog.
    ///
    /// The seam that makes the bound testable and tunable: a spool too slow to
    /// drain holds at most `max_backlog_bytes` of page bodies beyond the
    /// channel, whatever the producer rate. A ceiling of 0 means a full channel
    /// refuses pages outright.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] if the spool file (or its parent
    /// directory) cannot be created.
    pub async fn with_backlog_bytes(
        spool_path: PathBuf,
        buffer_size: usize,
        max_backlog_bytes: usize,
    ) -> Result<Self, BoundedSinkError> {
        if let Some(parent) = spool_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::File::create(&spool_path).await?;
        let buffer = buffer_size.max(1);
        let (tx, rx) = mpsc::channel(buffer);
        let backlog = Arc::new(Backlog::new(max_backlog_bytes));
        let cancel = CancellationToken::new();
        let durable_bytes = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        // Supervised: the JoinHandle is stored in `Self.writer` and joined in
        // `finish()`, which surfaces a JoinError as BoundedSinkError::Writer.
        #[allow(clippy::disallowed_methods)]
        let writer = tokio::spawn(
            spool_writer(
                file,
                rx,
                Arc::clone(&backlog),
                cancel.clone(),
                Arc::clone(&durable_bytes),
            )
            .in_current_span(),
        );

        info!(
            spool = %spool_path.display(),
            buffer_size = buffer,
            max_backlog_bytes,
            "bounded content sink ready"
        );

        Ok(Self {
            tx: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            backlog,
            cancel,
            spool_path,
            captured: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            buffer,
            durable_bytes,
        })
    }

    /// Path of the JSONL spool holding the captured pages.
    #[must_use]
    pub fn spool_path(&self) -> &Path {
        &self.spool_path
    }

    /// Pages accepted for spooling, whether they took the channel fast path or
    /// the backlog.
    ///
    /// Excludes pages refused at the backlog ceiling (see
    /// [`dropped`](Self::dropped)), so on a successful flush
    /// `captured() == written + dropped()`.
    #[must_use]
    pub fn captured(&self) -> usize {
        self.captured.load(Ordering::Relaxed)
    }

    /// Pages refused because the backlog byte ceiling was already reached.
    ///
    /// Non-zero means the spool could not keep up with the crawl *and* the
    /// sink's memory ceiling was hit: those page bodies are not in the spool and
    /// the run exported fewer pages than were fetched. A tripped backlog latches
    /// and reports exactly one `warn!`, so this counter is how a caller learns
    /// the loss was bounded and how large it was.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Bytes of the spool the writer has handed to the OS (WU2, #2611).
    ///
    /// Every page append ends with a `flush()`, so this watermark always sits
    /// on a record boundary: re-reading the spool up to it yields only complete
    /// records. It is the offset the incremental flush converts from and the
    /// sidecar persists as `watermark_bytes`.
    ///
    /// Best-effort by contract (design S6): `flush()` hands bytes to the OS
    /// page cache, it is NOT an fsync — a power loss may still truncate the
    /// file behind this number. That residual risk is accepted and documented;
    /// the recovery paths (`recover`, OrphanSpool) treat the tail with doubt.
    #[must_use]
    pub fn durable_bytes(&self) -> u64 {
        self.durable_bytes
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Request that the background writer stop and flush what it holds.
    ///
    /// Unparks a writer idle between pages; it cannot interrupt a write that is
    /// already blocked on the filesystem. [`finish`](Self::finish) always drains
    /// to the end of the spool, so this is for shutdown, not for completing a
    /// run.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether [`cancel`](Self::cancel) has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Close the channel, wait for the writer to flush, and return the number
    /// of pages persisted to the spool.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::AlreadyFinished`] on a second call, or the
    /// writer's own I/O / codec error when the flush failed.
    pub async fn finish(&self) -> Result<usize, BoundedSinkError> {
        // Drop the sender OUTSIDE any await so the writer observes the close.
        // Nothing else holds a clone: `capture` clones per call and releases it
        // before returning, so this single drop really does close the channel.
        {
            let mut guard = self
                .tx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.take().is_none() {
                return Err(BoundedSinkError::AlreadyFinished);
            }
        }

        let handle = {
            let mut guard = self
                .writer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.take()
        };
        let Some(handle) = handle else {
            return Err(BoundedSinkError::AlreadyFinished);
        };

        match handle.await {
            Ok(result) => {
                let written = result?;
                let dropped = self.dropped.load(Ordering::Relaxed);
                info!(
                    pages = written,
                    dropped_pages = dropped,
                    spool = %self.spool_path.display(),
                    "bounded content sink flushed"
                );
                Ok(written)
            },
            // The writer task died (panic or abort): the JoinError must
            // surface as a Writer failure, never as a successful flush —
            // pinned by `a_dead_writer_makes_finish_report_the_failure`.
            Err(join_err) => {
                error!(error = %join_err, "content spool writer task failed");
                Err(BoundedSinkError::Writer(join_err.to_string()))
            },
        }
    }

    /// Open a streaming reader over the spool.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] when the spool file cannot be opened.
    pub async fn reader(&self) -> Result<CapturedPageReader, BoundedSinkError> {
        CapturedPageReader::open(&self.spool_path).await
    }
}

impl CrawlContentSink for BoundedFileSink {
    /// Record `html`, spooling it to disk without blocking or spawning.
    ///
    /// Three outcomes, in order of preference: the page goes straight into the
    /// channel; the channel is full, so it goes into the byte-bounded
    /// `backlog`; or the backlog ceiling is reached too, so the page is
    /// refused, counted, and reported once. No outcome spawns a task, so the
    /// producer's in-flight work is constant no matter how far the writer falls
    /// behind.
    ///
    /// The `#[instrument]` span declares the per-capture identity at creation
    /// time — FileTraceLayer snapshots fields in `on_new_span` (#501), so the
    /// outcome is reported as its own event rather than as a later field write.
    /// It carries no URL: `capture` runs once per page inside a `crawl_page`
    /// span that already owns the URL, and page URLs are sensitive data that
    /// must not be duplicated into every log line.
    #[instrument(
        name = "crawler.capture",
        skip_all,
        level = "debug",
        fields(sink_buffer = self.buffer, html_bytes = html.len())
    )]
    fn capture(&self, url: &str, html: &str) {
        let page = CapturedPage {
            url: url.to_string(),
            html: html.to_string(),
        };

        let Some(sender) = self.current_sender() else {
            warn!(url, "capture after sink close — page discarded");
            return;
        };

        match sender.try_send(page) {
            Ok(()) => {
                self.captured.fetch_add(1, Ordering::Relaxed);
            },
            Err(mpsc::error::TrySendError::Full(page)) => self.defer(page),
            // LCOV_EXCL_START defensive: closed-channel — capture after finish() is a lifecycle bug
            Err(mpsc::error::TrySendError::Closed(page)) => {
                warn!(url = %page.url, "capture channel closed — page discarded");
            },
            // LCOV_EXCL_STOP
        }
    }
}

impl BoundedFileSink {
    /// The live sender, or `None` once [`finish`](Self::finish) has closed the
    /// sink.
    ///
    /// The clone is deliberately short-lived: it exists for this one `try_send`
    /// and is dropped at the end of `capture`, so `finish` closing the only
    /// sender really does close the channel.
    fn current_sender(&self) -> Option<mpsc::Sender<CapturedPage>> {
        let guard = self
            .tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clone()
    }

    /// Route a page the full channel could not take into the byte-bounded
    /// backlog — or refuse and count it when even that is full.
    fn defer(&self, page: CapturedPage) {
        debug!(url = %page.url, "capture buffer full — deferring to the bounded backlog");
        match self.backlog.admit(page) {
            Admit::Queued => {
                self.captured.fetch_add(1, Ordering::Relaxed);
            },
            Admit::Saturated { first } => self.record_refused(first),
        }
    }

    /// Count a page refused at the backlog ceiling, reporting the trip once.
    fn record_refused(&self, first: bool) {
        // LCOV_EXCL_START defensive: saturated — requires a spool that cannot
        // drain; the ceiling itself is covered in the unit tests below
        self.dropped.fetch_add(1, Ordering::Relaxed);
        if first {
            let (held_pages, held_bytes) = self.backlog.occupancy();
            warn!(
                held_pages,
                held_bytes,
                max_bytes = self.backlog.max_bytes(),
                dropped_pages = self.dropped.load(Ordering::Relaxed),
                "capture backlog byte cap exceeded — further page bodies are not spooled"
            );
        }
        // LCOV_EXCL_STOP
    }
}

#[path = "bounded_sink_backlog.rs"]
mod backlog;

#[path = "bounded_sink_spool_writer.rs"]
mod spool_writer;

mod reader;

pub use reader::CapturedPageReader;

#[cfg(test)]
#[path = "bounded_sink_tests.rs"]
mod tests;
