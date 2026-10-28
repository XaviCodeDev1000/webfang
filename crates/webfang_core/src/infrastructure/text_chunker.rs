//! Markdown text chunker — the production [`TextChunker`] adapter.
//!
//! The vault-search path needs to split a note into embeddable pieces before
//! vectorising it. This is that splitter: paragraph-aware, character-budgeted
//! and deterministic.
//!
//! # Why characters and not tokens
//!
//! ADR-0004 removed the local tokenizer along with the ONNX stack. The only
//! budget every embedding backend can be told about is characters, which is
//! also what `--max-chars` carries, so the chunker and the cleaner speak the
//! same unit.

use crate::domain::text_chunker::{ByteSpan, TextChunk, TextChunker};
use crate::error::SemanticError;

/// Smallest budget the chunker will honour.
///
/// A budget below this cannot hold a sentence, so clamping here is the
/// difference between "an operator typed a silly number" and "every chunk is
/// useless". The clamp is silent by design: it never rejects work that the
/// caller asked for.
const MIN_CHUNK_CHARS: usize = 64;

/// Budget used when the caller does not name one (mirrors the `--max-chars`
/// default so an unwired caller and a defaulted run behave identically).
const DEFAULT_CHUNK_CHARS: usize = 4096;

/// Splits Markdown into chunks of at most `max_chars` characters.
#[derive(Debug, Clone)]
pub struct MarkdownChunker {
    max_chars: usize,
}

impl MarkdownChunker {
    /// Build a chunker with a character budget.
    ///
    /// `max_chars` is clamped to `MIN_CHUNK_CHARS`; see that constant for
    /// why the clamp is silent.
    #[must_use]
    pub fn new(max_chars: usize) -> Self {
        Self {
            max_chars: max_chars.max(MIN_CHUNK_CHARS),
        }
    }

    /// The effective budget, after clamping.
    #[must_use]
    pub fn max_chars(&self) -> usize {
        self.max_chars
    }
}

impl Default for MarkdownChunker {
    fn default() -> Self {
        Self::new(DEFAULT_CHUNK_CHARS)
    }
}

impl TextChunker for MarkdownChunker {
    fn chunk_text(&self, text: &str) -> Result<Vec<TextChunk>, SemanticError> {
        if text.trim().is_empty() {
            return Err(SemanticError::Tokenize(
                "el texto a segmentar está vacío".to_string(),
            ));
        }

        let mut chunks: Vec<TextChunk> = Vec::new();
        let mut current = String::new();
        let mut current_spans: Vec<ByteSpan> = Vec::new();

        for (block, span) in blocks(text)? {
            let block_len = block.chars().count();
            let joined_len = current.chars().count() + 2 + block_len;

            if !current.is_empty() && joined_len > self.max_chars {
                chunks.push(TextChunk::new(
                    std::mem::take(&mut current),
                    std::mem::take(&mut current_spans),
                ));
            }

            if block_len > self.max_chars {
                if !current.is_empty() {
                    chunks.push(TextChunk::new(
                        std::mem::take(&mut current),
                        std::mem::take(&mut current_spans),
                    ));
                }
                chunks.extend(split_oversized(block, span, self.max_chars)?);
                continue;
            }

            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(block);
            current_spans.push(span);
        }

        if !current.is_empty() {
            chunks.push(TextChunk::new(current, current_spans));
        }

        Ok(chunks)
    }
}

/// Split Markdown into blank-line-separated blocks, trimmed, empties dropped.
///
/// Each block travels with the byte span of its trimmed text against the
/// source: the span shrinks with the trim, so it always points at the exact
/// bytes the chunk text was cut from.
fn blocks(text: &str) -> Result<Vec<(&str, ByteSpan)>, SemanticError> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut offset = 0;
    while let Some(pos) = rest.find("\n\n") {
        push_block(&mut out, &rest[..pos], offset)?;
        rest = &rest[pos + 2..];
        offset += pos + 2;
    }
    push_block(&mut out, rest, offset)?;
    Ok(out)
}

/// Trim one raw block and store it with the span of its trimmed bytes.
fn push_block<'a>(
    out: &mut Vec<(&'a str, ByteSpan)>,
    raw: &'a str,
    offset: usize,
) -> Result<(), SemanticError> {
    let lead = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let span = ByteSpan::try_new(offset + lead, offset + lead + trimmed.len())?;
    out.push((trimmed, span));
    Ok(())
}

/// Hard-split one oversized block, preferring a newline or space near the
/// boundary so the cut lands between words when one is available.
///
/// Each piece becomes its own single-span chunk: a piece is one contiguous
/// cut of the source, so its span resolves exactly against the input text.
fn split_oversized(
    block: &str,
    block_span: ByteSpan,
    max_chars: usize,
) -> Result<Vec<TextChunk>, SemanticError> {
    let mut pieces: Vec<TextChunk> = Vec::new();
    let mut rest = block;
    // Byte offset of `rest` within `block` (`block` starts at `block_span`).
    let mut rest_start = 0;

    while !rest.is_empty() {
        if rest.chars().count() <= max_chars {
            push_piece(&mut pieces, rest, block_span.start() + rest_start)?;
            break;
        }

        let mut cut = 0;
        let mut last_break: Option<usize> = None;
        for (idx, ch) in rest.char_indices() {
            if idx >= max_chars {
                break;
            }
            cut = idx + ch.len_utf8();
            if ch == '\n' || ch == ' ' {
                last_break = Some(cut);
            }
        }
        // Only accept a word boundary if it keeps the piece reasonably full;
        // otherwise cut exactly at the budget.
        let cut = match last_break {
            Some(pos) if pos >= max_chars / 2 => pos,
            _ => cut,
        };
        if cut == 0 {
            break;
        }

        let (piece, tail) = rest.split_at(cut);
        push_piece(&mut pieces, piece, block_span.start() + rest_start)?;
        rest_start += cut;
        rest = tail;
        let lead = rest.len() - rest.trim_start().len();
        rest = rest.trim_start();
        rest_start += lead;
    }

    Ok(pieces)
}

