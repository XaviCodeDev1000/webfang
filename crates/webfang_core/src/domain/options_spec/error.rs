//! Error taxonomy for spec-driven option validators (ADR-002).
//!
//! Every error a spec-driven validator can raise, with the exact
//! user-facing wording attached at the definition site. Re-exported from
//! [`crate::domain::options_spec`] so consumer paths stay unchanged.

/// Error raised by spec-driven validators.
///
/// Messages are user-facing (Spanish) and must render byte-identically to
/// the pre-migration clap parsers — pinned by the parity tests in
/// `cli::args::export`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OptionSpecError {
    /// Raw input cannot be parsed into the option's value kind.
    #[error("`{raw}` no es {detail}")]
    Parse {
        /// The offending raw input.
        raw: String,
        /// Completes the Spanish message ("un número entero válido", …).
        detail: &'static str,
    },
    /// Raw input cannot be parsed; the option's policy supplied a verbatim
    /// template whose fully rendered message (with `{value}` substituted) is
    /// carried here. Used when legacy wording cannot fit the [`Self::Parse`]
    /// shape.
    #[error("{0}")]
    ParseVerbatim(#[source] OwnedParseMessage),
    /// Parsed value violates the option's inclusive lower bound.
    #[error("{0}")]
    Bound(#[source] BoxedBoundMessage),
    /// A validator was called on an option whose kind does not support it.
    #[error("option `{id}` does not support this validator")]
    UnsupportedKind {
        /// Identifier of the misused option.
        id: &'static str,
    },
}

/// Owned wrapper so `OptionSpecError` stays `Clone + PartialEq` while the
/// message remains a cheap `&'static str` inside.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BoxedBoundMessage {
    /// Exact user-facing bound-violation message.
    pub message: &'static str,
}

/// Owned message produced by substituting `{value}` into a policy's verbatim
/// parse-failure template
/// ([`crate::domain::options_spec::NumericPolicy::parse_failure_template`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct OwnedParseMessage {
    /// Fully rendered user-facing parse-failure message.
    pub message: String,
}

/// Structured bound violation emitted by
/// [`crate::domain::options_spec::OptionSpec::check_bound`] (issue #948 F7).
///
/// Carries the offending bound as data so consumers (e.g. the MCP
/// validators in `webfang_mcp::mcp_server::params`) can format the
/// MCP-stable English wording ("must be at least N" / "must be at most N")
/// without re-comparing the value or re-reading the spec's
/// [`crate::domain::options_spec::NumericPolicy`]. The `Display` impl IS the
/// MCP-stable wording; the Spanish verbatim message stays in
/// [`crate::domain::options_spec::NumericPolicy`] and is reached via
/// [`OptionSpecError::Bound`] for CLI consumers that need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BoundError {
    /// Value is below the inclusive lower bound.
    #[error("must be at least {min}")]
    MinViolated {
        /// Inclusive lower bound.
        min: u64,
    },
    /// Value is above the inclusive upper bound.
    #[error("must be at most {max}")]
    MaxViolated {
        /// Inclusive upper bound.
        max: u64,
    },
}
