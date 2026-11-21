//! Content extraction — hub module.
//!
//! Hosts the pure CSS-selector extraction logic (`selector`) and the
//! Readability entry points (`readability`), plus re-exports preserving the
//! historical `extraction::*` public paths.

pub(crate) mod readability;
pub(crate) mod selector;

#[cfg(feature = "adaptive-selectors")]
pub(crate) use readability::adaptive_selector_repair;
pub(crate) use readability::prune_dom_if_enabled;
pub use readability::{extract_content, scrape_with_readability};
pub use selector::extract_with_selector;
pub(crate) use selector::selector_supplied_fragment;

#[cfg(test)]
#[path = "selector_serialization_tests.rs"]
mod selector_serialization;
#[cfg(test)]
#[path = "extraction_tests.rs"]
mod tests;
