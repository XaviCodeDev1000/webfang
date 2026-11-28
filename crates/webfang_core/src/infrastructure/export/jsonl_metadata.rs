//! The JSONL metadata record: the schema `WebfangMetadata` writes for one
//! document line.
//!
//! Split out of `jsonl_exporter.rs` when the additive degradation marker (T6,
//! issue #3117) landed: the record's field list is the schema downstream RAG
//! pipelines read, so it gets its own module and its own file-size budget
//! instead of widening the exporter that happens to write it.
//!
//! ## Metadata Schema (v2.1.0)
//!
//! Each JSONL line includes:
//! - `url`: Source URL
//! - `timestamp_utc`: ISO 8601 / RFC 3339 timestamp
//! - `title`: Document title (optional)
//! - `content`: Extracted text content
//! - `checksum_sha256`: SHA-256 hash of content for deduplication
//! - `metadata_version`: Schema version ("2.1.0")
//! - `content_length`: Character count for quick filtering
//! - `word_count`: Word count (optional, from metadata or computed)
//! - `reading_time`: Estimated reading time in minutes (optional)
//! - `language`: Detected language (optional)
//! - `content_type`: Content type classification (optional)
//! - `scrape_date`: Date of scrape (optional)
//! - `degraded_reason`: Why this line is the RAW fallback text rather than
//!   semantically cleaned text (optional, T6)
//! - `extra_metadata`: Additional metadata BTreeMap (optional, keys in lexicographic order)

use serde::Serialize;

use crate::domain::cleaning_status::DEGRADED_REASON_KEY;
use crate::domain::entities::DocumentChunkValidated;

/// Webfang JSONL metadata schema (v2.1.0)
///
/// Wraps DocumentChunkValidated with additional fields for
/// RAG pipeline integration and content deduplication.
#[derive(Serialize)]
pub struct WebfangMetadata<'a> {
    /// Source URL
    pub url: &'a str,
    /// ISO 8601 / RFC 3339 timestamp
    pub timestamp_utc: String,
    /// Document title (optional)
    pub title: Option<&'a str>,
    /// Extracted text content
    pub content: &'a str,
    /// SHA-256 hash of content for deduplication
    ///
    /// Serialized as [`CHECKSUM_FIELD`](crate::domain::exporter::CHECKSUM_FIELD);
    /// the field name is pinned there and a test fails if the two ever diverge
    /// (#1617, D3).
    #[serde(rename = "checksum_sha256")]
    pub checksum_sha256: String,
    /// Schema version
    pub metadata_version: &'static str,
    /// Character count for quick filtering
    pub content_length: usize,
    /// Word count (from metadata or computed from content)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_count: Option<usize>,
    /// Estimated reading time in minutes (from metadata or computed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading_time: Option<usize>,
    /// Detected language
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Content type classification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Date of scrape (ISO 8601)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrape_date: Option<String>,
    /// Why `content` is the RAW page text instead of semantically cleaned text
    /// (T6, issue #3117): the `SemanticError` variant that degraded the page.
    ///
    /// ABSENT when the page cleaned normally, so the pre-T6 line shape is
    /// unchanged for every clean page (additive-first, plan S4) — a consumer
    /// that knows nothing about the marker reads exactly what it read before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    /// Additional metadata (excerpt, author, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_metadata: Option<std::collections::BTreeMap<String, String>>,
}

impl<'a> WebfangMetadata<'a> {
    /// Create from a DocumentChunkValidated
    pub fn from_chunk(chunk: &'a DocumentChunkValidated) -> Self {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(chunk.content.as_bytes());
        let checksum = format!("{:x}", hasher.finalize());

        let word_count = chunk
            .metadata
            .get("word_count")
            .and_then(|v| v.parse::<usize>().ok())
            .or_else(|| {
                let count = chunk.content.split_whitespace().count();
                if count > 0 {
                    Some(count)
                } else {
                    None
                }
            });

        let reading_time = chunk
            .metadata
            .get("reading_time")
            .and_then(|v| v.parse::<usize>().ok())
            .or_else(|| word_count.map(|wc| (wc / 200).max(1)));

        let language = chunk.metadata.get("language").cloned();
        let content_type = chunk.metadata.get("content_type").cloned();
        let scrape_date = chunk.metadata.get("scrape_date").cloned();
        // The marker rides in the chunk metadata out of the cleaning phase, so
        // the exporter promotes it to its own top-level field: a downstream
        // consumer reads `degraded_reason`, not a key buried in a map.
        let degraded_reason = chunk.metadata.get(DEGRADED_REASON_KEY).cloned();

        let extra_metadata = if chunk.metadata.is_empty() {
            None
        } else {
            let mut meta = chunk.metadata.clone();
            // The marker has its own top-level field; keeping it in the
            // residual map too would ship the same fact twice with two
            // possible readers disagreeing which one is authoritative.
            meta.remove(DEGRADED_REASON_KEY);
            // Repair empty-byline artifact in excerpt (#800) — defense in depth
            // on top of the origin repair in extraction.rs / scraper_service.rs.
            // The frontmatter path already repairs; JSONL must not ship raw
            // `by (about)` when the excerpt carries the scar. Idempotent.
            if let Some(raw_excerpt) = meta.get("excerpt").cloned() {
                let author = meta.get("author").map(|s| s.as_str());
                let repaired =
                    crate::domain::excerpt_repair::repair_empty_byline(&raw_excerpt, author);
                if repaired.is_empty() {
                    meta.remove("excerpt");
                } else if repaired != raw_excerpt {
                    meta.insert("excerpt".to_string(), repaired);
                }
            }
            if meta.is_empty() {
                None
            } else {
                // #1595 — collect into BTreeMap so serde_json emits keys in
                // lexicographic order, byte-stable across processes and runs.
                // Membership preserved exactly; only the container changes.
                Some(
                    meta.into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>(),
                )
            }
        };

        Self {
            url: &chunk.url,
            timestamp_utc: chunk.timestamp.to_rfc3339(),
            title: Some(&chunk.title),
            content: &chunk.content,
            checksum_sha256: checksum,
            metadata_version: "2.1.0",
            content_length: chunk.content.chars().count(),
            word_count,
            reading_time,
            language,
            content_type,
            scrape_date,
            degraded_reason,
            extra_metadata,
        }
    }
}
