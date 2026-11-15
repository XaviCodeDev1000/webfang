//! Domain error types
//!
//! Error types for the domain layer, organized by concern.

pub mod cause;
mod crawl_error;
mod crawl_error_category;
mod domain_error;
mod error_class;
pub(crate) mod mapping;
pub(crate) mod resolution;

pub use crawl_error::{CrawlError, ResourceKind, WafDetectionKind};
pub use crawl_error_category::CrawlErrorCategory;
pub use domain_error::DomainError;
pub use error_class::ErrorClass;
