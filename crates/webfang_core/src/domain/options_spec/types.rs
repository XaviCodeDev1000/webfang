//! Declarative data model of the options spec (ADR-002).
//!
//! Pure data: the typed default, the per-option spec record, the numeric
//! validation policy and the value-kind domain. Every user-facing option is
//! described exactly once here; the derivation engine lives in
//! [`crate::domain::options_spec::validation`]. Re-exported from
//! [`crate::domain::options_spec`] so consumer paths stay unchanged.

use serde_json::{json, Value};
/// Typed default value used by the JSON schema path (issue #948 F4).
///
/// Single source of truth for the option's default: `json_schema()`
/// serializes the wire type natively — `"default": 2` for an `integer` kind,
/// `"default": true` for a `boolean` kind, `"default": "jsonl"` for a
/// `string`/enum kind. The CLI parity surface reaches the canonical string
/// form through the [`Display`](core::fmt::Display) impl (the clap `default_value` path
/// uses `.to_string()` to obtain the same string). Strict replacement of
/// the previous `default: Option<&'static str>` plus the additive
/// `schema_default` field — only the typed form remains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultValue {
    /// Free-form string default — every `Text`/`Path`/`Enum` default and any
    /// `MemorySize` byte-size default. The wrapped string is the CLI's
    /// `default_value` rendering and the schema's `"default"` value for
    /// textual kinds.
    Str(&'static str),
    /// Unsigned integer default — schema emits a JSON number, never a string.
    Uint(u64),
    /// Boolean default — schema emits a JSON boolean, never a string.
    Bool(bool),
}

impl DefaultValue {
    /// Native JSON value matching the option's wire kind — issue #948 F4.
    /// `Uint(2)` becomes `Value::Number(2)` (not `"2"`), `Bool(true)` becomes
    /// `Value::Bool(true)` (not `"true"`), `Str("jsonl")` becomes
    /// `Value::String("jsonl")`. This is the form [`OptionSpec::json_schema`]
    /// serializes into the advertised MCP schema and is the drift-killer
    /// for the type-inconsistency finding.
    #[must_use]
    pub fn to_json_value(&self) -> Value {
        match self {
            Self::Str(s) => Value::String((*s).to_owned()),
            Self::Uint(n) => json!(n),
            Self::Bool(b) => Value::Bool(*b),
        }
    }
}

impl core::fmt::Display for DefaultValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Str(s) => f.write_str(s),
            Self::Uint(n) => write!(f, "{n}"),
            Self::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// Declarative description of ONE user-facing option (ADR-002).
