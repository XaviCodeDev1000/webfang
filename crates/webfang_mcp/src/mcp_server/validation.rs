//! MCP parameter validation helpers (issue #512, Slice 1)
//!
//! Centralised validation for tool parameter structs. Each helper returns
//! `Result<_, McpError>` using `McpError::invalid_params` so handlers can
//! short-circuit with `?`. The module is framework-agnostic: it knows nothing
//! about the tool router or the request context.
//!
//! Every helper produces an error envelope identical to the one already used
//! by handlers (`McpError::invalid_params(format!(...), Some(field))`), so the
//! slice-2 wiring (`params.validate()?`) is a drop-in replacement.
//!
//! Since EC-08 (issue #1613) that envelope's `data` is a structured OBJECT
//! (`{"field": …, "reason": <slug>}`) rather than a bare string, so a caller
//! can tell WHICH field was wrong and WHY without parsing prose. The seven
//! stable slugs are `validation::REASON_*` (documented on the
//! `invalid_params_with_reason` builder, which is crate-private).

use rmcp::ErrorData as McpError;

mod path;
mod reason;

pub use path::{
    require_safe_filename, require_safe_path, require_safe_path_allow_absolute, SanitizedFilename,
};
pub(crate) use reason::{invalid_params_with_reason, reason_of};
pub use reason::{
    REASON_EMPTY, REASON_MALFORMED, REASON_NOT_IN_ALLOWED_SET, REASON_OUT_OF_RANGE,
    REASON_PATH_NOT_ALLOWED, REASON_TOO_LONG, REASON_UNSUPPORTED_SCHEME,
};

/// Max URL length. 8 KiB matches the upper bound recommended by RFC 9110 §5.4
/// for URI references and protects the server from memory DoS via oversize
/// inputs.
pub const MAX_URL_LEN: usize = 8192;

/// Max filesystem path length. 1 KiB is generous for relative paths and
/// protects against accidentally-joined traversal strings.
pub const MAX_PATH_LEN: usize = 1024;

/// Max HTML / markdown / content blob length. 1 MiB protects the server from
/// memory exhaustion via oversize inputs (legitimate pages fit comfortably).
pub const MAX_BLOB_LEN: usize = 1_048_576;

/// Max length of a domain string (RFC 1035 §2.3.4 caps FQDNs at 253 octets).
pub const MAX_DOMAIN_LEN: usize = 253;

/// Validate that `value` parses as an http or https URL.
///
/// Rejects: empty, longer than [`MAX_URL_LEN`], non-http(s) schemes (file://,
/// ftp://, gopher://, data:, javascript:, etc.), and unparseable strings.
/// Returns the parsed URL on success so callers can reuse it without a
/// second parse.
///
/// # Errors
/// Returns `McpError::invalid_params` for any of the rejection reasons above,
/// carrying [`REASON_EMPTY`], [`REASON_TOO_LONG`], [`REASON_MALFORMED`], or
/// [`REASON_UNSUPPORTED_SCHEME`] respectively.
pub fn require_http_url(field: &str, value: &str) -> Result<url::Url, McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_URL_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_URL_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    let parsed = url::Url::parse(value).map_err(|e| {
        invalid_params_with_reason(field, format!("invalid URL: {e}"), REASON_MALFORMED)
    })?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(invalid_params_with_reason(
            field,
            format!("unsupported scheme '{other}' (only http and https are allowed)"),
            REASON_UNSUPPORTED_SCHEME,
        )),
    }
}

/// Validate that `value` is at most `max_len` characters long.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_TOO_LONG`] if
/// `value.len() > max_len`.
pub fn require_max_len(field: &str, value: &str, max_len: usize) -> Result<(), McpError> {
    if value.len() > max_len {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {max_len} bytes"),
            REASON_TOO_LONG,
        ));
    }
    Ok(())
}

/// Validate that `value` is non-empty and ≤ [`MAX_PATH_LEN`] characters.
///
/// Use for fields like `filename` and `vault_name` that are joined into paths
/// but are not paths themselves (no traversal check needed).
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`] or
/// [`REASON_TOO_LONG`] if `value` is empty or exceeds [`MAX_PATH_LEN`].
pub fn require_safe_name(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    Ok(())
}

/// Validate that `value` is a well-formed bare domain string (e.g. "example.com"
/// or "a.b.c.d.e.example.com"): non-empty, ≤ [`MAX_DOMAIN_LEN`], no path or
/// scheme separators (`/`, `\`, `:`), no whitespace, no `..` segments, and at
/// least one `.`.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`],
/// [`REASON_TOO_LONG`], [`REASON_MALFORMED`] (whitespace, stray separator,
/// no `.` separator) or [`REASON_PATH_NOT_ALLOWED`] (a `..` segment) if
/// `value` fails any of the bare-domain rules.
pub fn require_safe_domain(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_DOMAIN_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_DOMAIN_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain whitespace",
            REASON_MALFORMED,
        ));
    }
    if value.contains(['/', '\\', ':']) {
        return Err(invalid_params_with_reason(
            field,
            "must be a bare domain (no path, scheme, or port separator)",
            REASON_MALFORMED,
        ));
    }
    if value.contains("..") {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..'",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if !value.contains('.') {
        return Err(invalid_params_with_reason(
            field,
            "must contain at least one '.' separator",
            REASON_MALFORMED,
        ));
    }
    Ok(())
}

