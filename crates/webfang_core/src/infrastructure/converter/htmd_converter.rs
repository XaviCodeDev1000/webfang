//! The production [`MarkdownConverter`]: `html_cleaner` pre-pass + `htmd`.
//!
//! One engine for the whole repo (#2360): boilerplate is stripped first
//! (navigation, scripts, page chrome), the cleaned HTML goes through `htmd`
//! with ATX headings and backtick-fenced code blocks (its defaults), and a
//! failed conversion degrades to plain text extraction so the contract
//! stays total.

use crate::domain::html_extraction::fallback;
use crate::domain::ports::MarkdownConverter;
use tracing::warn;

/// Adapter converting HTML to Markdown through the `htmd` engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct HtmdConverter;

impl HtmdConverter {
    /// Convert already-clean HTML (the `html_cleaner` pre-pass already ran).
    ///
    /// Exposed for the annotated path, which splits the cleaned document into
    /// top-level blocks and converts each fragment: re-cleaning a fragment is
    /// not guaranteed idempotent, so the pre-pass runs exactly once.
    /// `original_html` feeds the text fallback (the fallback never sees
    /// per-block fragments).
    pub fn convert_cleaned(&self, cleaned: &str, original_html: &str) -> String {
        htmd::convert(cleaned).unwrap_or_else(|e| {
            warn!(
                error = %e,
                "HTML to Markdown conversion failed; falling back to text extraction"
            );
            fallback::extract_text(original_html)
        })
    }
}

impl MarkdownConverter for HtmdConverter {
    fn convert(&self, html: &str) -> String {
        let cleaned = crate::infrastructure::converter::html_cleaner::clean_html(html);
        self.convert_cleaned(&cleaned, html)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_headings_to_atx() {
        let md = HtmdConverter.convert("<h1>Main Title</h1>");
        assert!(md.contains("# Main Title"), "got: {md}");
    }

    #[test]
    fn converts_code_blocks_to_backtick_fences() {
        let md = HtmdConverter.convert("<pre><code>fn main() {}</code></pre>");
        assert!(md.contains("```"), "expected fenced code, got: {md}");
    }

    #[test]
    fn strips_boilerplate_before_converting() {
        let html = "<html><body><nav>Menu</nav><main><h1>Title</h1></main>\
                    <footer>Copyright</footer></body></html>";
        let md = HtmdConverter.convert(html);
        assert!(!md.contains("Menu"), "nav must not survive: {md}");
        assert!(!md.contains("Copyright"), "footer must not survive: {md}");
        assert!(md.contains("Title"), "content must survive: {md}");
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(HtmdConverter.convert(""), "");
    }

    #[test]
    fn malformed_html_still_yields_text() {
        let md = HtmdConverter.convert("<div>Open div never closed<p>Paragraph");
        assert!(!md.is_empty(), "the total contract must hold");
    }

    #[test]
    fn is_usable_as_a_dyn_port() {
        let converter: Box<dyn MarkdownConverter> = Box::new(HtmdConverter);
        assert!(converter.convert("<h1>T</h1>").contains("# T"));
    }
}
