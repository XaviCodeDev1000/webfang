//! VectorExporter implementation for RAG pipeline
//!
//! Exports document chunks to JSON format with metadata headers,
//! supporting embeddings and cosine similarity calculations.

// `File::unlock()` is stable since 1.89.0, but we use fs2::FileExt for compatibility.
#![allow(clippy::incompatible_msrv)]

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use fs2::FileExt;

use crate::domain::entities::DocumentChunkValidated;
use crate::domain::exporter::{ExportResult, Exporter, ExporterConfig, ExporterError};

/// Reserved character width of the `total_documents` header field.
///
/// The count is written right-aligned (space-padded) into this window so
/// [`VectorExporter::close_json`] can patch it in place without shifting the
/// rest of the file. Covers document counts up to 9,999,999,999.
const COUNT_FIELD_WIDTH: usize = 10;

/// Reserved character width of the `dimensions` header field.
///
/// Right-aligned like [`COUNT_FIELD_WIDTH`]; holds either the embedding
/// dimension count or the literal `null`. Eight characters cover any
/// realistic embedding size.
const DIM_FIELD_WIDTH: usize = 8;

/// Number of leading bytes scanned to locate and patch the metadata header.
///
/// The header line fits comfortably inside this window even with the reserved
/// fields, so scanning a fixed prefix avoids reading whole files on close.
const HEADER_SCAN_BYTES: usize = 512;

/// Computes cosine similarity between two vectors
///
/// Returns a value between -1.0 and 1.0, where:
/// - 1.0 means identical direction
/// - 0.0 means orthogonal
/// - -1.0 means opposite direction
///
/// Returns 0.0 for zero-magnitude vectors.
///
/// # Arguments
/// * `a` - First vector
/// * `b` - Second vector
///
/// # Errors
/// Returns `DimensionMismatch` if vectors have different dimensions
#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Result<f32, ExporterError> {
    if a.len() != b.len() {
        return Err(ExporterError::DimensionMismatch {
            expected: b.len(),
            actual: a.len(),
        });
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if mag_a == 0.0 || mag_b == 0.0 {
        return Ok(0.0);
    }

    Ok(dot_product / (mag_a * mag_b))
}

/// VectorExporter for RAG pipeline
///
/// Exports documents to JSON format with:
/// - Metadata header (format version, dimensions, document count)
/// - Documents array with optional embeddings
/// - Support for append mode
pub struct VectorExporter {
    config: ExporterConfig,
    dimensions: Mutex<Option<usize>>,
}

impl VectorExporter {
    /// Create a new VectorExporter with default path
    #[must_use]
    pub fn new(config: ExporterConfig) -> Self {
        Self {
            config,
            dimensions: Mutex::new(None),
        }
    }

    /// Create a new VectorExporter with custom output path
    #[must_use]
    pub fn new_with_path(config: ExporterConfig, output_dir: impl Into<PathBuf>) -> Self {
        let mut config = config;
        config.output_dir = output_dir.into();
        Self {
            config,
            dimensions: Mutex::new(None),
        }
    }

    /// Get a file writer with proper locking
    ///
    /// Creates directories if needed, acquires fs2 file lock,
    /// and returns a BufWriter for efficient I/O.
    ///
    /// In append mode with an existing file, finds and truncates at the
    /// closing `]` so the writer can append documents and re-close.
    fn writer(&self) -> ExportResult<(File, BufWriter<File>)> {
        // Create output directory if it doesn't exist
        fs::create_dir_all(&self.config.output_dir)?;

        let path = self.config.output_path();

        let file = if self.config.append && path.exists() {
            let mut f = OpenOptions::new().read(true).write(true).open(&path)?;

            // Find the closing `]` and truncate there so we can re-append
            let len = f.metadata()?.len();
            if len > 0 {
                let seek_start = len.saturating_sub(256);
                f.seek(SeekFrom::Start(seek_start))?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;

                let content = String::from_utf8_lossy(&buf);
                if let Some(last_bracket) = content.rfind(']') {
                    let truncate_pos = seek_start + last_bracket as u64;
                    f.set_len(truncate_pos)?;
                    f.seek(SeekFrom::Start(truncate_pos))?;
                }
            }
            f
        } else {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(!self.config.append)
                .open(&path)?
        };

        // Acquire exclusive lock
        file.lock_exclusive()?;

        let writer = BufWriter::new(file.try_clone()?);

        Ok((file, writer))
    }

