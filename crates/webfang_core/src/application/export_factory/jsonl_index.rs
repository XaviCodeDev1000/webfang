//! JSONL resume checksum index: streaming reader of an existing output file
//! that collects `checksum_sha256` values (flush-proof membership) plus the
//! operator-facing [`JsonlIndexReport`] counters (#2393, #1812, #1617).
//! Split out of `export_factory.rs` under the ~500-line rule (#2584, Ola 1);
//! mechanical movement only.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};

use tracing::info;

use crate::domain::exporter::CHECKSUM_FIELD;

///
/// The report is what the export summary shows the operator: how many lines
/// were read, how many carried no checksum, and how many were skipped
/// because they were not valid UTF-8 (#1812).
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonlIndexReport {
    /// Non-empty lines read from the file.
    pub lines: usize,
    /// Lines that parsed as a JSON object but carried no checksum field.
    pub without_checksum: usize,
    /// Lines that were not valid UTF-8 and were skipped.
    pub skipped_non_utf8: usize,
}

/// The [`CHECKSUM_FIELD`] index of an existing JSONL output file, plus the
/// counters collected while reading it.
#[derive(Debug, Default)]
pub struct JsonlChecksumIndex {
    /// Every checksum found in the file.
    pub(crate) hashes: HashSet<String>,
    /// What the read observed.
    pub(crate) report: JsonlIndexReport,
}

/// Read the [`CHECKSUM_FIELD`] index of an existing JSONL output file.
///
/// # Streaming (#1812)
///
/// The file is read LINE BY LINE through a [`BufReader`], never as one
/// string: an export larger than RAM must not be loaded into RAM to be
/// indexed. This is the ONE reader for the export file's checksum field
/// (#2393): the JSONL writer session no longer reads the output file at all
/// on open — only its torn tail — so the application layer is the single
/// place that parses lines for `CHECKSUM_FIELD`, and the resume gate's view
/// of the file is built exactly here.
///
/// # Damaged bytes (#1812)
///
/// A line that is not valid UTF-8 is SKIPPED and counted, never fatal: one
/// damaged byte must not abort the whole export. The caller reports the
/// count through [`JsonlIndexReport::skipped_non_utf8`].
///
/// # Errors
///
/// Returns an I/O error when the file exists but cannot be read. A missing
/// file is NOT an error — it yields an empty index, exactly like a first run
/// against a fresh output path.
pub fn read_jsonl_checksum_index(path: &std::path::Path) -> std::io::Result<JsonlChecksumIndex> {
    let mut index = JsonlChecksumIndex::default();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(index),
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        // Strip the line terminator: the `\n` and the `\r` of a CRLF file.
        while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        index.report.lines += 1;
        let Ok(text) = std::str::from_utf8(&line) else {
            index.report.skipped_non_utf8 += 1;
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        match value
            .get(CHECKSUM_FIELD)
            .and_then(serde_json::Value::as_str)
        {
            Some(hash) => {
                index.hashes.insert(hash.to_owned());
            },
            None => index.report.without_checksum += 1,
        }
    }
    Ok(index)
}

/// Log a structured error that ALSO carries the [`std::io::ErrorKind`].
///
/// The shared [`log_scrape_error`](crate::infrastructure::observability::log_scrape_error)
/// renders `error = %error`, and an `io::Error`'s `Display` prints only its
/// message: the `ErrorKind` set on the value never reaches the event, so the
/// operator sees the message and the stage but not the KIND of the damage.
/// That helper's signature is frozen (other callers depend on it), so the
/// extra field is added HERE, at the call site.
///
/// The field set is the shared helper's, plus `error_kind`.
pub(crate) fn log_io_error_with_kind(
    error: &std::io::Error,
    url: &str,
    stage: &str,
    correlation_id: Option<&crate::domain::CorrelationId>,
    context: &str,
) {
    tracing::error!(
        error = %error,
        error_kind = ?error.kind(),
        url = %url,
        stage = %stage,
        trace_id = correlation_id.map(|c| c.trace_id().to_string()),
        "{context}"
    );
}

/// Rebuild the resume dedup index from an existing output file.
///
/// The field name comes from the pinned [`CHECKSUM_FIELD`] constant (#1617,
/// D3), not from a string literal at this site: the exporter writes it and
/// this is the ONE reader (#2393), so a rename on either side is visible
/// here. Lines that parse but carry no checksum are counted and reported, so
/// a writer/reader field-name mismatch is visible instead of silent.
///
/// The read itself is [`read_jsonl_checksum_index`]'s: STREAMING, and a line
/// that is not valid UTF-8 is skipped and reported instead of failing the
/// export (#1812).
pub(crate) fn build_content_hash_index(
    path: &std::path::Path,
) -> (HashSet<String>, JsonlIndexReport) {
    let index = match read_jsonl_checksum_index(path) {
        Ok(index) => index,
        Err(e) => {
            // The index only PROVES a flush for the resume gate; an
            // unreadable file proves nothing, but it must not abort the
            // export either. Report it and continue with an empty index.
            log_io_error_with_kind(
                &e,
                &path.display().to_string(),
                "jsonl_resume_index",
                None,
                "could not read the output file to rebuild the resume index; continuing with an empty index",
            );
            return (HashSet::new(), JsonlIndexReport::default());
        },
    };
    if index.report.skipped_non_utf8 > 0 {
        // The DECIDED policy (#1812): a line that is not valid UTF-8 is
        // skipped, the export COMPLETES, and the damage is reported — never
        // a silent skip and never a failed export.
        let error = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} line(s) of the export file are not valid UTF-8",
                index.report.skipped_non_utf8
            ),
        );
        log_io_error_with_kind(
            &error,
            &path.display().to_string(),
            "jsonl_resume_index",
            None,
            "JSONL lines that are not valid UTF-8 were skipped while indexing the export file",
        );
    }
    if index.report.without_checksum > 0 {
        tracing::warn!(
            file = %path.display(),
            lines = index.report.lines,
            lines_without_checksum = index.report.without_checksum,
            field = CHECKSUM_FIELD,
            "output lines carry no checksum field; resume dedup will re-drive them (writer/reader field-name mismatch?)"
        );
    }
    info!(
        file = %path.display(),
        lines = index.report.lines,
        hashes = index.hashes.len(),
        skipped_non_utf8 = index.report.skipped_non_utf8,
        "indexed output file for resume dedup"
    );
    (index.hashes, index.report)
}

/// Report of the ONE reader of an existing JSONL output file: how many
/// non-empty lines it has, how many carry no checksum field, and how many
/// were skipped for not being valid UTF-8.
///
/// This is the public surface for callers outside a commit session — the MCP
/// layer transports the count instead of recomputing it with its own line
/// semantics (#2272).
///
/// # Tolerant by policy (#1812)
///
/// An unreadable file proves nothing, but it must not abort: the caller gets
/// the empty report and the I/O error is logged. Only a genuinely damaged
/// count can be lost, and the export summary never lies about lines it did
/// read.
pub fn jsonl_index_report(path: &std::path::Path) -> JsonlIndexReport {
    match read_jsonl_checksum_index(path) {
        Ok(index) => index.report,
        Err(e) => {
            log_io_error_with_kind(
                &e,
                &path.display().to_string(),
                "jsonl_resume_index",
                None,
                "could not read the output file to report the JSONL index; continuing with an empty report",
            );
            JsonlIndexReport::default()
        },
    }
}
