//! HTML to Markdown conversion with a block-level source map.
//!
//! The engine is the [`HtmdConverter`](crate::infrastructure::converter::htmd_converter::HtmdConverter)
//! adapter (the repo's single converter path): HTML boilerplate (nav, sidebar,
//! SVG, scripts) is stripped before conversion for Obsidian-quality output.
//! This module owns the annotated whole-document-plus-block-map machinery.

use crate::domain::text_chunker::ByteSpan;
use crate::infrastructure::converter::htmd_converter::HtmdConverter;
use tracing::warn;

/// One top-level HTML block's footprint in the converted markdown.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MarkdownBlock {
    /// Byte range `[start, end)` of this block's markdown inside
    /// [`AnnotatedMarkdown::markdown`].
    pub markdown_span: ByteSpan,
    /// Source tag of the block (`h1`, `p`, `ul`, `table`, ...), or `#text`
    /// for loose text between elements.
    pub html_tag: String,
    /// 0-based position of the block among the top-level blocks of the
    /// cleaned HTML.
    pub html_index: usize,
}

/// Whole-document markdown plus its block-level source map.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AnnotatedMarkdown {
    /// The converted markdown — byte-identical to
    /// [`HtmdConverter::convert`](crate::infrastructure::converter::htmd_converter::HtmdConverter::convert)
    /// output for the same input, on every path.
    pub markdown: String,
    /// One entry per top-level HTML block, in document order. Empty when the
    /// per-block join could not reproduce the whole-document conversion:
    /// the map is never wrong, at worst absent.
    pub blocks: Vec<MarkdownBlock>,
}

/// Convert HTML to markdown annotated with a block-level source map
/// (markdown range → HTML block) — the seam the future "highlight in the
/// original HTML" consumer composes with the chunker's spans
/// (`chunk.spans` → markdown block → HTML element).
///
/// The pipeline is the single converter's; the cleaned HTML is additionally
/// split into top-level blocks, converted independently through
/// [`HtmdConverter::convert_cleaned`], and the per-block join is verified
/// against the whole-document result before any map is reported. When the
/// join diverges (inline flow that crosses block boundaries, conversion
/// fallbacks), the result degrades to the whole-document markdown with an
/// empty map.
pub fn convert_to_markdown_annotated(html: &str) -> AnnotatedMarkdown {
    let cleaned = crate::infrastructure::converter::html_cleaner::clean_html(html);
    let converter = HtmdConverter;
    let whole = converter.convert_cleaned(&cleaned, html);

    let Some(source_blocks) = split_top_level_blocks(&cleaned) else {
        return AnnotatedMarkdown {
            markdown: whole,
            blocks: Vec::new(),
        };
    };

    let mut markdown = String::new();
    let mut blocks = Vec::new();
    for (html_index, (tag, fragment)) in source_blocks.into_iter().enumerate() {
        // Pieces carry the same trailing newline the converter emits for the
        // whole document; trimmed so the "\n\n" separator is the only one.
        let piece = converter.convert_cleaned(fragment, html);
        let piece = piece.trim_end();
        if piece.is_empty() {
            continue;
        }
        if !markdown.is_empty() {
            markdown.push_str("\n\n");
        }
        let start = markdown.len();
        markdown.push_str(piece);
        let end = markdown.len();
        // start <= end by construction (the buffer only grows); the degrade
        // branch exists so an invariant regression degrades instead of
        // panicking or emitting a wrong span.
        let span = match ByteSpan::try_new(start, end) {
            Ok(span) => span,
            Err(_) => {
                return AnnotatedMarkdown {
                    markdown: whole,
                    blocks: Vec::new(),
                }
            },
        };
        blocks.push(MarkdownBlock {
            markdown_span: span,
            html_tag: tag,
            html_index,
        });
    }

    // The converter emits one document-level trailing newline; it belongs to
    // the document, not to the last block, so it is appended after the last
    // span was recorded.
    if whole.ends_with('\n') && !markdown.ends_with('\n') {
        markdown.push('\n');
    }

    if markdown != whole {
        warn!(
            whole_len = whole.len(),
            joined_len = markdown.len(),
            "per-block conversion join diverged from whole-document conversion; \
             emitting the markdown without a block map"
        );
        return AnnotatedMarkdown {
            markdown: whole,
            blocks: Vec::new(),
        };
    }

    AnnotatedMarkdown { markdown, blocks }
}

/// Split cleaned HTML into its top-level content blocks as `(tag, fragment)`
/// pairs. Returns `None` when the document cannot be split confidently (the
/// caller degrades to a map-less conversion). `<html>`/`<body>` wrappers are
/// unwrapped one level so real content blocks are reported.
fn split_top_level_blocks(html: &str) -> Option<Vec<(String, &str)>> {
    let scanned = scan_blocks(html)?;
    match scanned.as_slice() {
        [] => Some(Vec::new()),
        [(tag, fragment)] if tag == "html" || tag == "body" => {
            let inner = unwrap_element(fragment)?;
            scan_blocks(inner)
        },
        _ => Some(scanned),
    }
}