    /// Write metadata header to file
    ///
    /// For new files: writes the header template with reserved (right-aligned,
    /// space-padded) `dimensions` and `total_documents` windows that
    /// [`Self::close_json`] patches in place once the real values are known.
    /// For append mode with existing documents: the file is already truncated
    /// at the closing `]` by [`Self::writer`], so nothing is written here.
    fn write_metadata_header(
        &self,
        writer: &mut BufWriter<File>,
        file: &mut File,
        is_first_doc: bool,
    ) -> ExportResult<()> {
        if !is_first_doc {
            return Ok(());
        }

        if self.config.append && file.metadata()?.len() > 0 {
            // File already truncated at `]` by writer() — just seek back to it
            file.seek(SeekFrom::End(0))?;
        } else {
            // New file or overwrite mode — header with reserved patch windows
            let timestamp = Utc::now().to_rfc3339();
            let header = self.build_header_line(&timestamp);
            write!(writer, "{header}")?;
        }

        Ok(())
    }

    /// Build the metadata header line with reserved in-place patch windows.
    ///
    /// `dimensions` and `total_documents` are right-aligned into fixed-width
    /// windows ([`DIM_FIELD_WIDTH`] / [`COUNT_FIELD_WIDTH`]). Space padding is
    /// legal JSON whitespace and right-alignment avoids leading zeros, so both
    /// fields can be overwritten later without rewriting the whole file.
    fn build_header_line(&self, created_at: &str) -> String {
        // The slot holds one `Option<usize>`; recovering it keeps the header
        // written with whatever dimension the run learned instead of aborting
        // the export over a panic in an unrelated writer thread.
        let dimensions_json = self
            .dimensions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map(|d| d.to_string())
            .unwrap_or_else(|| "null".to_string());
        let count = 0_usize;
        format!(
            r#"{{"format_version": "1.0", "model_name": null, "dimensions": {dimensions_json:>DIM_FIELD_WIDTH$}, "total_documents": {count:>COUNT_FIELD_WIDTH$}, "created_at": "{created_at}", "documents": ["#
        )
    }

    /// Serialize a document chunk to JSON
    ///
    /// Validates embedding dimensions if present.
    /// Rejects NaN or Infinity values in embeddings — they produce invalid JSON.
    fn serialize_document(&self, doc: &DocumentChunkValidated) -> ExportResult<String> {
        // Validate embedding dimensions if present
        if let Some(ref embeddings) = doc.embeddings {
            // Recovered, not panicked: the slot is a single `Option<usize>`.
            let mut dim_guard = self
                .dimensions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(exp) = *dim_guard {
                if embeddings.len() != exp {
                    // Dimension mismatch: the document is degraded (no embeddings in output).
                    // error! because --output-vectors implies embeddings are the point.
                    tracing::error!(
                        expected_dimensions = exp,
                        actual_dimensions = embeddings.len(),
                        "Dimension mismatch — document serialized WITHOUT embeddings"
                    );
                    // Create a copy without embeddings
                    let mut doc_without_embeddings = doc.clone();
                    doc_without_embeddings.embeddings = None;
                    return serde_json::to_string(&doc_without_embeddings)
                        .map_err(|e| ExporterError::WriteError(e.to_string()));
                }
            } else {
                // First document with embeddings - record dimensions
                *dim_guard = Some(embeddings.len());
            }

            // Reject NaN/Infinity — serde_json serialises them as `null` silently
            if embeddings.iter().any(|v| !v.is_finite()) {
                return Err(ExporterError::WriteError(
                    "embeddings contain NaN or Infinity".into(),
                ));
            }
        }

        // Serialize to JSON
        let serialized = serde_json::to_string(doc)?;
        Ok(serialized)
    }

