//! File-based tracing layer — writes JSONL trace files without OTel dependency.
//!
//! Enabled when `--trace-file <path>` is passed. Each line is one JSON object
//! representing a tracing event or span. Replaces the OTel-coupled
//! `FileTraceExporter` with a native `tracing_subscriber::Layer`.
//!
//! The file is **truncated** on creation — each run produces a clean trace file.
//! Structured fields from `tracing::info!(key = value, ...)` are captured in the
//! `fields` object. `trace_id` is a **logical** identifier: it uses the root
//! span's ID (stable across worker threads) and finally a stable per-invocation
//! seed. This replaces the old
//! thread-ID-based `trace_id`, which fragmented across threads. `parent_id` is
//! also emitted so the logical trace tree is reconstructable from the JSONL.
//!
//! Span context (`span`, `span_id`, `parent_id`, `trace_id`, `span_fields`)
//! is derived from the subscriber `Context` / span scope — the same source
//! `on_close` uses. No thread-local span stack is consulted, so events keep
//! attribution when Tokio moves a task across worker threads (issue #1238).
//!
//! `span_fields` holds BOTH the fields declared when the span was created
//! (`on_new_span`) and the ones filled in afterwards via
//! `tracing::Span::record` (`on_record`) — a span's outcome is only known at
//! the end of the operation, so it must reach the JSONL after the fact
//! (#1610, OBS-P2-4).
//!
//! Top-level `trace_id` is the root span's `Id` formatted as 16-hex
//! (`format!("{:016x}", root.id().into_u64())`). One logical `trace_id` per
//! run: every record inside the run's root scope shares it, so the full JSONL
//! is reconstructable with `select(.trace_id == $ROOT)`. This value is
//! EPHEMERAL to the process/run (identity-within-run, not durable global
//! identity): span `Id`s are assigned by the in-process registry and are not
//! stable across processes. Do not persist or join on it across runs. The
//! durable run correlation lives in `span_fields.trace_id` /
//! `span_fields.correlation_id` (`CorrelationId`, W3C traceparent), not here.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use serde_json::{json, Map, Value};
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// A `tracing_subscriber::Layer` that writes JSONL trace files.
///
/// Each line is a JSON object with: `timestamp` (RFC3339), `level`, `target`,
/// `span` (name, when inside a span), `trace_id`, `span_id`, `message`,
/// and `fields` (all structured key-value pairs from the event).
///
/// Top-level `trace_id` is EPHEMERAL identity-within-run: the root span's `Id`
/// (16-hex), single per run, for offline reconstruction. It is not a durable
/// global identity; durable correlation is `CorrelationId` in `span_fields`.
///
/// When a span closes, an additional record with `"record": "span_close"` is
/// emitted carrying a **top-level** `span_duration_ms` (wall-clock milliseconds
/// from span creation to close), so the `analyze-trace.sh <file> slow` workflow
/// can rank spans by duration.
pub struct FileTraceLayer {
    writer: Mutex<BufWriter<File>>,
    /// Stable per-invocation seed used as the fallback `trace_id` when no
    /// logical span context is available (e.g. events outside any span).
    /// Like the root-span `trace_id`, this seed is EPHEMERAL to the run.
    trace_id_seed: u64,
}

impl FileTraceLayer {
    /// Create a new file trace layer, opening (or creating) the file at `path`.
    ///
    /// Parent directories are created automatically. The file is truncated on
    /// creation so each run produces a clean trace file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created or opened.
    pub fn new(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = File::create(&path)?;
        let writer = BufWriter::new(file);
        Ok(Self {
            writer: Mutex::new(writer),
            trace_id_seed: make_trace_seed(),
        })
    }

    /// Serialize one trace record as a JSONL line and append it to the file.
    ///
    /// Owns the whole lock → serialize → write → flush → report chain, so
    /// every emitter shares identical failure paths. A poisoned lock is
    /// recovered rather than skipped: the guard still grants exclusive access,
    /// and dropping trace lines silently is the one thing a trace sink must not
    /// do. Never blocks across `.await`: all callers are synchronous `Layer`
    /// callbacks.
    fn write_record(&self, record: &Value) {
        {
            let mut writer = self
                .writer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut line = match serde_json::to_vec(record) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[FileTraceLayer] serialization error: {e}");
                    return;
                },
            };
            line.push(b'\n');
            if let Err(e) = writer.write_all(&line) {
                eprintln!("[FileTraceLayer] write error: {e}");
            }
            if let Err(e) = writer.flush() {
                eprintln!("[FileTraceLayer] flush error: {e}");
            }
        }
    }
}

impl std::fmt::Debug for FileTraceLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileTraceLayer").finish_non_exhaustive()
    }
}

impl Drop for FileTraceLayer {
    fn drop(&mut self) {
        if let Ok(writer) = self.writer.get_mut() {
            if let Err(e) = writer.flush() {
                eprintln!("[FileTraceLayer] flush on drop failed: {e}");
            }
        }
    }
}

