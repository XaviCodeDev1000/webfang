//! HTML extraction primitives — domain-pure building blocks.
//!
//! This module owns the pure extraction strategies (Readability, htmd
//! fallback, author extraction, DOM pre-pruning) that the scraper service
//! composes. It is **not** a port: there is one production composition and
//! no adapter seam, so the helpers are plain functions and the caller names
//! them directly.

use std::sync::LazyLock;

use scraper::{ElementRef, Selector};

/// Multi-strategy author extraction (domain pure, `scraper` + `serde_json`).
pub mod author_extractor;
pub mod date_extractor;

/// DOM pre-pruning pass to drop invisible/empty nodes before Readability.
pub mod dom_pruner;

// --- primitives shared by the author and date cascades -----------------------
//
// Both cascades read the same JSON-LD blocks and both need to turn an element
// into a trimmed string. The two copies had already drifted into a duplication
// gate failure, so the shapes live here once and the submodules import them.

/// The `script[type="application/ld+json"]` selector.
///
/// # Rules Applied
///
/// - **opt-inline**: compiled once via `LazyLock`
/// - **err-no-unwrap-prod**: `expect` documents a compile-time invariant
#[allow(clippy::expect_used)]
static SEL_JSONLD: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("script[type=\"application/ld+json\"]")
        // LCOV_EXCL_LINE defensive: compile-time-selector — constant selector string, parse cannot fail
        .expect("BUG: invalid CSS selector application/ld+json")
});

/// Trim and return the string, or `None` if it is empty/whitespace.
fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Collapse an element's text nodes into a single trimmed string.
fn text_content(element: &ElementRef<'_>) -> Option<String> {
    let collapsed: String = element.text().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    non_empty(&collapsed)
}

/// Fallback text extraction (domain pure, htmd).
///
/// The repo's single basic extractor: `htmd` with a line-strip degradation.
/// The former byte-identical copy in `infrastructure::scraper::fallback`
/// was collapsed onto this function in #2360; the conversion adapter's
/// failure path and the application extraction paths share it.
pub mod fallback {
    /// Extract text without Readability (basic HTML stripping).
    #[must_use]
    pub fn extract_text(html: &str) -> String {
        htmd::convert(html).unwrap_or_else(|_| {
            html.lines()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }
}

/// Readability wrapper (domain pure, legible).
pub mod readability {
    use crate::error::{Result, ScraperError};

    /// Parsed article from Readability.
    #[derive(Debug, Clone)]
    pub struct Article {
        /// Article title.
        pub title: String,
        /// Clean HTML content.
        pub content: String,
        /// Text content.
        pub text_content: String,
        /// Excerpt/summary if available.
        pub excerpt: Option<String>,
        /// Author/byline if available.
        pub byline: Option<String>,
        /// Publication time if available.
        pub published_time: Option<String>,
    }

    /// Parse HTML using Readability algorithm.
    pub fn parse(html: &str, url: Option<&str>) -> Result<Article> {
        let article = legible::parse(html, url, None)
            .map_err(|e| ScraperError::Extraction(format!("Readability failed: {e}")))?;
        Ok(Article {
            title: article.title,
            content: article.content,
            text_content: article.text_content,
            excerpt: article.excerpt,
            byline: article.byline,
            published_time: article.published_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::fallback::extract_text;

    #[test]
    fn extracts_text_from_valid_html() {
        let html = r#"<html><body><p>Hello World</p></body></html>"#;
        let result = extract_text(html);
        assert!(result.contains("Hello World"));
        assert!(!result.contains("<html>"));
    }

    #[test]
    fn extracts_text_with_scripts_removed() {
        let html = r#"
            <html>
                <head><script>var x = 1;</script></head>
                <body><article>Main content here</article></body>
            </html>
        "#;
        let result = extract_text(html);
        assert!(result.contains("Main content"));
        assert!(!result.contains("<script>"));
    }

    #[test]
    fn empty_html_yields_empty_text() {
        assert_eq!(extract_text(""), "");
    }

    #[test]
    fn malformed_html_still_extracts_what_it_can() {
        let html = "<div>Open div never closed<p>Paragraph";
        // Should not crash, should extract what it can
        assert!(!extract_text(html).is_empty());
    }
}