/// Strip one wrapping element's outer tags, returning its inner content.
fn unwrap_element(fragment: &str) -> Option<&str> {
    let open_end = fragment.find('>')?;
    let rest = &fragment[open_end + 1..];
    let close_start = rest.rfind("</")?;
    Some(&rest[..close_start])
}

const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Depth-counting top-level scanner. Respects attribute quotes, comments and
/// doctypes; returns `None` on any structure it cannot resolve (EOF inside a
/// tag, an unbalanced close) so the caller degrades instead of guessing.
fn scan_blocks(html: &str) -> Option<Vec<(String, &str)>> {
    let bytes = html.as_bytes();
    let mut blocks = Vec::new();
    let mut pos = 0usize;
    let mut text_start = None;

    while pos < bytes.len() {
        match bytes[pos] {
            b'<' => {
                if let Some(text) = take_loose_text(html, text_start.take(), pos)? {
                    blocks.push(("#text".to_owned(), text));
                }
                if html[pos..].starts_with("<!--") {
                    pos = html[pos..].find("-->").map(|end| pos + end + 3)?;
                    continue;
                }
                if html[pos..].starts_with("<!") || html[pos..].starts_with("<?") {
                    pos = html[pos..].find('>').map(|end| pos + end + 1)?;
                    continue;
                }
                let (tag, after_open) = read_open_tag(html, pos)?;
                if VOID_ELEMENTS.contains(&tag.as_str()) {
                    blocks.push((tag, &html[pos..after_open]));
                    pos = after_open;
                    continue;
                }
                let close = find_matching_close(html, after_open, &tag)?;
                blocks.push((tag, &html[pos..close.1]));
                pos = close.1;
            },
            b' ' | b'\t' | b'\n' | b'\r' => {
                text_start.get_or_insert(pos);
                pos += 1;
            },
            _ => {
                text_start.get_or_insert(pos);
                pos += 1;
            },
        }
    }
    if let Some(text) = take_loose_text(html, text_start.take(), pos)? {
        blocks.push(("#text".to_owned(), text));
    }
    Some(blocks)
}

/// Commit a pending loose-text block only when it holds non-whitespace.
fn take_loose_text(html: &str, text_start: Option<usize>, end: usize) -> Option<Option<&str>> {
    match text_start {
        Some(start) => {
            let text = &html[start..end];
            if text.trim().is_empty() {
                Some(None)
            } else {
                Some(Some(text))
            }
        },
        None => Some(None),
    }
}

/// Read an opening tag starting at `<`, returning `(tag_name, offset_after_open_tag)`.
fn read_open_tag(html: &str, start: usize) -> Option<(String, usize)> {
    let bytes = html.as_bytes();
    let mut i = start + 1;
    let name_start = i;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    let tag = html[name_start..i].to_ascii_lowercase();
    if tag.is_empty() {
        return None;
    }
    // Walk to the tag-closing '>' outside quoted attribute values.
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {},
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return Some((tag, i + 1)),
            None => {},
        }
        i += 1;
    }
    None
}

/// Find the offset just past this element's matching close tag.
fn find_matching_close(html: &str, after_open: usize, tag: &str) -> Option<(usize, usize)> {
    let bytes = html.as_bytes();
    let mut depth = 1usize;
    let mut pos = after_open;
    while pos < bytes.len() {
        if bytes[pos] != b'<' {
            pos += 1;
            continue;
        }
        if html[pos..].starts_with("<!--") {
            pos = html[pos..].find("-->").map(|end| pos + end + 3)?;
            continue;
        }
        if html[pos + 1..].starts_with('/') {
            let (close_tag, after) = read_close_tag(html, pos)?;
            if close_tag == tag {
                depth -= 1;
                if depth == 0 {
                    return Some((pos, after));
                }
            }
            pos = after;
            continue;
        }
        let (open_tag, after_open) = read_open_tag(html, pos)?;
        if !VOID_ELEMENTS.contains(&open_tag.as_str())
            && !html[after_open - 2..after_open].starts_with("/>")
            && open_tag == tag
        {
            depth += 1;
        }
        pos = after_open;
    }
    None
}

