//! Background spool writer for [`BoundedFileSink`](super::BoundedFileSink).
//!
//! The consumer side of the capture hand-off: drains both halves (the channel
//! and the byte-bounded [`backlog`](super::backlog)) and appends one JSON
//! object per line to the spool file.

#![deny(clippy::await_holding_lock)]

use std::sync::Arc;

use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::backlog::Backlog;
use super::BoundedSinkError;
use crate::application::crawler::content_sink::CapturedPage;

/// Background writer: drains both hand-off halves and appends one JSON object
/// per line.
///
/// The loop is drain-then-wait: it empties the channel and the backlog without
/// blocking, then parks on whichever half gets a page. That ordering is what
/// makes shutdown correct — a page can never sit in the backlog while the writer
/// has already concluded the stream is over, because the exit condition is
/// re-checked after every drain. Every await is cancel-safe (a `mpsc::recv` and a
/// `Notify` future are both safe to drop, and neither holds a lock), which is
/// what #1616 CC-D3 was missing.
pub(super) async fn spool_writer(
    file: tokio::fs::File,
    mut rx: mpsc::Receiver<CapturedPage>,
    backlog: Arc<Backlog>,
    cancel: CancellationToken,
    durable_bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<usize, BoundedSinkError> {
    use std::sync::atomic::Ordering;

    let mut writer = BufWriter::new(file);
    let mut written = 0usize;

    loop {
        while let Ok(page) = rx.try_recv() {
            write_page(&mut writer, &page, &mut written).await?;
            // WU2 (#2611): hand the page's bytes to the OS and publish the
            // durable position BEFORE waiting for more work. A SIGKILL between
            // two pages can then lose at most the page being written, never
            // the ones already flushed — the watermark the incremental flush
            // converts from stays honest.
            writer.flush().await?;
            durable_bytes.store(
                writer.get_ref().metadata().await.map_or(0, |m| m.len()),
                Ordering::Release,
            );
        }
        for page in backlog.drain() {
            write_page(&mut writer, &page, &mut written).await?;
            // Same per-page durability as the channel path above: the backlog
            // drains page by page, so the watermark advances with it.
            writer.flush().await?;
            durable_bytes.store(
                writer.get_ref().metadata().await.map_or(0, |m| m.len()),
                Ordering::Release,
            );
        }

        // Only a closed channel can end the stream, and only once it is closed,
        // EMPTY, and the backlog it fed is empty. `is_closed()` alone is not
        // enough: closing makes the channel refuse new sends, it does not
        // discard what is already buffered. A page that arrives between the
        // drain above and this check is still queued, and breaking here would
        // drop it with the receiver — a silent loss of exactly the kind #631
        // exists to prevent.
        //
        // Ordering is what makes this race-free rather than merely hopeful:
        // once `is_closed()` is true every sender is gone, so no further `send`
        // or `admit` can run and both emptiness checks are stable. Checking
        // `is_closed()` first is therefore sufficient to freeze the channel
        // before reading the two lengths.
        if rx.is_closed() && rx.is_empty() && backlog.is_empty() {
            break;
        }

        tokio::select! {
            biased;
            next = rx.recv() => {
                if let Some(page) = next {
                    write_page(&mut writer, &page, &mut written).await?;
                    writer.flush().await?;
                    durable_bytes.store(
                        writer.get_ref().metadata().await.map_or(0, |m| m.len()),
                        Ordering::Release,
                    );
                }
                // `None`: every sender is gone. Loop to drain the backlog and
                // re-check the exit condition rather than breaking here.
            },
            _ = backlog.notified() => {}
            _ = cancel.cancelled() => {
                let pending = backlog.occupancy().0;
                warn!(written, pending_pages = pending, "capture spool write cancelled — flushing what was persisted");
                break;
            },
        }
    }

    writer.flush().await?;
    writer.into_inner().sync_all().await?;
    Ok(written)
}

/// Append one page as a single JSONL line and count it.
async fn write_page(
    writer: &mut BufWriter<tokio::fs::File>,
    page: &CapturedPage,
    written: &mut usize,
) -> Result<(), BoundedSinkError> {
    let line = serde_json::to_string(page)?;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    *written += 1;
    Ok(())
}
