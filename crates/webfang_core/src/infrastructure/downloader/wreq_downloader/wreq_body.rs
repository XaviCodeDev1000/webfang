//! The bounded body-read entry points of [`WreqDownloader`](super::WreqDownloader).
//! Their own module so the downloader stays under its grandfathered size.

use super::*;
use crate::domain::error::cause::FetchPhase;
use crate::domain::error::mapping::with_cause;

impl WreqDownloader {
    /// Read the response body with a HARD cap on the decompressed byte count
    /// (FIX-1, #1231 F-12).
    ///
    /// Replaces the unbounded `response.text()`: the body is consumed as a
    /// stream of already-decompressed chunks and the read aborts as soon as
    /// the accumulated size exceeds the configured `max_page_bytes`, so
    /// memory stays bounded at ~cap + one chunk regardless of the wire
    /// content (the audit's gzip bomb inflated 60 MB from a tiny
    /// Content-Length). Charset handling mirrors wreq's `text_with_charset`:
    /// the Content-Type charset param wins, UTF-8 as fallback.
    pub(super) async fn read_body_capped(
        &self,
        response: wreq::Response,
    ) -> Result<String, DownloadError> {
        self.read_body_bounded(response, self.max_page_bytes, BoundedRead::Abort)
            .await
            // #2784: the BODY phase — a timeout here is a READ timeout.
            .map_err(|error| with_cause(error, FetchPhase::Body))
    }

    /// Read at most [`WAF_SNIFF_MAX_BYTES`] of a response body for challenge
    /// inspection (F-11).
    ///
    /// Truncates rather than failing: unlike a page fetch, exceeding the budget here is
    /// the expected case — the caller only wants the prefix that carries marker prose.
    pub(super) async fn read_body_snippet(
        &self,
        response: wreq::Response,
    ) -> Result<String, DownloadError> {
        self.read_body_bounded(response, WAF_SNIFF_MAX_BYTES, BoundedRead::Truncate)
            .await
    }
}
