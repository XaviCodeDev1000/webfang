//! Pipeline processing stages.
//!
//! Each stage implements [`crate::domain::pipeline_item::PipelineStage`] and performs a single,
//! well-defined transformation or validation on [`crate::domain::pipeline_item::ScrapedItem`]s.

mod clean;
mod validate;

pub use clean::CleanStage;
pub use validate::ValidateStage;
