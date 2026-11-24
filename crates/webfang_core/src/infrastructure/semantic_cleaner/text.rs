//! HTML-to-text extraction and paragraph chunking for the semantic cleaner
//! (ADR-0004 step 6). Extracted from `semantic_cleaner.rs` without behavior
//! changes.

use crate::domain::text_chunker::{ByteSpan, TextChunk};
use crate::error::SemanticError;
use crate::infrastructure::content_processing::aggressive::strip_html_tags;

/// Remove HTML tags from a `legible` fragment while preserving its whitespace.
///
/// [`legible::parse`] returns a readability HTML fragment (`<div …><p>…</p></div>`),
/// not plain text. The degraded fallback uses [`strip_html_tags`], which drops
/// script/style/noscript bodies but collapses ALL whitespace; applying it to
/// the fragment would erase the newlines the chunker reads as paragraph
/// boundaries. This helper keeps that layout intact while enforcing the same
/// embedding boundary: no raw tag may reach a model client (S4 / T11).
pub(super) fn strip_tags_keep_layout(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match (in_tag, ch) {
            (false, '<') => in_tag = true,
            (true, '>') => in_tag = false,
            (true, _) => {},
            (false, _) => out.push(ch),
        }
    }
    out
}

/// Extract readable text from `html`, falling back to the shared block-dropping
/// stripper when `legible` fails or yields nothing but whitespace.
///
/// Both branches are an embedding boundary: no raw tag may reach a model client
/// (S4 / T11). The fallback must additionally drop script/style/noscript
/// bodies before any text reaches
/// [`EmbeddingPort::embed_batch`](crate::domain::embedding_port::EmbeddingPort::embed_batch) (#3009).
pub(super) fn extract_text_content(html: &str) -> String {
    match legible::parse(html, None, None) {
        Ok(article) if !article.content.trim().is_empty() => {
            strip_tags_keep_layout(&article.content)
        },
        _ => strip_html_tags(html),
    }
}

/// Split `text` into chunks joined from non-empty paragraphs, staying under
/// `max_chars` when possible.
///
/// A single paragraph longer than the budget cannot be split honestly, so it
/// fails with [`SemanticError::ChunkTooLarge`] — the caller layer degrades
/// the page instead of writing an unembeddable chunk.
///
/// #2429 AC2: each chunk also carries the exact byte spans of its paragraphs
/// inside `text` (half-open `[start, end)`, whitespace-trimmed boundaries) —
/// the locator the vector sink stamps into every [`VectorRecord`].
pub(super) fn split_into_chunks(
    text: &str,
    max_chars: usize,
) -> Result<Vec<TextChunk>, SemanticError> {
    // Paragraphs as trimmed `(start, end)` byte spans into `text`.
    let mut paragraphs: Vec<(usize, usize)> = Vec::new();
    let mut rest = text;
    let mut offset = 0;
    loop {
        let line_start = offset;
        let line = match rest.find('\n') {
            Some(nl) => {
                let line = &rest[..nl];
                rest = &rest[nl + 1..];
                offset += nl + 1;
                line
            },
            None => {
                let line = rest;
                rest = "";
                line
            },
        };
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let start = line_start + (trimmed.as_ptr() as usize - line.as_ptr() as usize);
            paragraphs.push((start, start + trimmed.len()));
        }
        if rest.is_empty() {
            break;
        }
    }

    let mut chunks: Vec<TextChunk> = Vec::new();
    let mut current = String::new();
    let mut current_spans: Vec<ByteSpan> = Vec::new();
    for (start, end) in paragraphs {
        let paragraph = &text[start..end];
        if !current.is_empty()
            && current.chars().count() + paragraph.chars().count() + 1 > max_chars
        {
            chunks.push(TextChunk::new(
                std::mem::take(&mut current),
                std::mem::take(&mut current_spans),
            ));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(paragraph);
        current_spans.push(ByteSpan::try_new(start, end)?);
    }
    if !current.is_empty() {
        chunks.push(TextChunk::new(current, current_spans));
    }

    for (chunk_id, chunk) in chunks.iter().enumerate() {
        if chunk.text().chars().count() > max_chars {
            return Err(SemanticError::ChunkTooLarge {
                chunk_id: chunk_id.to_string(),
                chars: chunk.text().len(),
                max: max_chars,
            });
        }
    }

    Ok(chunks)
}