/// Validate a seed host: a bare domain (e.g. "example.com") OR an http(s) URL
/// (e.g. `<https://example.com/path>`). Mirrors the core's
/// `url_validation::normalize_seed_host` acceptance so MCP validation does not
/// over-reject input the core legitimately handles. Rejects empty, whitespace,
/// `..` traversal, and non-http(s) schemes (file://, ftp://, ...).
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`],
/// [`REASON_MALFORMED`] (whitespace, neither bare domain nor URL, no `.`
/// separator), [`REASON_PATH_NOT_ALLOWED`] (a `..` segment) or
/// [`REASON_UNSUPPORTED_SCHEME`] (a non-http(s) URL scheme) if `value` is
/// empty, contains whitespace, `..`, a disallowed scheme, or is neither a bare
/// domain nor an http(s) URL.
pub fn require_safe_seed(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain whitespace",
            REASON_MALFORMED,
        ));
    }
    if value.contains("..") {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..'",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    // URL form: require an http(s) scheme. `split_once("://")` distinguishes
    // "https://x" (scheme present) from "example.com" (no "://", bare host).
    if let Some((scheme, _rest)) = value.split_once("://") {
        if scheme != "http" && scheme != "https" {
            return Err(invalid_params_with_reason(
                field,
                format!("unsupported scheme '{scheme}' (only http/https allowed)"),
                REASON_UNSUPPORTED_SCHEME,
            ));
        }
        return Ok(());
    }
    // Bare host form: require a domain shape (at least one '.', no '/' or ':').
    if value.contains(['/', ':']) {
        return Err(invalid_params_with_reason(
            field,
            "must be a bare domain or http(s) URL",
            REASON_MALFORMED,
        ));
    }
    if !value.contains('.') {
        return Err(invalid_params_with_reason(
            field,
            "must contain at least one '.' separator",
            REASON_MALFORMED,
        ));
    }
    Ok(())
}

/// Validate that `value` is non-empty.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`] if `value` is
/// empty.
pub fn require_non_empty(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    Ok(())
}

/// Validate that `value` does not exceed `max`.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_OUT_OF_RANGE`] if
/// `value > max`.
pub fn require_max_value_u64(field: &str, value: u64, max: u64) -> Result<(), McpError> {
    if value > max {
        return Err(invalid_params_with_reason(
            field,
            format!("must be at most {max}"),
            REASON_OUT_OF_RANGE,
        ));
    }
    Ok(())
}

/// Validate that `min <= value <= max`.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_OUT_OF_RANGE`] if `value`
/// is outside the inclusive range.
pub fn require_range_u64(field: &str, value: u64, min: u64, max: u64) -> Result<(), McpError> {
    if value < min {
        return Err(invalid_params_with_reason(
            field,
            format!("must be at least {min}"),
            REASON_OUT_OF_RANGE,
        ));
    }
    if value > max {
        return Err(invalid_params_with_reason(
            field,
            format!("must be at most {max}"),
            REASON_OUT_OF_RANGE,
        ));
    }
    Ok(())
}

/// Validate that `value` does not exceed `max`.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_OUT_OF_RANGE`] if
/// `value > max`.
pub fn require_max_value_u16(field: &str, value: u16, max: u16) -> Result<(), McpError> {
    if value > max {
        return Err(invalid_params_with_reason(
            field,
            format!("must be at most {max}"),
            REASON_OUT_OF_RANGE,
        ));
    }
    Ok(())
}

/// Validate that `value` (case-insensitive) is one of `options`.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_NOT_IN_ALLOWED_SET`] if
/// `value` does not match any option (case-insensitive).
pub fn require_one_of(field: &str, value: &str, options: &[&str]) -> Result<(), McpError> {
    let lower = value.to_ascii_lowercase();
    if options.iter().any(|o| o.eq_ignore_ascii_case(&lower)) {
        Ok(())
    } else {
        Err(invalid_params_with_reason(
            field,
            format!("must be one of: {} (got '{value}')", options.join(", ")),
            REASON_NOT_IN_ALLOWED_SET,
        ))
    }
}

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;
