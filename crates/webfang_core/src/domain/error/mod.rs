//! Domain error types
//!
//! Error types for the domain layer, organized by concern.

mod cause;
mod crawl_error;
mod crawl_error_category;
mod domain_error;
mod error_class;

pub use cause::{cause_in_chain, ClassifiedNetworkError, DnsResolutionError, NetworkCause};
pub use crawl_error::{CrawlError, ResourceKind, WafDetectionKind};
pub use crawl_error_category::CrawlErrorCategory;
pub use domain_error::DomainError;
pub use error_class::ErrorClass;
