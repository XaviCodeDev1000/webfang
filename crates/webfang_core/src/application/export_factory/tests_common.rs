//! Shared test base for the `export_factory` test modules.
//!
//! The helpers here are consumed by two or more child modules; items
//! used by a single child stay in that child, private.

use super::*;
use crate::domain::entities::ScrapedContent;
use crate::domain::ValidUrl;
use tempfile::TempDir;

#[cfg(test)]
pub(crate) use crate::infrastructure::export::RecordStore;

pub(super) fn make_scraped_content(url: &str, title: &str, content: &str) -> ScrapedContent {
    ScrapedContent {
        title: title.to_string(),
        content: content.to_string(),
        url: ValidUrl::parse(url).unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    }
}

/// Export through the public port (fresh run, no resume) and return the
/// URLs it drove: the arrange shared by the outcome-shape tests.
pub(super) fn export_urls(
    results: &[ScrapedContent],
    dir: &TempDir,
    format: ExportFormat,
) -> Vec<String> {
    process_results(results, dir.path().to_path_buf(), format, "export", None)
        .expect("the export succeeds")
        .processed_urls
}