    /// Read how many documents an existing export already holds.
    ///
    /// Called when appending to a non-empty file. Parses the reserved
    /// `total_documents` window written by [`Self::close_json`]. Files whose
    /// header predates the in-place counter fix (issue #502) fail the
    /// sentinel check and are migrated on the fly by
    /// [`Self::migrate_legacy_header`].
    ///
    /// Leaves the file offset at EOF so the caller can keep appending.
    fn read_append_state(&self, file: &mut File) -> ExportResult<usize> {
        let head = read_header_bytes(file)?;

        let marker = b"\"total_documents\": ";
        let value_start = find_subslice(&head, marker)
            .map(|pos| pos + marker.len())
            .ok_or_else(|| {
                ExporterError::WriteError(
                    "vector export header is missing the total_documents field".to_string(),
                )
            })?;

        // The fixed-width layout guarantees a ',' exactly COUNT_FIELD_WIDTH
        // bytes after the value start. Anything else means the file was
        // written by the pre-fix exporter (or is corrupt) and needs migration.
        let existing = if head.get(value_start + COUNT_FIELD_WIDTH) == Some(&b',') {
            let count = parse_count_field(&head[value_start..value_start + COUNT_FIELD_WIDTH])?;
            if count == 0 {
                // Defensive: files written by this version always carry the
                // true count. Fall back to counting document lines.
                count_documents(file)?
            } else {
                count
            }
        } else {
            self.migrate_legacy_header(file, &head)?
        };

        file.seek(SeekFrom::End(0))?;
        Ok(existing)
    }

    /// Migrate a pre-fix header to the reserved-window layout.
    ///
    /// Legacy files stored a hardcoded `"total_documents": 0` and dimensions
    /// learned too late to be written. The real document count is recovered
    /// from the line structure, the header is rebuilt with the reserved
    /// windows (preserving the original `created_at`), and the file head is
    /// rewritten in place. Safe because the caller holds the exclusive fs2
    /// lock for the whole export call.
    fn migrate_legacy_header(&self, file: &mut File, head: &[u8]) -> ExportResult<usize> {
        // serde_json never emits raw line breaks inside strings, so every
        // 0x0A byte marks the end of exactly one document line.
        let existing = count_documents(file)?;

        let docs_marker = b"\"documents\": [";
        let body_start = find_subslice(head, docs_marker)
            .map(|pos| pos + docs_marker.len())
            .ok_or_else(|| {
                ExporterError::WriteError(
                    "vector export header is missing the documents array".to_string(),
                )
            })?;
        let created_at = extract_created_at(head).ok_or_else(|| {
            ExporterError::WriteError("vector export header is missing created_at".to_string())
        })?;

        file.seek(SeekFrom::Start(body_start as u64))?;
        let mut body = Vec::new();
        file.read_to_end(&mut body)?;

        let new_head = self.build_header_line(&created_at);
        file.seek(SeekFrom::Start(0))?;
        file.write_all(new_head.as_bytes())?;
        file.write_all(&body)?;
        let new_len = file.stream_position()?;
        file.set_len(new_len)?;

        tracing::warn!(
            existing_documents = existing,
            path = %self.config.output_path().display(),
            "Migrating legacy vector export header to the fixed-count layout"
        );

        Ok(existing)
    }