///
/// Pure data: no clap types, no infrastructure types — this is domain-layer
/// SSOT that both the CLI bridge and (in later slices) the MCP schema
/// generator consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSpec {
    /// Stable identifier — clap arg id today, MCP property name tomorrow.
    pub id: &'static str,
    /// Long flag name without leading dashes (kebab-case).
    pub long: &'static str,
    /// Single-dash flag, if any.
    pub short: Option<char>,
    /// Alternative long names accepted by clap but NOT rendered in help
    /// output (clap's `alias`). Visible aliases live in
    /// [`OptionSpec::visible_aliases`].
    pub aliases: &'static [&'static str],
    /// Alternative long names accepted by clap AND rendered in help
    /// output (clap's `visible_alias`).
    pub visible_aliases: &'static [&'static str],
    /// Placeholder shown for the option's value (`<VALUE_NAME>`); today
    /// always the SCREAMING_SNAKE id (clap derive's rendering). A static
    /// string because clap's owned-`Str` support sits behind its `string`
    /// feature, which this crate does not enable.
    pub value_name: &'static str,
    /// Environment variable consulted when the flag is absent.
    pub env: Option<&'static str>,
    /// Canonical default value (issue #948 F4 strict replacement).
    ///
    /// The typed enum is the single source of truth: the CLI parity path
    /// uses the [`Display`](core::fmt::Display) impl to obtain the canonical string form
    /// for clap's `default_value`, and the JSON schema path uses
    /// [`DefaultValue::to_json_value`] to serialize the native wire type
    /// (`Uint(2)` → `2`, `Bool(true)` → `true`, `Str("jsonl")` →
    /// `"jsonl"`). The previous `default: Option<&'static str>` plus
    /// additive `schema_default` pair is gone — only the typed form
    /// remains, and the two surfaces derive from the same value.
    pub default: Option<DefaultValue>,
    /// Whether the advertised JSON schema must declare the property
    /// nullable — i.e. emit `"type": ["<inner>", "null"]` (issue #948 F5).
    /// The bridge preserves the schemars-derived `["<inner>", "null"]`
    /// shape for `Option<T>` fields by default; this flag is an explicit
    /// opt-in for entries that need the nullable shape even when the
    /// spec entry's kind is non-optional, or to force a non-nullable
    /// shape on a derived optional field. Most entries leave this as
    /// `false` and let the bridge infer from the derived schema.
    pub nullable: bool,
    /// Tool-appropriate description override for the advertised MCP
    /// schema (issue #948 F6). When `Some`, `json_schema()` uses this
    /// string as the `"description"` field instead of [`Self::help`].
    /// CLI/help output keeps [`Self::help`] (which is byte-exact
    /// against clap's rendering); the MCP bridge only sees the
    /// override, decoupling the CLI help wording from the LLM-facing
    /// description. `None` = no override, fall back to [`Self::help`].
    pub description_override: Option<&'static str>,
    /// Help text (`--help` long form); must match the clap derive's rendering.
    pub help: &'static str,
    /// Help heading/group under which clap lists this option.
    pub heading: Option<&'static str>,
    /// Value domain plus validation policy (bounds live HERE only).
    pub kind: ValueKind,
    /// Cargo feature required for this option to exist (`None` = always).
    /// No export-group option is gated; the seam serves later slices.
    pub feature_gate: Option<&'static str>,
    /// Optional character that splits a single CLI value into a `Vec` at
    /// parse time. Present only on flag groups whose surface includes a
    /// delimiter (slice 5a `obsidian_tags`); every other entry keeps
    /// `None` so the clap builder stays a pure hand-off from this field.
    pub value_delimiter: Option<char>,
}

/// Validation policy for numeric options: the inclusive bounds and the
/// exact user-facing messages raised when they are violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumericPolicy {
    /// Inclusive lower bound (`1` ⇒ zero is rejected; `0` keeps zero valid,
    /// e.g. seed-only crawl semantics).
    pub min: u64,
    /// Inclusive upper bound (`None` = unbounded above). Caps shared by
    /// every surface live HERE only (ADR-002 slice 4).
    pub max: Option<u64>,
    /// Completes "`{raw}` no es …" when parsing fails.
    pub parse_failure_detail: &'static str,
    /// Complete message raised when the value is below [`NumericPolicy::min`].
    pub below_min_message: &'static str,
    /// Complete message raised when the value is above
    /// [`NumericPolicy::max`]. Unreachable (and empty) when `max` is
    /// `None` or when `min = 0` already accepts every valid value below it.
    pub above_max_message: &'static str,
    /// Verbatim parse-failure message overriding the default
    /// `` `{raw}` no es {parse_failure_detail} `` rendering, with `{value}`
    /// substituted by the raw input. Present ONLY where pre-migration legacy
    /// wording cannot be expressed by the default template — byte-identical
    /// errors outrank uniformity (ADR-002 acceptance bar). When set, the
    /// default template and `parse_failure_detail` are unused for parsing.
    pub parse_failure_template: Option<&'static str>,
}

impl NumericPolicy {
    /// Canonical detail completing "`{raw}` no es …" for integer options
    /// whose kind carries no explicit policy (unbounded metadata entries).
    pub(super) const INTEGER_PARSE_DETAIL: &'static str = "un número entero válido";

