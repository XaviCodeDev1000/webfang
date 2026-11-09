//! Spool → content conversion shared by every consumer of a capture spool.
//!
//! Three callers, ONE conversion loop (#1290 shared path — identical DTO by
//! construction):
//!
//! - the batch export phase ([`crate::cli::batch_flow`] `extract_batch_content`),
//!   which converts the whole spool after the crawl and lets `export_phase`
//!   write the files;
//! - `webfang recover` (slice 1), which converts a STRANDED spool and writes
//!   the `.md` files the clean close never did (#2611);
//! - the incremental flush (slice 2), which converts the spool tail past a
//!   byte watermark and writes + commits per chunk.
//!
//! Conversion and writing are deliberately separate steps:
//! [`convert_spool`] is pure (no filesystem writes), and the caller owns
//! writing the converted pages through the repo's single saver. A consumer
//! that needs the page-vs-I/O distinction (#2611) composes both itself.
//!
//! # Watermark contract
//!
//! [`SpoolExportOutcome::consumed_bytes`] is the absolute byte offset the
//! reader reached — always a line boundary (`\n` included). It is the value a
//! flush checkpoint may persist and an [`CapturedPageReader::open_at`] may
//! resume from. Corrupt records count toward it (they are consumed bytes);
//! a mid-record I/O failure does NOT get a watermark — that is
//! [`SpoolExportError::SpoolRead`], and the caller must fall back to the
//! previous checkpoint.

use tracing::warn;

use crate::application::crawler::bounded_sink::{BoundedSinkError, CapturedPageReader};
use crate::application::crawler::content_sink::extract_page_content;
use crate::domain::config::ScraperConfig;
use crate::domain::{CorrelationId, ScrapedContent};
use crate::error::ScraperError;

/// Configuration for spool conversion.
#[derive(Debug, Clone)]
pub struct SpoolExportConfig {
    /// Extraction options (selector, WAF handling, …) applied to every page.
    pub scraper: ScraperConfig,
}

/// One page converted from the spool, not yet written to disk.
#[derive(Debug)]
pub struct ConvertedPage {
    /// Source URL of the page.
    pub url: String,
    /// Extracted content, ready for the caller's writer.
    pub content: ScrapedContent,
}

/// Result of converting a spool (or a byte range of one) into content.
#[derive(Debug, Default)]
pub struct SpoolExportOutcome {
    /// Pages successfully extracted, in spool order.
    pub converted: Vec<ConvertedPage>,
    /// `(url, error)` per page whose extraction failed. A page-level
    /// failure counts and advances the watermark; it never aborts the run.
    pub failed: Vec<(String, ScraperError)>,
    /// Corrupt spool lines: skipped, counted, never abort. Their bytes are
    /// part of [`Self::consumed_bytes`].
    pub skipped_corrupt: u64,
    /// Absolute byte offset the reader reached (line boundary, `\n`
    /// included) — the durable-bytes watermark for this conversion.
    pub consumed_bytes: u64,
}

/// Failures of a spool conversion.
///
/// A read failure ([`SpoolExportError::SpoolRead`]) is deliberately distinct
/// from an output failure ([`SpoolExportError::OutputIo`]): the first leaves
/// the spool watermark untrustworthy past the last line boundary, the second
/// leaves it untouched — the spool is still fully un-exported.
#[derive(Debug, thiserror::Error)]
pub enum SpoolExportError {
    /// A read failed mid-spool. `partial` carries the pages converted
    /// before the failure (NOT saved yet); `partial.consumed_bytes` is the
    /// last line boundary the reader vouches for.
    #[error("no se pudo leer el spool de captura: {source}")]
    SpoolRead {
        /// Underlying I/O error.
        source: std::io::Error,
        /// Conversion progress before the failure.
        partial: SpoolExportOutcome,
    },