impl<S> Layer<S> for FileTraceLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::Id,
        ctx: Context<'_, S>,
    ) {
        // Capture the span's structured fields at creation time so they can be
        // serialized into the JSONL later (S2 offline visibility).
        let mut recorder = EventRecorder::new();
        attrs.record(&mut recorder);
        if let Some(span_ref) = ctx.span(id) {
            span_ref.extensions_mut().insert(recorder);
            // Wall-clock start for span_duration_ms, read back at on_close.
            span_ref.extensions_mut().insert(SpanTimings {
                start: Instant::now(),
            });
        }
    }

    /// Merge fields recorded AFTER span creation (#1610, OBS-P2-4).
    ///
    /// `on_new_span` snapshots the declared fields, which is why an
    /// `#[instrument(fields(...))]` span can carry its identity offline. The
    /// outcome of a span, however, is only known when the operation ends —
    /// it has to be declared `Empty` and filled in later with
    /// [`tracing::Span::record`]. Without this hook those records went
    /// nowhere: the span closed with an `Empty` field invisible and the
    /// outcome lived only in a separate event, which is exactly the gap
    /// OBS-P2-4 reports.
    ///
    /// The recorded values land in the SAME `EventRecorder` extension the
    /// creation-time snapshot uses, so they appear in `span_fields` of every
    /// subsequent event inside the span AND in the `span_close` record.
    fn on_record(&self, id: &tracing::Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        let Some(span_ref) = ctx.span(id) else {
            return;
        };
        let mut extensions = span_ref.extensions_mut();
        if let Some(recorder) = extensions.get_mut::<EventRecorder>() {
            values.record(recorder);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let meta = event.metadata();

        let mut record = json!({
            "timestamp": system_time_to_rfc3339(SystemTime::now()),
            "level": meta.level().as_str(),
            "target": meta.target(),
        });

        // Span context from the subscriber Context (same source as on_close).
        // Never consult a thread-local span stack here: under Tokio
        // multi_thread the polling thread changes per `.await`, so a
        // thread-local view orphans events (issue #1238).
        if let Some(current) = ctx.lookup_current() {
            record["span"] = json!(current.name());
            record["span_id"] = json!(format!("{:016x}", current.id().into_u64()));

            // Capture span fields from the CURRENT span AND its parents (the
            // scope), so the root span's fields (e.g. the seed URL) become
            // visible offline without an external OTel collector (S2).
            // Child fields take precedence on key collision.
            let mut span_fields: Map<String, Value> = Map::new();
            for ancestor in current.scope() {
                if let Some(rec) = ancestor.extensions().get::<EventRecorder>() {
                    for (k, v) in &rec.fields {
                        span_fields.entry(k.clone()).or_insert(v.clone());
                    }
                }
            }
            if !span_fields.is_empty() {
                record["span_fields"] = Value::Object(span_fields);
            }

            // parent_id: reconstruct the logical trace tree. The current span's
            // parent (enclosing span) is serialized so the tree is recoverable
            // from the JSONL.
            if let Some(parent) = current.parent() {
                record["parent_id"] = json!(format!("{:016x}", parent.id().into_u64()));
            }

            // trace_id: logical identifier that survives thread hops. Uses the
            // root span's ID (stable for the whole run, survives worker-thread
            // hops) with a stable per-invocation seed fallback.
            let trace_id = logical_trace_id(self.trace_id_seed, &ctx);
            record["trace_id"] = json!(trace_id);
        } else {
            record["trace_id"] = json!(format!("{:016x}", self.trace_id_seed));
        }

        // Single-pass field capture: extracts all fields AND the message
        // in one traversal, avoiding the double-visit antipattern.
        let mut recorder = EventRecorder::new();
        event.record(&mut recorder);

        if !recorder.fields.is_empty() {
            record["fields"] = Value::Object(recorder.fields);
        }
        if let Some(m) = recorder.message {
            record["message"] = json!(m);
        }

        self.write_record(&record);
    }

    fn on_close(&self, id: tracing::Id, ctx: Context<'_, S>) {
        // Graceful entry — never panic if the span is already gone.
        let Some(span_ref) = ctx.span(&id) else {
            return;
        };

        let meta = span_ref.metadata();

        // Owned duration so the extensions borrow is released before the record
        // is built. Wall-clock lifetime: Instant captured at on_new_span.
        let duration_ms = span_ref
            .extensions()
            .get::<SpanTimings>()
            .map(|t| t.start.elapsed().as_millis() as u64);

        let mut record = json!({
            "timestamp": system_time_to_rfc3339(SystemTime::now()),
            "record": "span_close",
            "level": meta.level().as_str(),
            "target": meta.target(),
            "span": span_ref.name(),
            "span_id": format!("{:016x}", span_ref.id().into_u64()),
        });

        if let Some(d) = duration_ms {
            record["span_duration_ms"] = json!(d);
        }

        if let Some(parent) = span_ref.parent() {
            record["parent_id"] = json!(format!("{:016x}", parent.id().into_u64()));
        }

        // trace_id from the CLOSING span's own scope root — NOT
        // ctx.lookup_current(), which is the thread-local current span and is
        // wrong inside on_close.
        let trace_id = span_ref
            .scope()
            .last()
            .map(|root| format!("{:016x}", root.id().into_u64()))
            .unwrap_or_else(|| format!("{:016x}", self.trace_id_seed));
        record["trace_id"] = json!(trace_id);

        if let Some(rec) = span_ref.extensions().get::<EventRecorder>() {
            if !rec.fields.is_empty() {
                record["span_fields"] = Value::Object(rec.fields.clone());
            }
        }

        self.write_record(&record);
    }
}

