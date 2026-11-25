//! Whole-file (`jq -s`) commands of `analyze_trace` and their jq pretty printer.
//!
//! Kept in a sibling module so both this file and the row-oriented driver stay
//! under the repository's 500-line file-size limit.

use std::cmp::Ordering;

use serde_json::Value;

use super::{fmt_num, jq_escape, jq_text, rows_to_body, sort_rn, Line};

/// One `group_by(.fields.<key>)` bucket.
struct Group {
    key: Value,
    events: u64,
    total: f64,
    max_waited: f64,
    max_in_flight: f64,
    cancelled: u64,
}

const PACING_MESSAGES: [&str; 1] = ["rate limit wait"];
const ADMISSION_MESSAGES: [&str; 4] = [
    "semaphore acquire",
    "inference pool permit",
    "inference request queued for a worker",
    "inference worker took a request",
];

/// Group events by a `fields` key, jq's `group_by` sort order included.
fn groups_for(lines: &[Line], messages: &[&str], key: &str, count_cancelled: bool) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for entry in lines.iter().filter(|e| {
        e.line
            .message
            .as_deref()
            .is_some_and(|m| messages.contains(&m))
    }) {
        let fields = &entry.line.fields;
        let value = fields.get(key).cloned().unwrap_or(Value::Null);
        let index = match groups.iter().position(|g| g.key == value) {
            Some(index) => index,
            None => {
                groups.push(Group {
                    key: value.clone(),
                    events: 0,
                    total: 0.0,
                    max_waited: 0.0,
                    max_in_flight: 0.0,
                    cancelled: 0,
                });
                groups.len() - 1
            },
        };
        let group = &mut groups[index];
        let waited = jq_or_zero(fields.get("waited_ms"));
        group.events += 1;
        group.total += waited;
        group.max_waited = group.max_waited.max(waited);
        group.max_in_flight = group.max_in_flight.max(jq_or_zero(fields.get("in_flight")));
        if count_cancelled && fields.get("outcome").and_then(Value::as_str) == Some("cancelled") {
            group.cancelled += 1;
        }
    }
    groups.sort_by(|a, b| jq_cmp(&a.key, &b.key));
    groups
}

/// `jq -s 'map(select(...)) | group_by(.fields.scope) | map({scope, waits, ...})'`
pub(super) fn cmd_pacing(lines: &[Line]) -> Result<String, String> {
    let groups = groups_for(lines, &PACING_MESSAGES, "scope", true);
    let items: Vec<Pretty> = groups
        .iter()
        .map(|g| {
            Pretty::Object(vec![
                ("scope", pretty_value(&g.key)),
                ("waits", Pretty::Num(g.events as f64)),
                ("total_waited_ms", Pretty::Num(g.total)),
                ("max_waited_ms", Pretty::Num(g.max_waited)),
                ("cancelled", Pretty::Num(g.cancelled as f64)),
            ])
        })
        .collect();
    Ok(pretty_body(&Pretty::Array(items)))
}

/// `jq -r 'select(...) | [...] | @tsv' | awk ... | sort -rn | head`
pub(super) fn cmd_pacing_hot(lines: &[Line], n: &str) -> Result<String, String> {
    let limit = n.parse::<usize>().unwrap_or(10);
    let mut rows: Vec<(String, f64)> = Vec::new(); // (scope<TAB>url, total)
    for entry in lines
        .iter()
        .filter(|e| e.line.message.as_deref() == Some("rate limit wait"))
    {
        let fields = &entry.line.fields;
        let waited = jq_or_zero(fields.get("waited_ms"));
        let scope = jq_text(fields.get("scope"));
        let url = match fields.get("url") {
            Some(Value::String(value)) => value.clone(),
            _ => "unknown".to_string(),
        };
        let key = format!("{scope}\t{url}");
        match rows.iter_mut().find(|(existing, _)| *existing == key) {
            Some(row) => row.1 += waited,
            None => rows.push((key, waited)),
        }
    }
    let mut lines_out: Vec<String> = rows
        .into_iter()
        .map(|(key, total)| format!("{}\t{key}", fmt_num(total)))
        .collect();
    sort_rn(&mut lines_out);
    lines_out.truncate(limit);
    Ok(rows_to_body(&lines_out))
}

/// `jq -s 'map(select(...)) | group_by(.fields.operation) | map({operation, ...})'`
pub(super) fn cmd_admission(lines: &[Line]) -> Result<String, String> {
    let groups = groups_for(lines, &ADMISSION_MESSAGES, "operation", false);
    let items: Vec<Pretty> = groups
        .iter()
        .map(|g| {
            Pretty::Object(vec![
                ("operation", pretty_value(&g.key)),
                ("events", Pretty::Num(g.events as f64)),
                ("total_waited_ms", Pretty::Num(g.total)),
                ("max_waited_ms", Pretty::Num(g.max_waited)),
                ("max_in_flight", Pretty::Num(g.max_in_flight)),
            ])
        })
        .collect();
    Ok(pretty_body(&Pretty::Array(items)))
}