    /// Writing the converted pages failed. The watermark is unaffected:
    /// the failure is output-side, not capture-side.
    #[error("no se pudieron escribir los archivos de salida: {0}")]
    OutputIo(#[from] ScraperError),
}

/// Convert spool records into content, one page at a time, writing NOTHING.
///
/// Corrupt lines are skipped and counted (their bytes consumed); extraction
/// failures are collected in the outcome; an I/O read failure stops the loop
/// and returns [`SpoolExportError::SpoolRead`] with the partial progress.
/// This is the single conversion path shared with MCP's session export
/// (P6-2/F-16, #1290): every consumer feeds the same
/// [`extract_page_content`] with the same DTO by construction.
///
/// # Errors
///
/// Returns [`SpoolExportError::SpoolRead`] when the spool cannot be read.
/// Writing the converted pages is the caller's step; its I/O failures are
/// [`SpoolExportError::OutputIo`], produced by that writer.
pub async fn convert_spool(
    reader: &mut CapturedPageReader,
    cfg: &SpoolExportConfig,
) -> Result<SpoolExportOutcome, SpoolExportError> {
    let root_correlation = CorrelationId::new();
    let mut outcome = SpoolExportOutcome::default();
    let mut record_no: u64 = 0;

    loop {
        let page = match reader.next_page().await {
            Ok(Some(page)) => page,
            Ok(None) => break,
            // Corrupt record: a page-level failure. The reader already
            // consumed the line's bytes, so the watermark stays honest.
            Err(BoundedSinkError::Codec(e)) => {
                record_no += 1;
                outcome.skipped_corrupt += 1;
                warn!(record_no, error = %e, "corrupt capture record skipped");
                continue;
            },
            // I/O failure: stop. The caller must fall back to the previous
            // checkpoint — the position past the last line boundary is not
            // vouched for.
            Err(e) => {
                return Err(SpoolExportError::SpoolRead {
                    source: bounded_error_io(e),
                    partial: outcome,
                });
            },
        };
        record_no += 1;

        let page_correlation = root_correlation.child();
        match extract_page_content(&page, &cfg.scraper, &page_correlation).await {
            Ok(content) => {
                outcome.converted.push(ConvertedPage {
                    url: page.url.clone(),
                    content,
                });
            },
            Err((url, e)) => outcome.failed.push((url, e)),
        }
    }

    outcome.consumed_bytes = reader.consumed_bytes();
    Ok(outcome)
}

/// Reduce a [`BoundedSinkError`] to its I/O core for the [`SpoolExportError`]
/// envelope. Reader call sites can only produce `Io` and `Codec`; the
/// remaining variants are reported as opaque I/O failures rather than
/// panicking or being silently dropped.
fn bounded_error_io(e: BoundedSinkError) -> std::io::Error {
    match e {
        BoundedSinkError::Io(io) => io,
        other => std::io::Error::other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::crawler::content_sink::CapturedPage;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn page(url: &str, html: &str) -> CapturedPage {
        CapturedPage {
            url: url.to_string(),
            html: html.to_string(),
        }
    }

    /// Write a spool file from raw JSON lines and return (dir, path, total_len).
    async fn seed_spool(lines: &[std::string::String]) -> (TempDir, PathBuf, u64) {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join(".webfang-batch-capture.jsonl");
        let mut body = std::string::String::new();
        for line in lines {
            body.push_str(line);
            body.push('\n');
        }
        tokio::fs::write(&path, &body).await.expect("write spool");
        let len = u64::try_from(body.len()).expect("len fits u64");
        (dir, path, len)
    }

    fn valid_line(url: &str, title: &str) -> std::string::String {
        // Substantial body: the shared minimum-content guard (#706) rejects
        // tiny pages on the Readability success branch, so fixtures must
        // carry real content to exercise the CONVERTED path.
        let body = "contenido de prueba sustancial para superar el guardia de contenido mínimo del extractor. ".repeat(12);
        let html = format!("<html><body><h1>{title}</h1><p>{body}</p></body></html>");
        let page = page(url, &html);
        serde_json::to_string(&page).expect("serialize page")
    }

    fn config() -> SpoolExportConfig {
        SpoolExportConfig {
            scraper: ScraperConfig::default(),
        }
    }

    /// Seed a spool and convert it from offset 0.
    async fn convert_lines(lines: &[std::string::String]) -> (TempDir, SpoolExportOutcome, u64) {
        let (dir, path, total) = seed_spool(lines).await;
        let mut reader = CapturedPageReader::open(&path).await.expect("open");
        let outcome = convert_spool(&mut reader, &config())
            .await
            .expect("convert");
        (dir, outcome, total)
    }

    #[tokio::test]
    async fn reader_tracks_bytes_and_resumes_at_offset() {
        let first = valid_line("https://example.test/a", "A");
        let second = valid_line("https://example.test/b", "B");
        let (_dir, path, total) =
            seed_spool(&[first.clone(), std::string::String::new(), second]).await;

        let mut reader = CapturedPageReader::open(&path).await.expect("open");
        let one = reader.next_page().await.expect("page 1").expect("some");
        assert_eq!(one.url, "https://example.test/a");
        let after_first = reader.consumed_bytes();
        assert_eq!(after_first, first.len() as u64 + 1, "first line + newline");

        let mut resumed = CapturedPageReader::open_at(&path, after_first)
            .await
            .expect("open at offset");
        let two = resumed.next_page().await.expect("page 2").expect("some");
        assert_eq!(two.url, "https://example.test/b");
        // The blank line between the two records is consumed on the way.
        assert_eq!(
            resumed.consumed_bytes(),
            total,
            "resumed reader reaches EOF at the same absolute offset"
        );
    }

    #[tokio::test]
    async fn reader_surfaces_corrupt_lines_but_keeps_counting_bytes() {
        let good = valid_line("https://example.test/a", "A");
        let corrupt = "{not json".to_string();
        let (_dir, path, total) = seed_spool(&[good, corrupt]).await;

        let mut reader = CapturedPageReader::open(&path).await.expect("open");
        assert!(reader.next_page().await.expect("page 1").is_some());
        assert!(matches!(
            reader.next_page().await,
            Err(BoundedSinkError::Codec(_))
        ));
        assert!(reader.next_page().await.expect("eof").is_none());
        assert_eq!(reader.consumed_bytes(), total, "corrupt bytes are consumed");
    }

    #[tokio::test]
    async fn convert_spool_counts_corrupt_records_and_reports_watermark() {
        let (dir, outcome, total) = convert_lines(&[
            valid_line("https://example.test/a", "A"),
            "{broken".to_string(),
            valid_line("https://example.test/b", "B"),
        ])
        .await;

        assert_eq!(outcome.converted.len(), 2);
        assert_eq!(outcome.skipped_corrupt, 1);
        assert!(outcome.failed.is_empty());
        assert_eq!(outcome.consumed_bytes, total, "full spool consumed");
        // convert_spool is pure: the spool directory gained no output file.
        let entries = std::fs::read_dir(dir.path())
            .expect("read spool dir")
            .count();
        assert_eq!(entries, 1, "conversion must not write files");
    }

    #[tokio::test]
    async fn convert_spool_from_offset_only_reads_the_tail() {
        let first = valid_line("https://example.test/a", "A");
        let second = valid_line("https://example.test/b", "B");
        let (_dir, path, total) = seed_spool(&[first.clone(), second]).await;
        let cfg = config();
        let boundary = first.len() as u64 + 1;

        let mut reader = CapturedPageReader::open_at(&path, boundary)
            .await
            .expect("open at");
        let outcome = convert_spool(&mut reader, &cfg).await.expect("convert");

        assert_eq!(outcome.converted.len(), 1, "only the second record");
        assert_eq!(outcome.converted[0].url, "https://example.test/b");
        assert_eq!(outcome.skipped_corrupt, 0);
        assert_eq!(outcome.consumed_bytes, total);
    }

    #[tokio::test]
    async fn failed_extraction_is_collected_not_fatal() {
        let (_dir, outcome, total) = convert_lines(&[
            valid_line("https://example.test/a", "A"),
            // Empty body: extraction cannot produce content from it.
            serde_json::to_string(&page("https://example.test/empty", "")).expect("serialize"),
        ])
        .await;

        assert_eq!(outcome.converted.len(), 1);
        assert_eq!(outcome.failed.len(), 1, "empty page collected as failure");
        assert_eq!(outcome.failed[0].0, "https://example.test/empty");
        assert_eq!(
            outcome.consumed_bytes, total,
            "watermark advances past failures"
        );
    }
}
