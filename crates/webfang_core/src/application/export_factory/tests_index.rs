//! Checksum-index reader tests (#2393) and the #1812 data-integrity
//! tests that share the `write_wide_jsonl`/`peak_rss_bytes` fixtures.

use super::tests_common::*;
use super::*;
use tempfile::TempDir;

pub(super) fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kb.saturating_mul(1024))
}

/// Write a JSONL file of `lines` records, each roughly `width` bytes long.
///
/// The records go through a buffered writer so the FIXTURE never holds
/// the whole file: the measurement must see the cost of the READ, not of
/// the file it reads.
pub(super) fn write_wide_jsonl(path: &std::path::Path, lines: usize, width: usize) {
    use std::io::Write;
    let file = std::fs::File::create(path).expect("create jsonl");
    let mut writer = std::io::BufWriter::new(file);
    let filler = "x".repeat(width);
    for i in 0..lines {
        writeln!(
                writer,
                "{{\"url\":\"https://big.test/{i}\",\"content\":\"{filler}\",\"checksum_sha256\":\"{i:064x}\"}}"
            )
            .expect("write line");
    }
    writer.flush().expect("flush");
}

/// The DECIDED policy (#1812): a line that is not valid UTF-8 is
/// skipped, the export COMPLETES, and the skip is REPORTED — a
/// structured error plus the count of skipped entries in the export
/// reader.
#[test]
fn renamed_checksum_field_is_counted_and_not_indexed() {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("renamed.jsonl");
    // Same shape as a real export line, with the field name changed.
    let mut content = String::new();
    for n in 1..=3 {
        content.push_str(&format!("{{\"n\":{n},\"content_hash\":\"hash-{n}\"}}\n"));
    }
    std::fs::write(&path, &content).unwrap();

    let index = read_jsonl_checksum_index(&path).unwrap();
    assert!(
        index.hashes.is_empty(),
        "no line carries CHECKSUM_FIELD, so no line may be indexed"
    );
    assert_eq!(index.report.lines, 3);
    assert_eq!(index.report.without_checksum, 3);
    assert_eq!(index.report.skipped_non_utf8, 0);
}

// =========================================================================
// domain_from_url tests
// =========================================================================

/// summary, which is the surface the operator reads.
#[test]
pub(super) fn a_non_utf8_line_is_reported_in_the_export_summary() {
    use crate::infrastructure::observability::test_log_capture::LogBuffer;

    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("export.jsonl");
    let mut bytes = b"{\"url\":\"https://kept.test/a\",\"checksum_sha256\":\"aaaa\"}\n".to_vec();
    bytes.extend_from_slice(&[0xff, 0xfe, 0x00]);
    bytes.push(b'\n');
    std::fs::write(&path, &bytes).unwrap();

    let (buffer, _guard) = LogBuffer::capture();
    process_results(
        &[make_scraped_content(
            "https://new.test/b",
            "B",
            "body of the new record",
        )],
        temp_dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        None,
    )
    .expect("the export must COMPLETE even with a damaged line");

    let log = buffer.text();
    assert!(
        log.contains("skipped_non_utf8=1"),
        "the export summary must carry the count of skipped entries: {log}"
    );
    assert!(
        log.contains("not valid UTF-8"),
        "the structured error must name the damage: {log}"
    );
}

/// The resume index must be STREAMED, not read whole (#1812).
///
/// Coarse memory check: indexing a 16 MiB export must not cost the
/// process the whole file. The reader that did `fs::read_to_string`
/// loaded every byte; the streaming reader holds ONE line at a time, so
/// the peak grows by the size of the index (one hash per line), not by
/// the size of the file.
/// The measurement is the process PEAK rss (`VmHWM`) — the current rss
/// falls back when the reader frees its buffer. Linux-only: it returns early.
#[cfg_attr(miri, ignore)]
// not a UB surface: the gate is about the MEASUREMENT, which Miri invalidates — its own allocation dwarfs VmHWM, so the probe reads the interpreter and not the reader (run 43359470120: +136953856 bytes for a ~16 MiB export)
#[test]
pub(super) fn the_resume_index_is_streamed_not_read_whole() {
    if peak_rss_bytes().is_none() {
        return; // not Linux: the peak rss is not observable
    }
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("big.jsonl");
    write_wide_jsonl(&path, 4096, 4096); // 4096 lines x ~4 KiB = ~16 MiB

    let before = peak_rss_bytes().expect("peak rss");
    let session = CommitSession::open(None, &path);
    let after = peak_rss_bytes().expect("peak rss");

    assert_eq!(
        session.hash_index.len(),
        4096,
        "every line's checksum must be indexed"
    );
    let growth = after.saturating_sub(before);
    assert!(
            growth < 8 * 1024 * 1024,
            "the index must not hold the whole file: the peak grew by {growth} bytes for a ~16 MiB export"
        );
}

// =========================================================================
// AI chunk export: resume gate BEFORE batch export (BUG F3-A)
// =========================================================================
// The non-AI path (`process_single_item`) decides BEFORE each export;
// the AI chunk path must mirror that ordering so --clean-ai --resume
// never appends chunks whose URL is COMMITTED-proven or whose bytes
// are already flushed (PromoteFromFlushProof).
