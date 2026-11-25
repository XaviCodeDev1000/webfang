//! analyze_trace — Rust port of `scripts/analyze-trace.sh` (the trace cookbook).
//!
//! Reuses the FileTraceLayer parser in `webfang_benchmark::aggregate`: each
//! line is deserialized into `aggregate::TraceLine`, the same stage-1 model
//! the benchmark parser uses. The raw line is kept for the commands that echo
//! input records (`summary`, `trace`, `waf`), matching `jq -c` on compact JSONL.
//!
//! The differential harness
//! `scripts/test_artifact_analyze_trace_differential.sh` compares stdout,
//! stderr and exit code against the Bash script for all 13 commands, missing
//! files, missing arguments and injected trace ids.
//!
//! Commands: errors, slow \[N\], stages, progress, summary, counts,
//! urls-failed, trace `&lt;id&gt;`, waf, pacing, pacing-hot \[N\], admission,
//! percentiles.
//!
//! Locale note: the ordering helpers compare bytes, like GNU `sort`/`uniq`
//! under `LC_ALL=C`; the differential harness pins both sides to that locale.

use std::cmp::Ordering;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

use webfang_benchmark::aggregate::TraceLine;

#[path = "analyze_trace/whole.rs"]
mod whole;

/// Output of `grep '^#' "$0" | sed 's/^# \{0,1\}//'` on the Bash script.
const USAGE: &str = r#"!/usr/bin/env bash

analyze-trace.sh — query cookbook for WebFang FileTraceLayer JSONL output.

Usage:
  scripts/analyze-trace.sh <trace.jsonl> <command> [args]

Generate a trace first:
  webfang --url https://example.com --trace-file debug.jsonl -vvv

