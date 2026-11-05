//! JSON-LD author resolution — helpers extracted from `author_extractor/mod.rs`.
//!
//! These four functions form a cohesive cluster: `author_from_jsonld` is the
//! entry point called by [`JsonLdAuthorExtractor`](super::JsonLdAuthorExtractor),
//! and `strip_byline_prefix` is called by
//! [`CssClassAuthorExtractor`](super::CssClassAuthorExtractor). The other two
//! (`find_author`, `author_name`) are internal to the cluster.

use serde_json::Value;

use super::non_empty;

/// Leading byline prefixes stripped from CSS-class candidates.
///
/// Ordered longest-first so "written by" wins over "by".
const BYLINE_PREFIXES: &[&str] = &["written by", "por", "by"];

/// Resolve an author name from a raw JSON-LD block.
pub(super) fn author_from_jsonld(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    find_author(&value)
}

/// Recursively search a JSON-LD value for an `author` field.
fn find_author(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(name) = map.get("author").and_then(author_name) {
                return Some(name);
            }
            map.get("@graph").and_then(find_author)
        },
        Value::Array(items) => items.iter().find_map(find_author),
        _ => None,
    }
}

/// Coerce an `author` JSON value (string, object, or array) into a name.
fn author_name(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => non_empty(s),
        Value::Object(map) => map.get("name").and_then(Value::as_str).and_then(non_empty),
        Value::Array(items) => items.iter().find_map(author_name),
        _ => None,
    }
}

/// Strip a leading byline prefix ("By ", "Written by ", "Por ") if present.
pub(super) fn strip_byline_prefix(s: &str) -> String {
    let trimmed = s.trim();
    let lower = trimmed.to_lowercase();
    for prefix in BYLINE_PREFIXES {
        let Some(rest) = lower.strip_prefix(prefix) else {
            continue;
        };
        if rest.starts_with(' ') || rest.starts_with('\u{a0}') {
            return trimmed[prefix.len()..].trim().to_string();
        }
    }
    trimmed.to_string()
}