/// Trim one piece and store it as a single-span chunk whose span covers the
/// trimmed bytes. Within an oversized block the trim is a no-op (the block
/// arrives already trimmed), but the span stays honest regardless.
fn push_piece(
    pieces: &mut Vec<TextChunk>,
    raw: &str,
    raw_start: usize,
) -> Result<(), SemanticError> {
    let lead = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let span = ByteSpan::try_new(raw_start + lead, raw_start + lead + trimmed.len())?;
    pieces.push(TextChunk::new(trimmed, vec![span]));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunker() -> MarkdownChunker {
        MarkdownChunker::new(120)
    }

    #[test]
    fn empty_text_is_a_tokenize_error() {
        let err = chunker()
            .chunk_text("   \n\n  ")
            .expect_err("empty text must be rejected");
        assert!(matches!(err, SemanticError::Tokenize(_)), "got: {err:?}");
    }

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = chunker()
            .chunk_text("# Título\n\nUn párrafo corto.")
            .expect("short text chunks");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text().contains("Un párrafo corto."));
    }

    #[test]
    fn paragraphs_are_packed_up_to_the_budget() {
        let paragraph = "a".repeat(50);
        let text = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}");
        let chunks = chunker().chunk_text(&text).expect("chunks");
        assert!(chunks.len() > 1, "three 50-char blocks must not fit in 120");
        for chunk in &chunks {
            assert!(
                chunk.text().chars().count() <= 120,
                "chunk over budget: {} chars",
                chunk.text().chars().count()
            );
        }
    }

    #[test]
    fn oversized_block_is_hard_split_without_losing_text() {
        let words: Vec<String> = (0..200).map(|i| format!("palabra{i}")).collect();
        let text = words.join(" ");
        let chunks = chunker().chunk_text(&text).expect("chunks");
        assert!(chunks.len() > 1, "a 200-word block must split");
        for chunk in &chunks {
            assert!(
                chunk.text().chars().count() <= 120,
                "chunk over budget: {} chars",
                chunk.text().chars().count()
            );
        }
        let joined = chunks
            .iter()
            .map(TextChunk::text)
            .collect::<Vec<_>>()
            .join(" ");
        for word in &words {
            assert!(joined.contains(word.as_str()), "lost token {word}");
        }
    }

    /// The locator invariant: slicing the source with each chunk's spans and
    /// re-joining with the same "\n\n" the chunker uses must reconstruct the
    /// chunk text exactly — the offsets are real, not approximations.
    fn assert_spans_reconstruct(chunks: &[TextChunk], source: &str) {
        for chunk in chunks {
            let slices: Vec<&str> = chunk
                .spans()
                .iter()
                .map(|span| &source[span.start()..span.end()])
                .collect();
            assert!(
                !chunk.spans().is_empty(),
                "every chunk must carry at least one span: {:?}",
                chunk.text()
            );
            assert_eq!(
                slices.join("\n\n"),
                chunk.text(),
                "spans must reconstruct the chunk text against the source"
            );
        }
    }

    #[test]
    fn spans_reconstruct_a_joined_chunk_against_the_source() {
        let text = "# Título\n\nPrimer párrafo.\n\nSegundo párrafo.";
        let chunks = chunker().chunk_text(text).expect("chunks");
        assert_eq!(chunks.len(), 1, "three short blocks fit in one chunk");
        assert_eq!(chunks[0].spans().len(), 3, "three blocks, three spans");
        assert_spans_reconstruct(&chunks, text);
    }

    #[test]
    fn spans_survive_trim_surrounding_whitespace_and_multibyte_text() {
        let text = "  \n\n\n\n  Párrafo con acentos: áéíóú.  \n\n  Emoji 🚀 al final.";
        let chunks = chunker().chunk_text(text).expect("chunks");
        assert_spans_reconstruct(&chunks, text);
    }

    #[test]
    fn oversized_pieces_carry_a_span_resolvable_against_the_source() {
        let words: Vec<String> = (0..200).map(|i| format!("palabra{i}")).collect();
        let text = words.join(" ");
        let chunks = chunker().chunk_text(&text).expect("chunks");
        for chunk in &chunks {
            assert_eq!(
                chunk.spans().len(),
                1,
                "an oversized piece is one contiguous cut of the source"
            );
        }
        assert_spans_reconstruct(&chunks, &text);
    }

    #[test]
    fn tiny_budget_is_clamped() {
        assert_eq!(MarkdownChunker::new(0).max_chars(), MIN_CHUNK_CHARS);
        assert_eq!(MarkdownChunker::new(1).max_chars(), MIN_CHUNK_CHARS);
        assert_eq!(MarkdownChunker::new(500).max_chars(), 500);
    }
}