Commands:
  errors                 All ERROR events with url/stage/error context
  slow [N]               Top N slowest spans (default 20)
  stages                 Time/count distribution per pipeline stage
  progress               Crawl progress events over time
  summary                Final "crawl completed" summary
  counts                 Operation counts by span type
  urls-failed            Unique URLs that produced an ERROR
  trace <trace_id>       Reconstruct one operation by trace_id
  waf                    WAF challenges and banned-domain events
  pacing                 Rate-limit wait cost per pacing scope (issue #1610)
  pacing-hot [N]         Top N URLs by rate-limit wait time (default 10)
  admission              Admission/dispatch wait cost per operation (#1610)
  percentiles            p50/p95/p99 of scrape durations, from the JSONL (#1610)

Requires: jq
"#;

/// Output of `grep '^#   ' "$0" | sed 's/^#   /  /'` on the Bash script.
const USAGE_COMMANDS: &str = r#"  scripts/analyze-trace.sh <trace.jsonl> <command> [args]
  webfang --url https://example.com --trace-file debug.jsonl -vvv
  errors                 All ERROR events with url/stage/error context
  slow [N]               Top N slowest spans (default 20)
  stages                 Time/count distribution per pipeline stage
  progress               Crawl progress events over time
  summary                Final "crawl completed" summary
  counts                 Operation counts by span type
  urls-failed            Unique URLs that produced an ERROR
  trace <trace_id>       Reconstruct one operation by trace_id
  waf                    WAF challenges and banned-domain events
  pacing                 Rate-limit wait cost per pacing scope (issue #1610)
  pacing-hot [N]         Top N URLs by rate-limit wait time (default 10)
  admission              Admission/dispatch wait cost per operation (#1610)
  percentiles            p50/p95/p99 of scrape durations, from the JSONL (#1610)
"#;

/// One parsed trace line plus its raw text.
struct Line {
    text: String,
    line: TraceLine,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(file), Some(cmd)) = (args.next(), args.next()) else {
        print!("{USAGE}");
        return ExitCode::from(1);
    };
    if !Path::new(&file).is_file() {
        eprintln!("error: trace file not found: {file}");
        return ExitCode::from(1);
    }
    let rest: Vec<String> = args.collect();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match run(&file, &cmd, &rest, &mut out) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(error) => {
            eprintln!("analyze_trace: {error}");
            ExitCode::from(1)
        },
    }
}

/// Dispatch one command; returns the process exit status (0 or 1).
fn run(file: &str, cmd: &str, rest: &[String], out: &mut impl Write) -> Result<u8, String> {
    let lines = read_lines(Path::new(file))?;
    let body = match cmd {
        "errors" => cmd_errors(&lines),
        "slow" => cmd_slow(&lines, &arg_or(rest, 0, "20")),
        "stages" => cmd_stages(&lines),
        "progress" => cmd_progress(&lines),
        "summary" => cmd_summary(&lines),
        "counts" => cmd_counts(&lines),
        "urls-failed" => cmd_urls_failed(&lines),
        "trace" => {
            let id = rest.first().map_or("", String::as_str);
            if id.is_empty() {
                eprintln!("error: 'trace' requires a <trace_id> argument");
                return Ok(1);
            }
            cmd_trace(&lines, id)
        },
        "waf" => cmd_waf(&lines),
        "pacing" => whole::cmd_pacing(&lines),
        "pacing-hot" => whole::cmd_pacing_hot(&lines, &arg_or(rest, 0, "10")),
        "admission" => whole::cmd_admission(&lines),
        "percentiles" => whole::cmd_percentiles(&lines),
        _ => {
            eprintln!("error: unknown command: {cmd}");
            out.write_all(USAGE_COMMANDS.as_bytes())
                .map_err(|error| format!("write failed: {error}"))?;
            return Ok(1);
        },
    };
    let body = body?;
    out.write_all(body.as_bytes())
        .map_err(|error| format!("write failed: {error}"))?;
    Ok(0)
}

/// Read the trace file through the benchmark's stage-1 parser model.
fn read_lines(path: &Path) -> Result<Vec<Line>, String> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut lines = Vec::new();
    for (idx, raw) in BufReader::new(file).lines().enumerate() {
        let raw = raw.map_err(|error| format!("read line {}: {error}", idx + 1))?;
        if raw.trim().is_empty() {
            continue;
        }
        let parsed: TraceLine =
            serde_json::from_str(&raw).map_err(|error| format!("line {}: {error}", idx + 1))?;
        lines.push(Line {
            text: raw,
            line: parsed,
        });
    }
    Ok(lines)
}

/// `${1:-default}` — an unset OR empty argument falls back to the default.
fn arg_or(rest: &[String], index: usize, default: &str) -> String {
    match rest.get(index) {
        Some(value) if !value.is_empty() => value.clone(),
        _ => default.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Row-oriented commands
// ---------------------------------------------------------------------------

/// `jq -c 'select(.level == "ERROR") | {target, url, stage, error, msg}'`
fn cmd_errors(lines: &[Line]) -> Result<String, String> {
    let mut body = String::new();
    for entry in lines
        .iter()
        .filter(|e| e.line.level.as_deref() == Some("ERROR"))
    {
        let fields = &entry.line.fields;
        body.push_str(&format!(
            "{{\"target\":{},\"url\":{},\"stage\":{},\"error\":{},\"msg\":{}}}\n",
            jval_text(entry.line.target.as_deref()),
            jval(fields.get("url")),
            jval(fields.get("stage")),
            jval(fields.get("error")),
            jval_text(entry.line.message.as_deref()),
        ));
    }
    Ok(body)
}

/// `jq -r 'select(.span_duration_ms != null) | [.span_duration_ms, .span] | @tsv' | sort -rn | head`
fn cmd_slow(lines: &[Line], n: &str) -> Result<String, String> {
    let limit = n.parse::<usize>().unwrap_or(20);
    let mut rows: Vec<String> = lines
        .iter()
        .filter_map(|entry| {
            entry.line.span_duration_ms.map(|duration| {
                let span = entry.line.span.as_deref().unwrap_or("");
                format!("{}\t{}", fmt_num(duration), span)
            })
        })
        .collect();
    sort_rn(&mut rows);
    rows.truncate(limit);
    Ok(rows_to_body(&rows))
}

/// `jq -r 'select(.span != null and .record != "span_close") | .span' | sort | uniq -c | sort -rn`
fn cmd_stages(lines: &[Line]) -> Result<String, String> {
    let names: Vec<String> = lines
        .iter()
        .filter(|e| e.line.span.is_some() && e.line.record.as_deref() != Some("span_close"))
        .filter_map(|e| e.line.span.clone())
        .collect();
    Ok(distribution(names))
}

/// `jq -c 'select(.message == "crawl progress") | {pages, pct, eta_s}'`
fn cmd_progress(lines: &[Line]) -> Result<String, String> {
    let mut body = String::new();
    for entry in lines
        .iter()
        .filter(|e| e.line.message.as_deref() == Some("crawl progress"))
    {
        let fields = &entry.line.fields;
        body.push_str(&format!(
            "{{\"pages\":{},\"pct\":{},\"eta_s\":{}}}\n",
            jval(fields.get("pages_crawled")),
            jval(fields.get("progress_pct")),
            jval(fields.get("eta_secs")),
        ));
    }
    Ok(body)
}

/// `jq -c 'select(.message == "crawl completed")'` — compact records.
fn cmd_summary(lines: &[Line]) -> Result<String, String> {
    let mut body = String::new();
    for entry in lines
        .iter()
        .filter(|e| e.line.message.as_deref() == Some("crawl completed"))
    {
        body.push_str(&compact_json(&entry.text));
        body.push('\n');
    }
    Ok(body)
}

/// `jq -r 'select(.record != "span_close") | .span // "event"' | sort | uniq -c | sort -rn`
fn cmd_counts(lines: &[Line]) -> Result<String, String> {
    let names: Vec<String> = lines
        .iter()
        .filter(|e| e.line.record.as_deref() != Some("span_close"))
        .map(|e| e.line.span.clone().unwrap_or_else(|| "event".to_string()))
        .collect();
    Ok(distribution(names))
}

/// `jq -r 'select(.level == "ERROR") | .fields.url // empty' | sort -u`
fn cmd_urls_failed(lines: &[Line]) -> Result<String, String> {
    let mut urls: Vec<String> = lines
        .iter()
        .filter(|e| e.line.level.as_deref() == Some("ERROR"))
        .filter_map(|e| match e.line.fields.get("url") {
            Some(Value::String(url)) => Some(url.clone()),
            _ => None,
        })
        .collect();
    urls.sort();
    urls.dedup();
    Ok(rows_to_body(&urls))
}

/// `jq -c --arg t "$TRACE" 'select(.trace_id == $t)'` — id is DATA, never a filter.
fn cmd_trace(lines: &[Line], id: &str) -> Result<String, String> {
    let mut body = String::new();
    for entry in lines
        .iter()
        .filter(|e| e.line.trace_id.as_deref() == Some(id))
    {
        body.push_str(&compact_json(&entry.text));
        body.push('\n');
    }
    Ok(body)
}

/// `jq -c 'select((.fields.message? // "") | test("WAF|Banned domain"; "i"))'`
fn cmd_waf(lines: &[Line]) -> Result<String, String> {
    let mut body = String::new();
    for entry in lines {
        let message = entry
            .line
            .fields
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("");
        let lowered = message.to_lowercase();
        if lowered.contains("waf") || lowered.contains("banned domain") {
            body.push_str(&compact_json(&entry.text));
            body.push('\n');
        }
    }
    Ok(body)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn rows_to_body(rows: &[String]) -> String {
    let mut body = String::new();
    for row in rows {
        body.push_str(row);
        body.push('\n');
    }
    body
}

/// `sort | uniq -c | sort -rn` — GNU `uniq -c` uses a 7-wide right-justified
/// count followed by one space; GNU `sort -rn` parses the leading number and
/// reverses the last-resort line comparison on ties.
fn distribution(mut names: Vec<String>) -> String {
    names.sort();
    let mut rows: Vec<String> = Vec::new();
    let mut index = 0;
    while index < names.len() {
        let mut count = 1usize;
        while index + count < names.len() && names[index + count] == names[index] {
            count += 1;
        }
        rows.push(format!("{count:>7} {}", names[index]));
        index += count;
    }
    sort_rn(&mut rows);
    rows_to_body(&rows)
}

fn sort_rn(rows: &mut [String]) {
    rows.sort_by(|a, b| {
        let (left, right) = (sort_number(a), sort_number(b));
        right
            .partial_cmp(&left)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.cmp(a))
    });
}

/// GNU `sort -n`: the longest numeric prefix, 0 when the line has none.
fn sort_number(line: &str) -> f64 {
    let text = line.trim_start();
    let bytes = text.as_bytes();
    let mut end = usize::from(bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')));
    let mut digits = false;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
        digits = true;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
            digits = true;
        }
    }
    if !digits {
        return 0.0;
    }
    if end < bytes.len() && matches!(bytes[end], b'e' | b'E') {
        let mut cursor = end + 1;
        if cursor < bytes.len() && matches!(bytes[cursor], b'+' | b'-') {
            cursor += 1;
        }
        let mut exponent_digits = false;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
            exponent_digits = true;
        }
        if exponent_digits {
            end = cursor;
        }
    }
    text[..end].parse::<f64>().unwrap_or(0.0)
}

/// jq's compact JSON rendering of a scalar value (or `null` when absent).
fn jval(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => fmt_num(number.as_f64().unwrap_or(0.0)),
        Some(Value::String(text)) => format!("\"{}\"", jq_escape(text)),
        Some(other) => other.to_string(),
    }
}

/// jq's compact JSON rendering of a top-level string field (or `null`).
fn jval_text(value: Option<&str>) -> String {
    match value {
        Some(text) => format!("\"{}\"", jq_escape(text)),
        None => "null".to_string(),
    }
}

/// jq's `@tsv` text rendering of a scalar.
fn jq_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => fmt_num(number.as_f64().unwrap_or(0.0)),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// jq number text: integral values carry no decimal point.
fn fmt_num(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Compact one already-parsed JSON line the way `jq -c` does: drop
/// insignificant whitespace while preserving key order and string bytes.
/// (`serde_json::Value` would reorder keys through its map implementation.)
fn compact_json(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            out.push(byte as char);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
            out.push(byte as char);
        } else if !byte.is_ascii_whitespace() {
            out.push(byte as char);
        }
        index += 1;
    }
    out
}

fn jq_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
