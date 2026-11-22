//! The DocumentChunk entity — the RAG-pipeline chunk typestate.
//!
//! The page-level entity it is built from, [`ScrapedContent`], lives in
//! [`super::scraped_content`] (Ola 3 B4, #2966); this module owns the
//! `Draft → Validated → Exported` typestate machine and the conversions.

use std::collections::HashMap;
use std::marker::PhantomData;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::text_chunker::ByteSpan;
use crate::domain::CorrelationId;

/// Re-exported for backward compatibility: `entities::content::ScrapedContent`
/// is a documented external path (consumers outside this crate reach it by
/// full path), so the move to [`super::scraped_content`] keeps it resolvable.
pub use super::scraped_content::ScrapedContent;

// ============================================================================
// Validation Errors
// ============================================================================

/// Errors that can occur during DocumentChunk validation
///
/// Domain errors define WHAT failed, not HOW to present to users.
/// Error messages are neutral identifiers for programmatic handling.
/// Presentation layer (CLI/TUI) maps these to user-friendly messages.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// Content body is empty or whitespace-only.
    #[error("empty_content")]
    EmptyContent,

    /// Title is empty or whitespace-only.
    #[error("empty_title")]
    EmptyTitle,

    /// URL failed to parse as a valid HTTP/HTTPS URL.
    #[error("invalid_url: {0}")]
    InvalidUrl(String),

    /// A metadata value is empty or whitespace-only.
    #[error("invalid_metadata: {0}")]
    InvalidMetadata(String),
}

// ============================================================================
// Typestate Markers — Private state types for DocumentChunk
// ============================================================================

/// Private state marker: DocumentChunk is newly created, not validated
/// No public constructor - only created via `From<ScrapedContent>`
#[derive(Clone, Copy)]
pub struct Draft;

/// Private state marker: DocumentChunk has passed validation checks
/// Can be exported to disk
#[derive(Clone, Copy)]
pub struct Validated;

/// Private state marker: DocumentChunk has been exported
/// Metadata includes export path
#[derive(Clone, Copy)]
pub struct Exported;

/// Constructor for DocumentChunk<Draft> that bypasses From<ScrapedContent> (tests only)
#[cfg(test)]
impl DocumentChunk<Draft> {
    /// Create a test DocumentChunk with the given fields.
    pub fn test_new(
        id: Uuid,
        url: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id,
            url: url.into(),
            title: title.into(),
            content: content.into(),
            metadata: HashMap::new(),
            timestamp: Utc::now(),
            embeddings: None,
            chunk_spans: None,
            correlation_id: None,
            _state: PhantomData,
        }
    }
}

/// Public constructor for `DocumentChunk<Draft>` in production code
/// Required for modules that create DocumentChunk directly (e.g., chunker, relevance_scorer)
impl DocumentChunk<Draft> {
    /// Create a new [`DocumentChunk<Draft>`](DocumentChunk) from individual fields.
    pub fn new(
        id: Uuid,
        url: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id,
            url: url.into(),
            title: title.into(),
            content: content.into(),
            metadata: HashMap::new(),
            timestamp: Utc::now(),
            embeddings: None,
            chunk_spans: None,
            correlation_id: None,
            _state: PhantomData,
        }
    }
}

/// Constructor with metadata for chunks that need to preserve source metadata
impl DocumentChunk<Draft> {
    /// Create a new [`DocumentChunk<Draft>`](DocumentChunk) with pre-populated metadata.
    pub fn with_metadata(
        id: Uuid,
        url: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
        metadata: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            url: url.into(),
            title: title.into(),
            content: content.into(),
            metadata,
            timestamp: Utc::now(),
            embeddings: None,
            chunk_spans: None,
            correlation_id: None,
            _state: PhantomData,
        }
    }
}

