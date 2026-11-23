//! Byte-window helpers for the vector export metadata header.
//!
//! Locate, parse, count and patch the fixed-width header fields without
//! reading the whole export file. Kept separate from the `VectorExporter`
//! write path in [`super`] so each file stays a cohesive unit.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::domain::exporter::{ExportResult, ExporterError};

/// Number of leading bytes scanned to locate and patch the metadata header.
///
/// The header line fits comfortably inside this window even with the reserved
/// fields, so scanning a fixed prefix avoids reading whole files on close.
pub(super) const HEADER_SCAN_BYTES: usize = 512;

/// Read up to [`HEADER_SCAN_BYTES`] leading bytes from the file.
///
/// Seeks to the start first. Returns fewer bytes when the file is shorter
/// than the scan window.
pub(super) fn read_header_bytes(file: &mut File) -> ExportResult<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut head = vec![0_u8; HEADER_SCAN_BYTES];
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
