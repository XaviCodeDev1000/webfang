//! HTTP client settings of the crawler group: the retry budget, the request
//! identity headers, and the cookie jar scope.
//!
//! Split out of `crawler.rs` by heading, mechanically: every spec below is
//! byte-for-byte the entry that lived there, in the same relative order. The
//! group re-exports them, so `crawler::ACCEPT_LANGUAGE` and the rest keep
//! resolving to the same path.
use crate::domain::options_spec::{DefaultValue, OptionSpec, ValueKind};

/// `--max-retries <MAX_RETRIES>` — metadata-only (no bound today).
pub const MAX_RETRIES: OptionSpec = OptionSpec {
    id: "max_retries",
    value_name: "MAX_RETRIES",
    long: "max-retries",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_MAX_RETRIES"),
    default: Some(DefaultValue::Uint(3)),
    nullable: false,
    description_override: None,
    help: "Maximum number of retry attempts",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
    value_delimiter: None,
};

/// `--backoff-base-ms <BACKOFF_BASE_MS>` — metadata-only (no bound
/// today).
pub const BACKOFF_BASE_MS: OptionSpec = OptionSpec {
    id: "backoff_base_ms",
    value_name: "BACKOFF_BASE_MS",
    long: "backoff-base-ms",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_BACKOFF_BASE_MS"),
    default: Some(DefaultValue::Uint(1000)),
    nullable: false,
    description_override: None,
    help: "Base delay for exponential backoff (ms)",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
    value_delimiter: None,
};

/// `--backoff-max-ms <BACKOFF_MAX_MS>` — metadata-only (no bound today).
pub const BACKOFF_MAX_MS: OptionSpec = OptionSpec {
    id: "backoff_max_ms",
    value_name: "BACKOFF_MAX_MS",
    long: "backoff-max-ms",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_BACKOFF_MAX_MS"),
    default: Some(DefaultValue::Uint(10000)),
    nullable: false,
    description_override: None,
    help: "Maximum delay for exponential backoff (ms)",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
    value_delimiter: None,
};

/// `--accept-language <ACCEPT_LANGUAGE>`
pub const ACCEPT_LANGUAGE: OptionSpec = OptionSpec {
    id: "accept_language",
    value_name: "ACCEPT_LANGUAGE",
    long: "accept-language",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_ACCEPT_LANGUAGE"),
    default: Some(DefaultValue::Str("en-US,en;q=0.9")),
    help: "Accept-Language header value",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--user-agent <USER_AGENT>`
pub const USER_AGENT: OptionSpec = OptionSpec {
    id: "user_agent",
    value_name: "USER_AGENT",
    long: "user-agent",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_USER_AGENT"),
    default: None,
    help: "Custom User-Agent header value (overrides Chrome 145 default)",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};
/// `--cookie-jar-scope <COOKIE_JAR_SCOPE>`
pub const COOKIE_JAR_SCOPE: OptionSpec = OptionSpec {
    id: "cookie_jar_scope",
    value_name: "COOKIE_JAR_SCOPE",
    long: "cookie-jar-scope",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_COOKIE_JAR_SCOPE"),
    default: Some(DefaultValue::Str("shared")),
    help: "Cookie jar scope: shared (one jar for the process, current behavior) or operation (one fresh jar per operation)",
    heading: Some("Cookies"),
    kind: ValueKind::Enum {
        variants: &["shared", "operation"],
    },
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};