/// A document chunk ready for export to RAG pipelines
///
/// Represents a single unit of content that can be:
/// - Embedded in a vector database
/// - Exported in various formats (JSONL, Markdown)
///
/// The embeddings field is optional because in the initial scraping phase
/// the AI model may not be available yet. It can be populated later
/// by a separate embedding pipeline.
///
/// Uses typestate pattern: must call `.validate()` before export.
///
/// ```compile_fail
/// use webfang_core::domain::DocumentChunkUnvalidated;
/// use webfang_core::domain::Exporter;
/// use webfang_core::domain::exporter::ExporterConfig;
/// use webfang_core::infrastructure::export::FileExporter;
/// use webfang_core::ExportFormat;
/// use uuid::Uuid;
/// use std::path::PathBuf;
///
/// // DocumentChunk<Draft> cannot be compiled into the export path.
/// // The typestate pattern enforces .validate() before export at compile time.
/// let config = ExporterConfig::new(PathBuf::from("/tmp"), ExportFormat::Jsonl, "test");
/// let exporter = FileExporter::new(config);
/// let chunk = DocumentChunkUnvalidated::new(
///     Uuid::new_v4(),
///     "https://example.com",
///     "Test Title",
///     "Test content",
/// );
///
/// // This MUST fail: Exporter::export() expects DocumentChunk<Validated>
/// let _ = exporter.export(chunk);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk<S = Draft> {
    /// Unique identifier for this chunk (UUID v4)
    pub id: Uuid,
    /// Source URL where this content was scraped from
    pub url: String,
    /// Title of the source page/article
    pub title: String,
    /// The actual text content (cleaned, ready for embedding)
    pub content: String,
    /// Additional metadata extracted during scraping
    /// Keys: author, date, excerpt, domain, etc.
    pub metadata: HashMap<String, String>,
    /// Timestamp when this content was scraped (UTC)
    pub timestamp: DateTime<Utc>,
    /// Optional embedding vector (for vector database storage)
    /// Populated by embedding pipeline after initial scrape
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<Vec<f32>>,
    /// Byte spans of this chunk inside the text the chunker segmented
    /// (#2429 AC2), half-open `[start, end)`. `None` when the chunk has no
    /// locator information (e.g. built from `ScrapedContent` directly, or the
    /// CPU path): the offsets are against the chunker's input text, and the
    /// raw-HTML mapping is the deferred Option 3 of the chunk-locators ADR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_spans: Option<Vec<ByteSpan>>,
    /// W3C TraceContext correlation ID for distributed tracing
    /// Optional - when present, enables request correlation across services
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Typestate marker (crate-visible for tests)
    #[serde(skip)]
    pub(crate) _state: PhantomData<S>,
}

/// Alias for backward compatibility - DocumentChunk in Draft state
/// Use `DocumentChunk<Draft>` for new code, DocumentChunk for existing code
pub type DocumentChunkUnvalidated = DocumentChunk<Draft>;
/// Type alias for a [`DocumentChunk`] that has passed validation.
pub type DocumentChunkValidated = DocumentChunk<Validated>;
/// Type alias for a [`DocumentChunk`] that has been exported.
pub type DocumentChunkExported = DocumentChunk<Exported>;

// NOTE: DocumentChunk (non-generic) is re-exported from domain/mod.rs for backward compatibility

/// Conversion from ScrapedContent creates DocumentChunk in Draft state
impl From<ScrapedContent> for DocumentChunk<Draft> {
    fn from(scraped: ScrapedContent) -> Self {
        let mut metadata = HashMap::new();

        if let Some(excerpt) = scraped.excerpt {
            metadata.insert("excerpt".to_string(), excerpt);
        }
        if let Some(author) = scraped.author {
            metadata.insert("author".to_string(), author);
        }
        if let Some(date) = scraped.date {
            metadata.insert("date".to_string(), date);
        }

        if let Ok(url) = url::Url::parse(&scraped.url.to_string()) {
            if let Some(host) = url.host_str() {
                metadata.insert("domain".to_string(), host.to_string());
            }
        }

        Self {
            id: Uuid::new_v4(),
            url: scraped.url.to_string(),
            title: scraped.title,
            content: scraped.content,
            metadata,
            timestamp: Utc::now(),
            embeddings: None,
            chunk_spans: None,
            correlation_id: scraped.correlation_id.map(|c| c.to_traceparent()),
            _state: PhantomData,
        }
    }
}

impl DocumentChunk<Draft> {
    /// Create a new DocumentChunk from ScrapedContent (Draft state)
    ///
    /// This is the main conversion method from the scraper's output
    /// to the RAG pipeline's input format.
    #[must_use]
    pub fn from_scraped_content(scraped: &ScrapedContent) -> Self {
        Self::from_scraped_content_with_correlation(scraped, scraped.correlation_id.as_ref())
    }

    /// Create a new DocumentChunk from ScrapedContent with correlation ID
    ///
    /// Includes W3C traceparent for distributed tracing.
    #[must_use]
    pub fn from_scraped_content_with_correlation(
        scraped: &ScrapedContent,
        correlation_id: Option<&CorrelationId>,
    ) -> Self {
        let mut metadata = HashMap::new();

        // Extract optional fields into metadata HashMap
        if let Some(ref excerpt) = scraped.excerpt {
            metadata.insert("excerpt".to_string(), excerpt.clone());
        }
        if let Some(ref author) = scraped.author {
            metadata.insert("author".to_string(), author.clone());
        }
        if let Some(ref date) = scraped.date {
            metadata.insert("date".to_string(), date.clone());
        }

        // Extract domain from URL
        if let Ok(url) = url::Url::parse(&scraped.url.to_string()) {
            if let Some(host) = url.host_str() {
                metadata.insert("domain".to_string(), host.to_string());
            }
        }

        Self {
            id: Uuid::new_v4(),
            url: scraped.url.to_string(),
            title: scraped.title.clone(),
            content: scraped.content.clone(),
            metadata,
            timestamp: Utc::now(),
            embeddings: None,
            chunk_spans: None,
            correlation_id: correlation_id.map(|c| c.to_traceparent()),
            _state: PhantomData,
        }
    }

    /// Create a new DocumentChunk with custom ID
    ///
    /// Use this when you need to preserve a specific ID (e.g., from a database)
    #[must_use]
    pub fn with_id(scraped: &ScrapedContent, id: Uuid) -> Self {
        let mut chunk = Self::from_scraped_content(scraped);
        chunk.id = id;
        chunk
    }

