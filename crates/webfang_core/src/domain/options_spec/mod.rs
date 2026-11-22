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

use serde_json::{json, Map, Value};

mod error;
mod types;

pub use error::{BoundError, BoxedBoundMessage, OptionSpecError, OwnedParseMessage};
pub use types::{DefaultValue, NumericPolicy, OptionSpec, ValueKind};

impl OptionSpec {
    /// Whether this option participates in the current build given its
    /// feature gate. A gate of `None` is always active; known gates map
    /// to this crate's cargo features so the spec mirrors the `cfg`
    /// duplication of the pre-slice-3 derives. Unknown gates fail closed
    /// (inactive) until they are wired here.
    #[must_use]
    pub const fn active(&self) -> bool {
        match self.feature_gate {
            None => true,
            Some(gate) => Self::gate_active(gate.as_bytes()),
        }
    }

    /// Byte-wise gate comparison: `&str` patterns are not allowed in
    /// `const fn` on Rust 1.88, so known gates are matched manually.
    /// Unknown gates fail closed (inactive) until wired here.
    const fn gate_active(gate: &[u8]) -> bool {
        if Self::bytes_eq(gate, b"ai") {
            // ADR-0004 paso 6: the `ai` Cargo feature is GONE, so this gate
            // has a FIXED value. The decision recorded here is `false` — the
            // value the default (feature-off) build already produced — so a
            // spec that still names `ai` stays INACTIVE instead of silently
            // activating a capability no configuration compiles any more.
            return false;
        }
        if Self::bytes_eq(gate, b"adaptive-selectors") {
            return cfg!(feature = "adaptive-selectors");
        }
        false
    }