    /// Close the JSON structure and patch the header counters in place.
    ///
    /// Writes the closing bracket, then overwrites the reserved
    /// `total_documents` window (and the `dimensions` window once the
    /// embedding size became known during this run) at their fixed header
    /// offsets. The `file` handle shares its offset with the `BufWriter`'s
    /// clone, so the writer is flushed before any seek.
    fn close_json(
        &self,
        writer: &mut BufWriter<File>,
        file: &mut File,
        total_documents: usize,
    ) -> ExportResult<()> {
        writeln!(writer, "]}}")?;
        writer.flush()?;

        let head = read_header_bytes(file)?;

        let count = format!("{total_documents:>COUNT_FIELD_WIDTH$}");
        patch_header_field(
            file,
            &head,
            b"\"total_documents\": ",
            "total_documents",
            &count,
            COUNT_FIELD_WIDTH,
        )?;

        // Dimensions may have been learned mid-run by serialize_document —
        // backfill the reserved window now that the final value is known.
        // Same recovery as `build_header_line`: the backfill must not be
        // dropped because another thread panicked while holding the slot.
        let known_dimensions = *self
            .dimensions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(dimensions) = known_dimensions {
            let value = format!("{dimensions:>DIM_FIELD_WIDTH$}");
            patch_header_field(
                file,
                &head,
                b"\"dimensions\": ",
                "dimensions",
                &value,
                DIM_FIELD_WIDTH,
            )?;
        }

        file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    /// Write a whole batch, returning how many payload bytes were written.
    ///
    /// The write loop of [`Exporter::export_batch`], split out so the
    /// instrumented wrapper can report the outcome AND the byte volume on its
    /// span (#1610, OBS-H6) without touching this loop's control flow. The
    /// returned count sums the serialized document lines this batch appended;
    /// it deliberately EXCLUDES the metadata header and the closing `]}}`,
    /// which this exporter writes around the documents and does not own
    /// per-document.
    fn export_batch_documents(&self, documents: &[DocumentChunkValidated]) -> ExportResult<u64> {
        if documents.is_empty() {
            return Ok(0);
        }

        let (mut file, mut writer) = self.writer()?;
        let is_first_doc =
            !self.config.append || file.metadata().map(|m| m.len() == 0).unwrap_or(true);
        let existing = if is_first_doc {
            0
        } else {
            self.read_append_state(&mut file)?
        };

        self.write_metadata_header(&mut writer, &mut file, is_first_doc)?;

        let mut doc_count = 0;
        let mut payload_bytes: u64 = 0;
        for (i, doc) in documents.iter().enumerate() {
            if i > 0 || !is_first_doc {
                write!(writer, ",")?;
            }

            let serialized = self.serialize_document(doc)?;
            payload_bytes += serialized.len() as u64;
            writeln!(writer, "{serialized}")?;
            doc_count += 1;
        }

        self.close_json(&mut writer, &mut file, existing + doc_count)?;

        // Release lock
        fs2::FileExt::unlock(&file)?;

        Ok(payload_bytes)
    }
}

/// Read up to [`HEADER_SCAN_BYTES`] leading bytes from the file.
///
/// Seeks to the start first. Returns fewer bytes when the file is shorter
/// than the scan window.
fn read_header_bytes(file: &mut File) -> ExportResult<Vec<u8>> {
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
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parse the right-aligned `total_documents` window into a count.
fn parse_count_field(field: &[u8]) -> ExportResult<usize> {
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
fn count_documents(file: &mut File) -> ExportResult<usize> {
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
fn extract_created_at(head: &[u8]) -> Option<String> {
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
fn patch_header_field(
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

impl Exporter for VectorExporter {
    fn export(&self, document: DocumentChunkValidated) -> ExportResult<()> {
        let (mut file, mut writer) = self.writer()?;
        let is_first_doc =
            !self.config.append || file.metadata().map(|m| m.len() == 0).unwrap_or(true);
        let existing = if is_first_doc {
            0
        } else {
            self.read_append_state(&mut file)?
        };

        self.write_metadata_header(&mut writer, &mut file, is_first_doc)?;

        let serialized = self.serialize_document(&document)?;

        if !is_first_doc {
            write!(writer, ",")?;
        }
        writeln!(writer, "{serialized}")?;

        self.close_json(&mut writer, &mut file, existing + 1)?;

        // Release lock
        fs2::FileExt::unlock(&file)?;

        Ok(())
    }

    // `payload_bytes` semantics (shared by all three exporters, #1610): the
    // serialized document bytes THIS batch handed to its writer. The vector
    // exporter knows them per document and excludes the header/closer it owns;
    // the file exporter's Auto/Vector branches delegate to `save_json`, which
    // reports no size, so that exporter leaves the key absent rather than
    // inventing a number.
    #[tracing::instrument(
        skip(self, documents),
        fields(
            exporter = "vector",
            documents = documents.len(),
            outcome = tracing::field::Empty,
            payload_bytes = tracing::field::Empty
        )
    )]
    fn export_batch(&self, documents: &[DocumentChunkValidated]) -> ExportResult<()> {
        super::run_export_batch(|| self.export_batch_documents(documents))
    }

    fn config(&self) -> &ExporterConfig {
        &self.config
    }
}

#[cfg(test)]
#[path = "vector_exporter_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "vector_exporter_append_tests.rs"]
mod append_tests;