    /// Set embeddings for this chunk
    ///
    /// Typically called by the embedding pipeline after generating vectors
    #[must_use]
    pub fn with_embeddings(mut self, embeddings: Vec<f32>) -> Self {
        self.embeddings = Some(embeddings);
        self
    }

    /// Attach the byte spans this chunk was cut from (#2429 AC2).
    ///
    /// Called by chunkers that track offsets into their input text; the
    /// offsets are against that input, not against any other representation
    /// of the document.
    #[must_use]
    pub fn with_chunk_spans(mut self, spans: Vec<ByteSpan>) -> Self {
        self.chunk_spans = Some(spans);
        self
    }

    /// Enrich this AI-generated chunk with url/title/metadata from its source page.
    ///
    /// The semantic cleaner operates on raw HTML and produces `DocumentChunk`s
    /// with empty `url`/`title`. Before these chunks can pass `validate()` and
    /// be exported, they must inherit identity from the `ScrapedContent` they
    /// originated from. This method performs that enrichment in place.
    ///
    /// Only unambiguous page-level facts are merged into the chunk metadata:
    /// `url`, `title`, `date`, and `domain`. The page-level `author` and
    /// `excerpt` are intentionally NOT copied — they describe the whole page,
    /// not the individual AI-chunk's content, and a wrong (page-inherited)
    /// author is worse than an absent one in a RAG corpus. AI-side entries
    /// already present are preserved.
    #[must_use]
    pub fn enrich_from_scraped_content(mut self, scraped: &ScrapedContent) -> Self {
        self.url = scraped.url.to_string();
        self.title = scraped.title.clone();

        // Merge page-level facts (date/domain) from the scraped page,
        // preserving any AI-side entries. `author`/`excerpt` are deliberately
        // omitted — see the doc comment above.
        if let Some(ref date) = scraped.date {
            self.metadata
                .entry("date".to_string())
                .or_insert_with(|| date.clone());
        }
        if let Ok(url) = url::Url::parse(&scraped.url.to_string()) {
            if let Some(host) = url.host_str() {
                self.metadata
                    .entry("domain".to_string())
                    .or_insert_with(|| host.to_string());
            }
        }

        // Propagate per-page trace identity (same policy as `From<ScrapedContent>`):
        // a chunk without its own correlation inherits the page's traceparent,
        // keeping vector exports joinable with the JSONL trace (#843).
        if self.correlation_id.is_none() {
            self.correlation_id = scraped.correlation_id.as_ref().map(|c| c.to_traceparent());
        }

        self
    }

    /// Validate this Draft DocumentChunk
    ///
    /// Returns Validated state if content is valid:
    /// - content is not empty
    /// - title is not empty
    /// - URL is a valid HTTP/HTTPS URL
    /// - metadata values are reasonable
    ///
    /// # Errors
    /// Returns ValidationError if validation fails
    pub fn validate(self) -> Result<DocumentChunkValidated, ValidationError> {
        // Validation: content must not be empty
        if self.content.trim().is_empty() {
            return Err(ValidationError::EmptyContent);
        }

        // Validation: title must not be empty
        if self.title.trim().is_empty() {
            return Err(ValidationError::EmptyTitle);
        }

        // Validation: URL must be valid
        if let Err(e) = url::Url::parse(&self.url) {
            return Err(ValidationError::InvalidUrl(e.to_string()));
        }

        // Validation: metadata values should not be empty strings
        for (key, value) in &self.metadata {
            if value.trim().is_empty() {
                return Err(ValidationError::InvalidMetadata(format!(
                    "metadata key '{key}' has empty value"
                )));
            }
        }

        // Pure move: consume self (Draft state) and produce Validated state
        // No clones - all fields are moved from self to the new instance
        Ok(DocumentChunk {
            id: self.id,
            url: self.url,
            title: self.title,
            content: self.content,
            metadata: self.metadata,
            timestamp: self.timestamp,
            embeddings: self.embeddings,
            chunk_spans: self.chunk_spans,
            correlation_id: self.correlation_id,
            _state: PhantomData,
        })
    }
}

/// Methods available for any DocumentChunk state
impl<S> DocumentChunk<S> {
    /// Check if this chunk has embeddings
    #[must_use]
    pub fn has_embeddings(&self) -> bool {
        self.embeddings.is_some()
    }

    /// Get the text length (character count)
    #[must_use]
    pub fn text_length(&self) -> usize {
        self.content.len()
    }
}

/// Methods for Validated DocumentChunk
impl DocumentChunk<Validated> {
    /// Export this Validated DocumentChunk
    ///
    /// This method is only available for Validated state.
    /// Ensures content has passed validation before export.
    pub fn export(&self) -> &Self
    where
        Self: Send + Sync + 'static,
    {
        // Already validated, just return reference
        self
    }
}

#[cfg(test)]
#[path = "scraped_content_tests.rs"]
mod scraped_content_tests;

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
