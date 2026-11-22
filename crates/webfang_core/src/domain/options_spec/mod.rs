//! OptionsSpec — single source of truth for user-facing options (ADR-002).
//!
//! Every user-facing option is described exactly once as declarative data:
//! identifiers, value kind, default, environment variable, numeric bounds,
//! help text, help heading, feature gate, and clap surface (long/short/
//! aliases). From this description the project derives, slice by slice:
//!
//! 1. clap argument definitions,
//! 2. JSON Schema for MCP tool input validation,
//! 3. shared validators with identical bounds.
//!
//! Slice 1 migrated the export flag group (`export`, mirroring
//! `cli::args::ExportArgs`); slice 2 migrates the crawler flag group
//! (`crawler`, mirroring `cli::args::CrawlerArgs`). Remaining option groups
//! keep their hand-written definitions until later slices. The clap derive
//! stays in place as the parsing engine — byte-identical help/error output is
//! the acceptance bar — while its value parsers and the parity tests route
//! through the spec so bounds and messages have exactly one home.

mod error;
mod types;
mod validation;

pub use error::{BoundError, BoxedBoundMessage, OptionSpecError, OwnedParseMessage};
pub use types::{DefaultValue, NumericPolicy, OptionSpec, ValueKind};
pub use validation::schema_object;

/// Export flag group (ADR-002 slice 1): mirrors `cli::args::ExportArgs`.
pub mod export;

/// Crawler flag group (ADR-002 slice 2): mirrors `cli::args::CrawlerArgs`.
pub mod crawler;

/// AI flag group (ADR-002 slice 5a): mirrors `cli::args::AiArgs`.
pub mod ai;

/// Obsidian flag group (ADR-002 slice 5a): mirrors `cli::args::ObsidianArgs`.
pub mod obsidian;

/// LLM provider flag group: mirrors `cli::args::LlmArgs`. Not gated by the
/// `ai` cargo feature — the remote provider port is always compiled.
pub mod llm;
#[cfg(test)]
#[path = "options_spec_tests.rs"]
mod tests;