/// Read a closing tag starting at `</`, returning `(tag_name, offset_after_close)`.
fn read_close_tag(html: &str, start: usize) -> Option<(String, usize)> {
    let end = html[start..].find('>').map(|end| start + end + 1)?;
    let name = html[start + 2..end - 1].trim().to_ascii_lowercase();
    Some((name, end))
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::domain::ports::MarkdownConverter;

    /// The whole-document conversion under test — the single converter path
    /// (clean pre-pass + `htmd` + text fallback), exercised through the port.
    fn convert_to_markdown(html: &str) -> String {
        HtmdConverter.convert(html)
    }

    /// The locator invariant at the conversion seam: when the per-block join
    /// reproduces the whole-document conversion, every block's span must cut
    /// the annotated markdown at exactly the bytes that block produced.
    #[test]
    fn annotated_blocks_reconstruct_the_whole_document_markdown() {
        let html = "<h1>Título</h1><p>Primer párrafo.</p><p>Segundo párrafo.</p>";
        let annotated = convert_to_markdown_annotated(html);
        assert!(
            !annotated.blocks.is_empty(),
            "well-formed block HTML must yield a map, got: {annotated:?}"
        );
        assert_eq!(
            annotated.markdown,
            convert_to_markdown(html),
            "the annotated markdown must be the whole-document conversion"
        );
        for block in &annotated.blocks {
            let slice = &annotated.markdown[block.markdown_span.start()..block.markdown_span.end()];
            assert!(!slice.trim().is_empty(), "a block must own its bytes");
            assert!(
                !block.html_tag.is_empty(),
                "every block names its source tag"
            );
        }
        let heads: Vec<&str> = annotated
            .blocks
            .iter()
            .map(|b| b.html_tag.as_str())
            .collect();
        assert_eq!(
            heads,
            vec!["h1", "p", "p"],
            "one map entry per top-level block"
        );
    }

    #[test]
    fn annotated_conversion_degrades_honestly_when_join_diverges() {
        // Inline content spanning two top-level elements (a table split by a
        // heading inside a caption row boundary is contrived; the realistic
        // divergence trigger is whitespace-sensitive inline flow). Whatever
        // the trigger, the contract is: blocks empty, markdown still the
        // whole-document conversion, never a wrong map.
        let html = "texto suelto <b>con inline</b> y más<p>tras el texto</p>";
        let annotated = convert_to_markdown_annotated(html);
        assert_eq!(
            annotated.markdown,
            convert_to_markdown(html),
            "the markdown must always be the whole-document conversion"
        );
        for block in &annotated.blocks {
            let slice = &annotated.markdown[block.markdown_span.start()..block.markdown_span.end()];
            assert!(!slice.trim().is_empty());
        }
    }

    #[test]
    fn test_convert_heading() {
        let html = "<h1>Main Title</h1>";
        let md = convert_to_markdown(html);
        assert!(md.contains("# Main Title"));
    }

    #[test]
    fn test_convert_paragraph() {
        let html = "<p>This is a paragraph.</p>";
        let md = convert_to_markdown(html);
        assert!(md.contains("This is a paragraph."));
    }

    #[test]
    fn test_convert_nested_structure() {
        let html = "<article><h1>Title</h1><p>Intro</p><h2>Section</h2><p>Content</p></article>";
        let md = convert_to_markdown(html);
        assert!(md.contains("# Title"));
        assert!(md.contains("## Section"));
        assert!(md.contains("Intro"));
        assert!(md.contains("Content"));
    }

    #[test]
    fn test_convert_empty_html() {
        let html = "";
        let md = convert_to_markdown(html);
        assert_eq!(md, "");
    }

    #[test]
    fn test_code_block_uses_backticks() {
        let html = "<pre><code>fn main() {}</code></pre>";
        let md = convert_to_markdown(html);
        assert!(md.contains("```"), "Expected fenced code blocks, got: {md}");
    }

    #[test]
    fn test_boilerplate_removed() {
        let html = "<html><body><nav>Menu</nav><main><h1>Title</h1><p>Content</p></main><footer>Copyright</footer></body></html>";
        let md = convert_to_markdown(html);
        assert!(!md.contains("Menu"));
        assert!(!md.contains("Copyright"));
        assert!(md.contains("Title"));
        assert!(md.contains("Content"));
    }

    // ============================================================================
    // Error path tests
    // ============================================================================

    #[test]
    fn test_convert_links_to_markdown() {
        let html = r#"<p>Visit <a href="https://example.com">Example</a> for more info.</p>"#;
        let md = convert_to_markdown(html);
        assert!(
            md.contains("[Example](https://example.com)"),
            "link should be converted to markdown"
        );
    }

    #[test]
    fn test_convert_lists() {
        let html = r#"
            <ul>
                <li>Item 1</li>
                <li>Item 2</li>
                <li>Item 3</li>
            </ul>
            <ol>
                <li>First</li>
                <li>Second</li>
            </ol>
        "#;
        let md = convert_to_markdown(html);
        // Unordered list items should have bullet markers
        assert!(md.contains("Item 1"));
        assert!(md.contains("Item 2"));
        assert!(md.contains("Item 3"));
        // Ordered list items should be present
        assert!(md.contains("First"));
        assert!(md.contains("Second"));
    }

    #[test]
    fn test_convert_tables() {
        let html = r#"
            <table>
                <thead>
                    <tr><th>Name</th><th>Value</th></tr>
                </thead>
                <tbody>
                    <tr><td>Foo</td><td>42</td></tr>
                    <tr><td>Bar</td><td>99</td></tr>
                </tbody>
            </table>
        "#;
        let md = convert_to_markdown(html);
        // Table content should be present
        assert!(md.contains("Name"));
        assert!(md.contains("Value"));
        assert!(md.contains("Foo"));
        assert!(md.contains("Bar"));
        // Markdown tables use pipe separators
        assert!(md.contains("|"));
    }
}
