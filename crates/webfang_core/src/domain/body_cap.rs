//! Body reading utilities with size caps
//!
//! Provides helpers to read HTTP response bodies with a maximum size limit
//! to prevent memory exhaustion from large or gzip-bomb responses.
//!
//! This domain-owned helper mirrors the implementation in
//! `infrastructure::downloader::wreq_downloader::read_body_capped` but
//! returns `HttpError` instead of `DownloadError` for use by the
//! application HTTP client.

use std::time::Instant;

use bytes::BytesMut;
use encoding_rs::{Encoding, UTF_8};
use futures::StreamExt;
use tracing::warn;
use wreq::Response;

use crate::domain::error::cause::FetchPhase;
use crate::domain::error::mapping::http_failure;
use crate::domain::http_error::HttpError;

/// Reads the response body as a string with a size cap.
///
/// The body is read as a stream of bytes, accumulating until either:
/// - The stream ends successfully, or
/// - The accumulated size exceeds `limit`, at which point reading is
///   aborted and an `HttpError::BodyTooLarge` is returned.
///
/// Charset is extracted from the `Content-Type` header before consuming
/// the body, with UTF-8 as fallback.
///
/// # Arguments
///
/// * `response` - The HTTP response to read
/// * `limit` - Maximum number of bytes to read (decompressed size)
///
/// # Returns
///
/// The response body as a string, or an `HttpError` if the body is too
/// large or if there was an error reading the stream or decoding the bytes.
pub async fn read_body_capped(response: Response, limit: u64) -> Result<String, HttpError> {
    // Charset from Content-Type BEFORE the body is consumed (headers stay
    // readable until the body stream is taken).
    let content_type = response
        .headers()
        .get(wreq::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(';').find_map(|part| {
                let part = part.trim();
                part.strip_prefix("charset=")
                    .map(|c| c.trim_matches('"').trim().to_ascii_lowercase())
            })
        })
        .unwrap_or_else(|| "utf-8".to_string());

    // #2859: a body-stream failure is transport, not request construction.
    // The host is read before the response is consumed; the phase decides that
    // a timeout here is a read deadline, so the start instant is only carried
    // by the seam and never used to refine a connect cause.
    let host = response.uri().host().map(str::to_owned);
    let body_read_started = Instant::now();
    let mut stream = response.bytes_stream();
    let mut buf = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| http_failure(e, FetchPhase::Body, host.as_deref(), body_read_started))?;
        if buf.len().saturating_add(chunk.len()) as u64 > limit {
            // The outer fetch span already carries the URL; the event only
            // needs the machine-readable cap facts.
            warn!(
                limit = limit,
                "response body exceeded the page size cap; aborting read"
            );
            return Err(HttpError::BodyTooLarge { limit });
        }
        buf.extend_from_slice(&chunk);
    }

    let (text, _, _) = Encoding::for_label(content_type.as_bytes())
        .unwrap_or(UTF_8)
        .decode(&buf);
    Ok(text.into_owned())
}
