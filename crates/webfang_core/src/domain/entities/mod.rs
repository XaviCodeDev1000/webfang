//! Domain entities — Core business types
//!
//! These are the fundamental data structures used throughout the application.
//! They are serializable for persistence but contain no business logic.

pub mod content;
pub mod download;
pub mod export;
pub mod progress;
pub mod scraped_content;

pub use content::{
    DocumentChunk, DocumentChunkExported, DocumentChunkUnvalidated, DocumentChunkValidated, Draft,
    Exported, Validated, ValidationError,
};
pub use download::DownloadedAsset;
pub use export::{ExportFormat, ExportState, StateVersion};
pub use progress::{
    ErrorEntry, ErrorType, ProgressState, ScrapeError, ScrapeProgress, ScrapeStatus, UrlState,
};
pub use scraped_content::ScrapedContent;
