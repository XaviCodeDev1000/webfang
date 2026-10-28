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

use crate::domain::text_chunker::TextChunker;
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
    fn chunk_text(&self, text: &str) -> Result<Vec<String>, SemanticError> {
        if text.trim().is_empty() {
            return Err(SemanticError::Tokenize(
                "el texto a segmentar está vacío".to_string(),
            ));
        }

        let mut chunks: Vec<String> = Vec::new();
        let mut current = String::new();

        for block in blocks(text) {
            let block_len = block.chars().count();
            let joined_len = current.chars().count() + 2 + block_len;

            if !current.is_empty() && joined_len > self.max_chars {
                chunks.push(std::mem::take(&mut current));
            }

            if block_len > self.max_chars {
                if !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                }
                chunks.extend(split_oversized(&block, self.max_chars));
                continue;
            }

            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(&block);
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        Ok(chunks)
    }
}

/// Split Markdown into blank-line-separated blocks, trimmed, empties dropped.
fn blocks(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .map(str::to_string)
        .collect()
}

/// Hard-split one oversized block, preferring a newline or space near the
/// boundary so the cut lands between words when one is available.
fn split_oversized(block: &str, max_chars: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = block;

    while !rest.is_empty() {
        if rest.chars().count() <= max_chars {
            pieces.push(rest.to_string());
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
        let piece = piece.trim();
        if !piece.is_empty() {
            pieces.push(piece.to_string());
        }
        rest = tail.trim_start();
    }

    pieces
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
        assert!(chunks[0].contains("Un párrafo corto."));
    }

    #[test]
    fn paragraphs_are_packed_up_to_the_budget() {
        let paragraph = "a".repeat(50);
        let text = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}");
        let chunks = chunker().chunk_text(&text).expect("chunks");
        assert!(chunks.len() > 1, "three 50-char blocks must not fit in 120");
        for chunk in &chunks {
            assert!(
                chunk.chars().count() <= 120,
                "chunk over budget: {} chars",
                chunk.chars().count()
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
                chunk.chars().count() <= 120,
                "chunk over budget: {} chars",
                chunk.chars().count()
            );
        }
        let joined = chunks.join(" ");
        for word in &words {
            assert!(joined.contains(word.as_str()), "lost token {word}");
        }
    }

    #[test]
    fn tiny_budget_is_clamped() {
        assert_eq!(MarkdownChunker::new(0).max_chars(), MIN_CHUNK_CHARS);
        assert_eq!(MarkdownChunker::new(1).max_chars(), MIN_CHUNK_CHARS);
        assert_eq!(MarkdownChunker::new(500).max_chars(), 500);
    }
}