    /// Canonical positive-integer policy (`min = 1`) with the standard
    /// integer parse-failure message and no verbatim template.
    #[must_use]
    pub const fn positive(below_min_message: &'static str) -> Self {
        Self {
            min: 1,
            max: None,
            parse_failure_detail: Self::INTEGER_PARSE_DETAIL,
            below_min_message,
            above_max_message: "",
            parse_failure_template: None,
        }
    }

    /// Policy that keeps zero valid (e.g. seed-only crawl depth semantics)
    /// and enforces an inclusive upper bound with the canonical integer
    /// parse-failure message. The below-min branch is unreachable because
    /// `min = 0`.
    #[must_use]
    pub const fn zero_valid_capped(max: u64, above_max_message: &'static str) -> Self {
        Self {
            min: 0,
            max: Some(max),
            parse_failure_detail: Self::INTEGER_PARSE_DETAIL,
            below_min_message: "",
            above_max_message,
            parse_failure_template: None,
        }
    }

    /// Attach an inclusive upper bound to an existing policy without
    /// touching its user-facing messages. Used when a fully migrated flag
    /// gains a shared cap while keeping its legacy wording (#940).
    #[must_use]
    pub const fn capped(mut self, max: u64, above_max_message: &'static str) -> Self {
        self.max = Some(max);
        self.above_max_message = above_max_message;
        self
    }

    /// Policy for fully migrated flags that keep pre-migration legacy
    /// wording: canonical integer detail, an enforced inclusive minimum, and
    /// a verbatim parse-failure template with `{value}` substitution
    /// (#780 byte-exactness outranks uniformity).
    #[must_use]
    pub const fn legacy_verbatim(
        min: u64,
        below_min_message: &'static str,
        parse_failure_template: &'static str,
    ) -> Self {
        Self {
            min,
            max: None,
            parse_failure_detail: Self::INTEGER_PARSE_DETAIL,
            below_min_message,
            above_max_message: "",
            parse_failure_template: Some(parse_failure_template),
        }
    }
}

/// Value domain of an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// Free-form string.
    Text,
    /// Comma-delimited list of strings (`Vec<String>`); clap splits on
    /// [`OptionSpec::value_delimiter`]. Absent today outside slice 5a's
    /// `obsidian_tags`, but the kind ships now so any future
    /// comma-delimited flag (or a delimiter other than `,`) reuses the
    /// same code path without a spec extension.
    TextList,
    /// Filesystem path.
    Path,
    /// Boolean switch parsed from `"true"`/`"false"`.
    Bool,
    /// Closed set of accepted canonical (kebab-case) values.
    Enum {
        /// Accepted values in declaration order.
        variants: &'static [&'static str],
    },
    /// Unsigned integer. `Some(policy)` enforces the policy's inclusive
    /// lower bound with its exact messages; `None` records a metadata-only
    /// entry with NO bound today — parse failures render the canonical
    /// `` `{raw}` no es un número entero válido `` message.
    Uint {
        /// Shared parse + bound policy, absent when unbounded.
        policy: Option<NumericPolicy>,
    },
    /// Memory size with binary-suffix support (`8GB`, `2048MB`, plain bytes).
    /// Suffix parsing lives in `infrastructure::autotuning`; the byte floor,
    /// when one exists, lives here via [`NumericPolicy`].
    MemorySize {
        /// Shared bound policy for externally-parsed byte counts, absent
        /// when unbounded.
        policy: Option<NumericPolicy>,
    },
}

impl ValueKind {
    /// Metadata-only unsigned integer: parsing stays with the external
    /// engine (clap's built-in integer parser) and the spec invents no
    /// bound.
    #[must_use]
    pub const fn uint_unbounded() -> Self {
        Self::Uint { policy: None }
    }

    /// Unsigned integer whose inclusive lower bound is enforced through the
    /// spec ([`OptionSpec::parse_uint`] / [`OptionSpec::check_bound`]).
    #[must_use]
    pub const fn uint(policy: NumericPolicy) -> Self {
        Self::Uint {
            policy: Some(policy),
        }
    }
}
