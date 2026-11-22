//! The rejection envelope and its reason-slug taxonomy (EC-08, issue #1613):
//! the standard `McpError::invalid_params` builder every handler funnels
//! through, and the seven stable `REASON_*` slugs that make `data.reason` a
//! closed, branchable wire contract. Pure envelope plumbing — the validators
//! that attach the slugs live in the parent `validation` module and in
//! `path`.

use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

/// The required value is empty — fill it in.
pub const REASON_EMPTY: &str = "empty";
/// The value exceeds a byte cap — shorten it.
pub const REASON_TOO_LONG: &str = "too_long";
/// The value is present but unparseable — fix its shape.
pub const REASON_MALFORMED: &str = "malformed";
/// The URL scheme is not `http`/`https` — use one of those.
pub const REASON_UNSUPPORTED_SCHEME: &str = "unsupported_scheme";
/// The path is rejected for any structural reason (leading `/`, UNC prefix,
/// Windows drive letter, `..` traversal, non-flat filename, Windows reserved
/// name, Windows-invalid character, trailing `.`/space) — use a safe relative
/// path and a plain filename.
pub const REASON_PATH_NOT_ALLOWED: &str = "path_not_allowed";
/// A numeric value is outside its inclusive bounds — clamp it.
pub const REASON_OUT_OF_RANGE: &str = "out_of_range";
/// An enum-ish value is not in the allowed list — pick a listed value.
pub const REASON_NOT_IN_ALLOWED_SET: &str = "not_in_allowed_set";

/// Build the standard `McpError::invalid_params` envelope used by every
/// handler in this crate, WITH a stable `REASON_*` slug. `data` is
/// `{"field": "<field>", "reason": "<slug>"}`.
///
/// Every rejection branch in the crate funnels through here, which is what
/// makes the taxonomy exhaustive: a caller can always branch on `data.reason`
/// instead of parsing prose. The sibling SSRF channel
/// ([`crate::mcp_server::ssrf`]) attaches the same `data.reason` key without a
/// `field` — an SSRF refusal is not a bad field but a policy decision or a
/// server-side DNS fault — so one reader handles both.
///
/// # The reason taxonomy is a STABLE contract (EC-08, issue #1613)
///
/// `error.data` used to be a bare JSON string naming the field, and every
/// rejection shared the JSON-RPC code `-32602` — an agent could tell which
/// field was wrong but never why, so it had to parse prose. `data` is now an
/// object and `data.reason` is one of exactly SEVEN coarse slugs:
///
/// | slug | meaning | what the caller should do |
/// | :--- | :--- | :--- |
/// | [`REASON_EMPTY`] | required value is empty | fill it in |
/// | [`REASON_TOO_LONG`] | exceeds a byte cap | shorten it |
/// | [`REASON_MALFORMED`] | present but unparseable | fix its shape |
/// | [`REASON_UNSUPPORTED_SCHEME`] | not `http`/`https` | use http or https |
/// | [`REASON_PATH_NOT_ALLOWED`] | any path rejection (absolute, UNC, drive letter, `..`, non-flat filename, reserved name, illegal char, trailing `.`/space) | use a safe relative path |
/// | [`REASON_OUT_OF_RANGE`] | numeric bound | clamp it |
/// | [`REASON_NOT_IN_ALLOWED_SET`] | not in the allowed list | pick a listed value |
///
/// **Coarse is deliberate, and the set is load-bearing.** A slug's job is to
/// tell an agent what to DO, not to restate the message; seven stable values
/// beat forty brittle ones. Downstream tooling branches on these strings, so
/// do not rename one, add an eighth, or change a mapping to a different slug
/// without a matching change to the documented contract in
/// `docs/src/mcp-error-contract.md`. Message text remains the human-readable
/// half and is NOT part of this contract (the module mixes English and one
/// Spanish island by design, #1613).
///
/// There is deliberately no reason-less variant: a caller that cannot name a
/// slug should pick the closest one rather than ship an absent reason, because
/// a consumer branching on `data.reason` must be able to rely on it being
/// there. Every rejection in the crate therefore carries one.
pub(crate) fn invalid_params_with_reason(
    field: &str,
    msg: impl Into<String>,
    reason: &str,
) -> McpError {
    McpError::invalid_params(
        msg.into(),
        Some(json!({ "field": field, "reason": reason })),
    )
}

/// Read the `reason` slug out of an `invalid_params` `data` payload.
///
/// Tolerates a `data` that is `None`, a non-object (the pre-EC-08 bare
/// string), or an object without a `reason` key — every one of those yields
/// `None` rather than panicking, because `data` is sender-defined and this
/// helper sits on a success-shaped diagnostic path.
pub(crate) fn reason_of(data: Option<&Value>) -> Option<&str> {
    data?.get("reason")?.as_str()
}
