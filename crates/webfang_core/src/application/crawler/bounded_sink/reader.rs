//! Streaming reader over a spool written by
//! [`BoundedFileSink`](super::BoundedFileSink).

#![deny(clippy::await_holding_lock)]

use std::path::Path;

use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};

use super::BoundedSinkError;
use crate::application::crawler::content_sink::CapturedPage;

/// Streaming reader over a spool written by [`BoundedFileSink`](super::BoundedFileSink).
///
/// Yields one [`CapturedPage`] at a time so the consumer never materializes the
/// whole batch in memory, and reports the exact byte position it reached so a
/// consumer can resume later (`open_at`) or checkpoint a watermark.
///
/// Bytes are counted through [`AsyncBufReadExt::read_until`], so
/// [`Self::consumed_bytes`] always lands on a line boundary: re-opening at it
/// never splits a record and never re-reads one. Invalid UTF-8 in a line
/// surfaces as [`BoundedSinkError::Codec`] (a corrupt record), not as an I/O
/// error — a consumer that skips corrupt records can therefore keep going.
#[derive(Debug)]
pub struct CapturedPageReader {
    inner: BufReader<tokio::fs::File>,
    start_offset: u64,
    consumed: u64,
}

impl CapturedPageReader {
    /// Open the spool at `path` from its beginning.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] when the file cannot be opened.
    pub async fn open(path: &Path) -> Result<Self, BoundedSinkError> {
        Self::open_at(path, 0).await
    }

    /// Open the spool at `path` starting from the absolute byte `offset` —
    /// a watermark previously reported by [`Self::consumed_bytes`]. Only
    /// offsets handed out by this type (or `0`) are meaningful: seeking
    /// anywhere else makes the first parse fail as
    /// [`BoundedSinkError::Codec`].
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] when the file cannot be opened or
    /// the seek fails.
    pub async fn open_at(path: &Path, offset: u64) -> Result<Self, BoundedSinkError> {
        let mut file = tokio::fs::File::open(path).await?;
        if offset > 0 {
            AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(offset)).await?;
        }
        Ok(Self {
            inner: BufReader::new(file),
            start_offset: offset,
            consumed: 0,
        })
    }

    /// Absolute byte position the reader reached: every byte of every line
    /// returned so far, including newlines, blank lines, and corrupt
    /// records. After a mid-record I/O failure this value is NOT a safe
    /// watermark — the failed read may have consumed an unknown number of
    /// bytes past the last line boundary it could vouch for.
    #[must_use]
    pub fn consumed_bytes(&self) -> u64 {
        self.start_offset + self.consumed
    }

    /// Read the next captured page, or `None` at end of spool.
    ///
    /// Blank lines are skipped; their bytes still count toward
    /// [`Self::consumed_bytes`]. A line that is not a valid record fails
    /// with [`BoundedSinkError::Codec`], and its bytes are consumed either
    /// way — the caller may keep reading after one.
    ///
    /// # Errors
    ///
    /// Returns [`BoundedSinkError::Io`] on a read failure or
    /// [`BoundedSinkError::Codec`] when a line is not a valid record.
    pub async fn next_page(&mut self) -> Result<Option<CapturedPage>, BoundedSinkError> {
        loop {
            let mut line = Vec::with_capacity(1024);
            let read = self.inner.read_until(b'\n', &mut line).await?;
            if read == 0 {
                return Ok(None);
            }
            self.consumed += read as u64;
            while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
                line.pop();
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return serde_json::from_slice(&line)
                .map(Some)
                .map_err(BoundedSinkError::Codec);
        }
    }
}
