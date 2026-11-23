//! Byte-window helpers for the vector export metadata header.
//!
//! Locate, parse, count and patch the fixed-width header fields without
//! reading the whole export file. Kept separate from the `VectorExporter`
//! write path in [`super`] so each file stays a cohesive unit.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::domain::exporter::{ExportResult, ExporterError};

/// Initial number of leading bytes scanned to locate the metadata header.
///
/// Headers written by this exporter fit in this window, so the common case is
/// one bounded read. A larger header (for example a legacy `created_at` copied
/// from an external file) grows the window instead of truncating the markers.
pub(super) const HEADER_SCAN_BYTES: usize = 512;

/// Hard ceiling of the adaptive header scan (#3000).
///
/// Beyond this point the prefix is not a header: scanning stops and the
/// callers report their typed "missing field" error rather than reading an
/// unbounded prefix on every append.
pub(super) const MAX_HEADER_SCAN_BYTES: usize = 64 * 1024;

const HEADER_END_MARKER: &[u8] = b"\"documents\": [";

/// Read the leading bytes that contain the whole metadata header.
///
/// Starts at [`HEADER_SCAN_BYTES`] and doubles until the `documents` marker is
/// visible, the file ends, or [`MAX_HEADER_SCAN_BYTES`] is reached. Always
/// seeks to offset 0 first; returned offsets are absolute file offsets.
pub(super) fn read_header_bytes(file: &mut File) -> ExportResult<Vec<u8>> {
    let mut window = HEADER_SCAN_BYTES;
    loop {
        let head = read_prefix(file, window)?;
        if find_subslice(&head, HEADER_END_MARKER).is_some()
            || head.len() < window
            || window >= MAX_HEADER_SCAN_BYTES
        {
            return Ok(head);
        }
        window = window.saturating_mul(2).min(MAX_HEADER_SCAN_BYTES);
    }
}

fn read_prefix(file: &mut File, len: usize) -> ExportResult<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut head = vec![0_u8; len];
    let mut filled = 0;
    while filled < head.len() {
        let read = file.read(&mut head[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    head.truncate(filled);
    Ok(head)
}

/// Find the byte offset of `needle` inside `haystack`.
pub(super) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parse the right-aligned `total_documents` window into a count.
pub(super) fn parse_count_field(field: &[u8]) -> ExportResult<usize> {
    let text = std::str::from_utf8(field).map_err(|_| {
        ExporterError::WriteError(
            "vector export total_documents field is not valid UTF-8".to_string(),
        )
    })?;
    text.trim().parse::<usize>().map_err(|_| {
        ExporterError::WriteError(format!(
            "vector export total_documents field holds an unparsable count: {text:?}"
        ))
    })
}

/// Count document lines by scanning the whole file for 0x0A bytes.
///
/// The header carries no newline and every document is written with a single
/// `writeln!` (serde_json escapes line breaks inside strings), so in a file
/// truncated at its closing `]` the newline total equals the document count.
pub(super) fn count_documents(file: &mut File) -> ExportResult<usize> {
    file.seek(SeekFrom::Start(0))?;
    let mut newlines = 0_usize;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        newlines += buffer[..read].iter().filter(|&&byte| byte == b'\n').count();
    }
    Ok(newlines)
}

/// Truncate an existing export at the documents-array closing `]`.
///
/// A missing bracket means a previous write was interrupted mid-document:
/// fail closed instead of appending after a torn fragment. No byte is written
/// before the bracket is found (ADR-0023).
pub(super) fn truncate_at_closing_bracket(
    file: &mut File,
    path: &std::path::Path,
    seek_start: u64,
    tail: &[u8],
) -> ExportResult<()> {
    let content = String::from_utf8_lossy(tail);
    let last_bracket = content.rfind(']').ok_or_else(|| {
        tracing::warn!(
            path = %path.display(),
            tail_bytes = tail.len(),
            "refusing to append: vector export has no closing bracket (torn document)"
        );
        ExporterError::WriteError(format!(
            "vector export {} has no closing bracket in the last {} bytes: \
             a torn document from an interrupted write; refusing to append",
            path.display(),
            tail.len()
        ))
    })?;
    let insertion_point = seek_start + last_bracket as u64;
    file.set_len(insertion_point)?;
    file.seek(SeekFrom::Start(insertion_point))?;
    Ok(())
}

/// Extract the `created_at` value from a header scan, if present.
pub(super) fn extract_created_at(head: &[u8]) -> Option<String> {
    let marker = b"\"created_at\": \"";
    let start = find_subslice(head, marker)? + marker.len();
    let end = head[start..].iter().position(|&byte| byte == b'"')? + start;
    std::str::from_utf8(&head[start..end])
        .ok()
        .map(str::to_string)
}

/// Overwrite one reserved header window in place.
///
/// Verifies the sentinel byte (the ',' that must follow the fixed-width
/// window) before writing, so any layout mismatch is an error instead of a
/// silent corruption. Seeks `file` to the window but does not restore the
/// offset afterwards.
pub(super) fn patch_header_field(
    file: &mut File,
    head: &[u8],
    marker: &[u8],
    field_name: &str,
    value: &str,
    width: usize,
) -> ExportResult<()> {
    if value.len() != width {
        return Err(ExporterError::WriteError(format!(
            "vector export {field_name} value does not fit the reserved {width}-byte window"
        )));
    }
    let value_start = find_subslice(head, marker)
        .map(|pos| pos + marker.len())
        .ok_or_else(|| {
            ExporterError::WriteError(format!(
                "vector export header is missing the {field_name} field"
            ))
        })?;
    if head.get(value_start + width) != Some(&b',') {
        return Err(ExporterError::WriteError(format!(
            "vector export header layout mismatch around {field_name}"
        )));
    }
    file.seek(SeekFrom::Start(value_start as u64))?;
    file.write_all(value.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_created_at_header_is_scanned_past_the_initial_window() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("oversized.json");
        let created_at = "x".repeat(HEADER_SCAN_BYTES + 64);
        let header = format!(
            r#"{{"format_version": "1.0", "model_name": null, "dimensions": null, "total_documents": 0, "created_at": "{created_at}", "documents": ["#
        );
        let mut file = std::fs::File::create(&path).expect("create header fixture");
        file.write_all(header.as_bytes())
            .expect("write header fixture");
        drop(file);

        let mut file = std::fs::File::open(&path).expect("open header fixture");
        let head = read_header_bytes(&mut file).expect("read header");
        assert!(
            find_subslice(&head, b"\"documents\": [").is_some(),
            "the documents marker must stay inside the scan window"
        );
    }
}