/// Resolve a logical `trace_id` independent of the OS thread, so it stays
/// stable when a task hops between worker threads (D3). Prefers the root
/// span's ID; falls back to a per-invocation seed when no span is current.
///
/// The returned value is EPHEMERAL identity-within-run (root span `Id` as
/// 16-hex, one per run), not a durable global identity. Reconstruct the run
/// with `select(.trace_id == $ROOT)`; correlate across systems with
/// `CorrelationId` in `span_fields`, not with this field.
fn logical_trace_id<S>(seed: u64, ctx: &Context<'_, S>) -> String
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if let Some(current) = ctx.lookup_current() {
        if let Some(root) = current.scope().last() {
            return format!("{:016x}", root.id().into_u64());
        }
    }
    format!("{seed:016x}")
}

/// Build a stable per-invocation seed used as the fallback `trace_id` when no
/// logical span context is available.
fn make_trace_seed() -> u64 {
    use std::hash::{Hash, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    hasher.finish()
}

/// Wall-clock start stored in span extensions at `on_new_span`, read at
/// `on_close` to compute `span_duration_ms`.
struct SpanTimings {
    start: Instant,
}

/// Single-pass event recorder. Captures ALL fields (including `message`) in one
/// traversal of the event's field set, avoiding the double-visit antipattern.
#[derive(Clone)]
struct EventRecorder {
    fields: Map<String, Value>,
    message: Option<String>,
}

impl EventRecorder {
    fn new() -> Self {
        Self {
            fields: Map::new(),
            message: None,
        }
    }
}

/// Sink-level backstop: strip userinfo from any URL embedded in a field or
/// message, including emitters that predate the redactor-aware call sites.
///
/// Only the `userinfo@` segment of each `scheme://userinfo@host` occurrence is
/// removed; the surrounding text, the scheme, host, port, path and query stay
/// byte-for-byte. Values without a credentialed URL are returned unchanged, so
/// existing trace contracts hold.
fn redact_url_value(value: &str) -> String {
    /// Bytes that end a URL authority inside free text.
    const AUTHORITY_END: [char; 9] = ['/', '?', '#', '"', '\'', ')', ']', ' ', '\t'];

    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(marker) = value[cursor..].find("://") {
        let authority_start = cursor + marker + 3;
        let tail = &value[authority_start..];
        let authority_end = tail.find(AUTHORITY_END).unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        out.push_str(&value[cursor..authority_start]);
        match authority.rfind('@') {
            // RFC 3986: the LAST `@` of the authority separates userinfo.
            Some(at) => out.push_str(&authority[at + 1..]),
            None => out.push_str(authority),
        }
        cursor = authority_start + authority_end;
    }
    out.push_str(&value[cursor..]);
    out
}

impl tracing::field::Visit for EventRecorder {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let name = field.name().to_string();
        let formatted = format!("{value:?}");
        let text = if formatted.starts_with('"') && formatted.ends_with('"') && formatted.len() >= 2
        {
            formatted[1..formatted.len() - 1].to_string()
        } else {
            formatted
        };
        let text = redact_url_value(&text);

        if name == "message" {
            self.message = Some(text);
        } else {
            self.fields.insert(name, Value::String(text));
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), Value::Bool(value));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        let text = redact_url_value(value);
        if field.name() == "message" {
            self.message = Some(text);
        } else {
            self.fields
                .insert(field.name().to_string(), Value::String(text));
        }
    }
}

/// Convert a `SystemTime` to an RFC 3339 string with millisecond precision.
///
/// Format: `2025-07-09T20:41:34.123Z`
fn system_time_to_rfc3339(t: SystemTime) -> String {
    let duration = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    // Days since 1970-01-00 (Howard Hinnant algorithm)
    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if m <= 2 { y + 1 } else { y };

    format!("{yr:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

#[cfg(test)]
#[path = "file_trace_layer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "file_trace_layer_span_tests.rs"]
mod span_tests;
