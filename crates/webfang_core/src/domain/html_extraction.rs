//! HTML extraction primitives — domain-pure building blocks.
//!
//! This module owns the pure extraction strategies (Readability, htmd
//! fallback, author extraction, DOM pre-pruning) that the scraper service
//! composes. It is **not** a port: there is one production composition and
//! no adapter seam, so the helpers are plain functions and the caller names
//! them directly.

/// Multi-strategy author extraction (domain pure, `scraper` + `serde_json`).
pub mod author_extractor;

/// DOM pre-pruning pass to drop invisible/empty nodes before Readability.
pub mod dom_pruner;

/// Fallback text extraction (domain pure, htmd).
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