/// `jq -s '[.[] | select(...scrape recorded...) | .fields.duration_ms] | sort | ... nr(50/95/99)'`
pub(super) fn cmd_percentiles(lines: &[Line]) -> Result<String, String> {
    let mut samples: Vec<Option<f64>> = lines
        .iter()
        .filter(|e| e.line.message.as_deref() == Some("scrape recorded"))
        .map(|e| e.line.fields.get("duration_ms").and_then(Value::as_f64))
        .collect();
    samples.sort_by(|a, b| match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => x.total_cmp(y),
    });
    let n = samples.len();
    // Nearest rank: jq's floor((p*n + 99) / 100) - 1, 1-based → 0-based.
    let nearest_rank = |p: u32| -> Option<f64> {
        if n == 0 {
            return None;
        }
        let index = ((f64::from(p) * n as f64 + 99.0) / 100.0).floor() as i64 - 1;
        if index < 0 {
            return None;
        }
        samples.get(index as usize).copied().flatten()
    };
    let value = Pretty::Object(vec![
        ("samples", Pretty::Num(n as f64)),
        ("p50_ms", optional_pretty(nearest_rank(50))),
        ("p95_ms", optional_pretty(nearest_rank(95))),
        ("p99_ms", optional_pretty(nearest_rank(99))),
    ]);
    Ok(pretty_body(&value))
}

/// jq's `// 0`: null, false and missing all become zero.
fn jq_or_zero(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn jq_rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

/// jq's `sort`/`group_by` ordering: type rank, then value.
fn jq_cmp(left: &Value, right: &Value) -> Ordering {
    jq_rank(left)
        .cmp(&jq_rank(right))
        .then_with(|| match (left, right) {
            (Value::Number(a), Value::Number(b)) => a
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&b.as_f64().unwrap_or(0.0)),
            (Value::String(a), Value::String(b)) => a.cmp(b),
            _ => Ordering::Equal,
        })
}

/// Minimal jq pretty-printer: 2-space indent, insertion-ordered object keys,
/// `[]`/`{}` for empty containers, one trailing newline for the whole value.
enum Pretty {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Array(Vec<Pretty>),
    Object(Vec<(&'static str, Pretty)>),
}

fn pretty_value(value: &Value) -> Pretty {
    match value {
        Value::Null => Pretty::Null,
        Value::Bool(flag) => Pretty::Bool(*flag),
        Value::Number(number) => Pretty::Num(number.as_f64().unwrap_or(0.0)),
        Value::String(text) => Pretty::Str(text.clone()),
        Value::Array(_) | Value::Object(_) => Pretty::Null,
    }
}

fn optional_pretty(value: Option<f64>) -> Pretty {
    match value {
        Some(number) => Pretty::Num(number),
        None => Pretty::Null,
    }
}

fn pretty_body(value: &Pretty) -> String {
    let mut body = String::new();
    pretty_into(value, 0, &mut body);
    body.push('\n');
    body
}

fn pretty_into(value: &Pretty, indent: usize, body: &mut String) {
    match value {
        Pretty::Null => body.push_str("null"),
        Pretty::Bool(flag) => body.push_str(if *flag { "true" } else { "false" }),
        Pretty::Num(number) => body.push_str(&fmt_num(*number)),
        Pretty::Str(text) => {
            body.push('"');
            body.push_str(&jq_escape(text));
            body.push('"');
        },
        Pretty::Array(items) => {
            if items.is_empty() {
                body.push_str("[]");
                return;
            }
            body.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                body.push_str(&"  ".repeat(indent + 1));
                pretty_into(item, indent + 1, body);
                if index + 1 < items.len() {
                    body.push(',');
                }
                body.push('\n');
            }
            body.push_str(&"  ".repeat(indent));
            body.push(']');
        },
        Pretty::Object(entries) => {
            if entries.is_empty() {
                body.push_str("{}");
                return;
            }
            body.push_str("{\n");
            for (index, (key, entry)) in entries.iter().enumerate() {
                body.push_str(&"  ".repeat(indent + 1));
                body.push('"');
                body.push_str(key);
                body.push_str("\": ");
                pretty_into(entry, indent + 1, body);
                if index + 1 < entries.len() {
                    body.push(',');
                }
                body.push('\n');
            }
            body.push_str(&"  ".repeat(indent));
            body.push('}');
        },
    }
}
