//! Text chunker port — domain trait for content segmentation.
//!
//! Defines the contract for splitting text into semantic, located chunks.
//! The concrete Markdown implementation moved to `webfang_core` when ADR-0004
//! paso 6 removed the `webfang_ai` crate; the HTML implementation uses
//! `HtmlChunker`.
//!
//! This trait enables the application layer to orchestrate chunking
//! without depending on the concrete implementation (dependency inversion).

use crate::error::SemanticError;

/// A byte range `[start, end)` into the exact text a [`TextChunker`] received.
///
/// Offsets are BYTES, not characters, and always land on UTF-8 character
/// boundaries: every span can slice the source text directly. This is the
/// only coordinate space a chunker can guarantee — a chunk assembled from
/// several blocks is not contiguous in the source, so [`TextChunk`] carries
/// one span per source piece instead of a single range.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ByteSpan {
    start: usize,
    end: usize,
}

impl ByteSpan {
    /// Validate and build a span; `start` must not exceed `end`.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::InvalidSpan`] when `start > end` — a
    /// programming bug in a chunker implementation, never user input.
    pub fn try_new(start: usize, end: usize) -> Result<Self, SemanticError> {
        if start > end {
            return Err(SemanticError::InvalidSpan { start, end });
        }
        Ok(Self { start, end })
    }

    /// The first byte of the span, inclusive.
    #[must_use]
    pub const fn start(&self) -> usize {
        self.start
    }

    /// The byte after the last byte of the span, exclusive.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.end
    }

    /// The span length in bytes (`end - start`).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the span covers zero bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// One output chunk: its embeddable text plus the source pieces it came from.
///
/// `text` is the chunk as produced (blocks re-joined with `"\n\n"`, pieces
/// trimmed); `spans` are the exact byte ranges of the input text each piece
/// was cut from, in order. Slicing the source with every span and re-joining
/// with `"\n\n"` reconstructs `text` — the locator invariant pinned by the
/// chunker tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    text: String,
    spans: Vec<ByteSpan>,
}

impl TextChunk {
    /// Assemble a chunk from its text and source spans.
    ///
    /// `spans` must be non-empty and resolve inside the text the chunker
    /// received. The source text is not carried here, so that invariant is
    /// the implementation's duty — the chunker tests pin it.
    #[must_use]
    pub fn new(text: impl Into<String>, spans: Vec<ByteSpan>) -> Self {
        Self {
            text: text.into(),
            spans,
        }
    }

    /// The chunk text, ready for embedding.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The source spans this chunk was assembled from, in order.
    #[must_use]
    pub fn spans(&self) -> &[ByteSpan] {
        &self.spans
    }
}

/// Domain trait for text segmentation into semantic located chunks.
///
/// Implementations split raw text into chunks suitable for embedding, each
/// carrying the byte spans of the source text it was assembled from
/// (see [`TextChunk`]). The trait is dyn-compatible (no generics, no `async`)
/// so it can be stored as `Arc<dyn TextChunker>` in the Container.
pub trait TextChunker: Send + Sync {
    /// Split text into semantic chunks with source locators.
    ///
    /// Returns each chunk's text plus the byte spans (`[start, end)`) of the
    /// pieces it was assembled from — offsets against the exact `text` passed
    /// in, always on UTF-8 character boundaries. Metadata (headings, source
    /// path, document reference) is the caller's responsibility.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Tokenize`] if the text is empty or
    /// cannot be processed.
    fn chunk_text(&self, text: &str) -> Result<Vec<TextChunk>, SemanticError>;
}