    const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut i = 0;
        while i < a.len() {
            if a[i] != b[i] {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Validate and parse a raw textual value as an unsigned integer.
    ///
    /// Only meaningful for [`ValueKind::Uint`]; other kinds yield
    /// [`OptionSpecError::UnsupportedKind`]. Bounds come exclusively from the
    /// spec — this is THE single enforcement point for migrated paths.
    ///
    /// # Errors
    ///
    /// [`OptionSpecError::Parse`] on malformed input;
    /// [`OptionSpecError::Bound`] below the inclusive minimum or above the
    /// inclusive maximum;
    /// [`OptionSpecError::UnsupportedKind`] for non-integer kinds.
    pub fn parse_uint(&self, raw: &str) -> Result<u64, OptionSpecError> {
        match self.kind {
            ValueKind::Uint { policy } => {
                let value: u64 = raw
                    .parse()
                    .map_err(|_| self.parse_failure_error(raw, policy.as_ref()))?;
                Self::enforce_policy(value, policy.as_ref())?;
                Ok(value)
            },
            _ => Err(self.unsupported_kind()),
        }
    }

    /// Enforce the inclusive lower bound on a value parsed elsewhere
    /// ([`ValueKind::MemorySize`], whose suffix parsing belongs to
    /// infrastructure). Returns the input unchanged on success.
    ///
    /// # Errors
    ///
    /// [`BoundError::MinViolated`] when below the inclusive minimum or
    /// [`BoundError::MaxViolated`] above the inclusive maximum;
    /// [`OptionSpecError::UnsupportedKind`] for kinds without bounds.
    /// The error is the structured form (issue #948 F7) so consumers can
    /// format the wire-correct wording without re-comparing the value.
    pub fn check_bound(&self, value: u64) -> Result<u64, BoundError> {
        match self.kind {
            ValueKind::Uint { policy } | ValueKind::MemorySize { policy } => {
                Self::enforce_policy_structured(value, policy.as_ref())?;
                Ok(value)
            },
            _ => Err(self.unsupported_kind_bound()),
        }
    }

    /// Shared bound enforcement behind [`Self::parse_uint`] and
    /// [`Self::check_bound`]: THE single place where spec bounds reject a
    /// value, using each policy's exact stored user-facing message.
    fn enforce_policy(value: u64, policy: Option<&NumericPolicy>) -> Result<(), OptionSpecError> {
        let Some(policy) = policy else {
            // `None` = the spec records no bound: nothing to enforce.
            return Ok(());
        };
        if value < policy.min {
            return Err(OptionSpecError::Bound(BoxedBoundMessage {
                message: policy.below_min_message,
            }));
        }
        if let Some(max) = policy.max {
            if value > max {
                return Err(OptionSpecError::Bound(BoxedBoundMessage {
                    message: policy.above_max_message,
                }));
            }
        }
        Ok(())
    }

    /// Structured bound enforcement behind [`Self::check_bound`] (issue
    /// #948 F7). Same policy semantics as [`Self::enforce_policy`] but
    /// returns the typed [`BoundError`] variants so consumers can format
    /// the wire-correct wording without re-comparing the value or
    /// re-reading the policy. Used by every surface whose user-facing
    /// messages name the parameter, not the CLI flag.
    fn enforce_policy_structured(
        value: u64,
        policy: Option<&NumericPolicy>,
    ) -> Result<(), BoundError> {
        let Some(policy) = policy else {
            return Ok(());
        };
        if value < policy.min {
            return Err(BoundError::MinViolated { min: policy.min });
        }
        if let Some(max) = policy.max {
            if value > max {
                return Err(BoundError::MaxViolated { max });
            }
        }
        Ok(())
    }

    /// Build the parse-failure error for this option's kind with the exact
    /// stored message. Used by bridges whose parsing engine is external
    /// (e.g. memory-size suffix parsing).
    #[must_use]
    pub fn parse_error(&self, raw: &str) -> OptionSpecError {
        match self.kind {
            ValueKind::Uint { policy } | ValueKind::MemorySize { policy } => {
                self.parse_failure_error(raw, policy.as_ref())
            },
            _ => OptionSpecError::Parse {
                raw: raw.to_string(),
                detail: "un valor válido",
            },
        }
    }

    /// Render the parse-failure error per the policy: verbatim template with
    /// `{value}` substitution when present, default `` `{raw}` no es … ``
    /// shape otherwise. Unbounded entries (`None`) render the canonical
    /// integer message.
    fn parse_failure_error(&self, raw: &str, policy: Option<&NumericPolicy>) -> OptionSpecError {
        let (template, detail) = match policy {
            Some(policy) => (policy.parse_failure_template, policy.parse_failure_detail),
            None => (None, NumericPolicy::INTEGER_PARSE_DETAIL),
        };
        match template {
            Some(template) => OptionSpecError::ParseVerbatim(OwnedParseMessage {
                message: template.replace("{value}", raw),
            }),
            None => OptionSpecError::Parse {
                raw: raw.to_string(),
                detail,
            },
        }
    }

    fn unsupported_kind(&self) -> OptionSpecError {
        OptionSpecError::UnsupportedKind { id: self.id }
    }

    /// Bound-check on a non-numeric kind never reaches here in practice,
    /// but the structured API mirrors the existing `unsupported_kind` so
    /// the error type stays coherent (issue #948 F7).
    fn unsupported_kind_bound(&self) -> BoundError {
        // Unreachable for the current `ValueKind` set — the structured
        // path is only entered from numeric kinds. Re-routed through the
        // `MinViolated` arm with a sentinel min of 0 so the validator
        // surfaces a usable message rather than panicking on an
        // impossible branch. Callers that hit this path have a logic
        // bug; the assertion is intentionally not panic-typed because
        // the MCP validator surfaces errors as JSON-RPC, not crashes.
        BoundError::MinViolated { min: 0 }
    }

    /// Accepted variants when this option is [`ValueKind::Enum`] (in
    /// declaration order); `None` for every other kind. Lets downstream
    /// surfaces (MCP validation, schema bridge) derive closed value sets
    /// from the SSOT instead of duplicating string literals.
    #[must_use]
    pub const fn enum_variants(&self) -> Option<&'static [&'static str]> {
        match self.kind {
            ValueKind::Enum { variants } => Some(variants),
            _ => None,
        }
    }

    /// JSON Schema fragment describing this option (ADR-002 seam #2).
    ///
    /// Consumed by the MCP schema bridge (slice 4) and pinned by tests in
    /// slice 1.
    #[must_use]
    pub fn json_schema(&self) -> Value {
        let mut schema = Map::new();
        // F6: description override wins over `help` for the MCP wire
        // surface; the CLI/help path keeps `help` byte-exact against
        // clap. Default is `help` (no override) so existing entries
        // stay drift-free.
        let description = self.description_override.unwrap_or(self.help);
        schema.insert("description".into(), json!(description));
        match self.kind {
            ValueKind::Text | ValueKind::Path => {
                schema.insert("type".into(), json!("string"));
            },
            ValueKind::TextList => {
                schema.insert("type".into(), json!("array"));
                let mut items = Map::new();
                items.insert("type".into(), json!("string"));
                schema.insert("items".into(), Value::Object(items));
            },
            ValueKind::Bool => {
                schema.insert("type".into(), json!("boolean"));
            },
            ValueKind::Enum { variants } => {
                schema.insert("type".into(), json!("string"));
                schema.insert("enum".into(), json!(variants));
            },
            ValueKind::Uint { policy } => {
                schema.insert("type".into(), json!("integer"));
                if let Some(policy) = policy {
                    schema.insert("minimum".into(), json!(policy.min));
                    if let Some(max) = policy.max {
                        schema.insert("maximum".into(), json!(max));
                    }
                }
            },
            ValueKind::MemorySize { policy } => {
                // CLI/env input is textual (suffixes allowed); the byte floor
                // travels alongside until the MCP slice picks a final shape.
                schema.insert("type".into(), json!("string"));
                schema.insert("format".into(), json!("byte-size"));
                if let Some(policy) = policy {
                    schema.insert("minimumBytes".into(), json!(policy.min));
                }
            },
        }
        // F5: explicit `nullable: true` upgrades the type to a union with
        // `null`. The bridge also auto-promotes optional fields by
        // preserving the derived `["<inner>", "null"]` shape, so most
        // entries leave this as `false`.
        if self.nullable {
            if let Some(Value::String(inner)) = schema.get("type").cloned() {
                schema.insert(
                    "type".into(),
                    Value::Array(vec![Value::String(inner), Value::Null]),
                );
            }
        }
        // F4 strict replacement: `default` is the single typed source of
        // truth. `json!` of a `&'static str` would emit a string, so the
        // `DefaultValue::to_json_value` path serializes native wire types
        // (`Uint(2)` → `2`, `Bool(true)` → `true`, `Str("jsonl")` →
        // `"jsonl"`).
        if let Some(default) = self.default {
            schema.insert("default".into(), default.to_json_value());
        }
        if let Some(gate) = self.feature_gate {
            schema.insert("x-feature-gate".into(), json!(gate));
        }
        Value::Object(schema)
    }
}

/// Aggregate per-option JSON Schemas into a `{id: schema}` object.
///
/// Key layout follows serde_json's map ordering (alphabetical), not slice
/// order — consumers must key by id.
#[must_use]
pub fn schema_object(options: &[OptionSpec]) -> Value {
    Value::Object(
        options
            .iter()
            .map(|opt| (opt.id.to_owned(), opt.json_schema()))
            .collect::<Map<String, Value>>(),
    )
}

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
