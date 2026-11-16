//! In-test `tracing` capture for integration tests.
//!
//! The in-crate capture helper (`infrastructure::observability::test_log_capture::LogBuffer`)
//! is `#[cfg(test)]`, so it does NOT exist in the library an integration test
//! links against. This module installs its own small `Layer` as the
//! thread-local default subscriber and records the fields tests assert on —
//! the same approach `export_non_utf8_policy_test.rs` takes inline. A
//! `fmt::TestWriter` would NOT do: it snapshots the buffer at install time,
//! so the assertions would read an empty log and the test would fail for the
//! wrong reason.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// One captured `tracing` event: its message and its fields, flattened to
/// strings so the assertions do not depend on the value's Rust type.
#[derive(Clone, Debug)]
pub struct CapturedEvent {
    pub message: String,
    pub fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// The value of a field, when the event carried one (unquoted).
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Flattens an event's fields into `name -> rendered value` pairs.
///
/// The event's message is the field named `message` (that is where `tracing`
/// keeps it), so it is lifted out of the map instead of being asserted
/// through it.
#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl FieldVisitor {
    /// A `Debug`-rendered value may arrive quoted (`"text"`); the quotes are
    /// not part of the value.
    fn unquote(rendered: String) -> String {
        if rendered.len() >= 2 && rendered.starts_with('"') && rendered.ends_with('"') {
            rendered[1..rendered.len() - 1].to_string()
        } else {
            rendered
        }
    }

    fn insert(&mut self, field: &Field, rendered: String) {
        if field.name() == "message" {
            self.message = Some(Self::unquote(rendered));
        } else {
            self.fields.insert(field.name().to_string(), rendered);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.insert(field, Self::unquote(format!("{value:?}")));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, value.to_string());
    }
}

/// The `Layer` that captures every event the run emits.
#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        if let Ok(mut events) = self.events.lock() {
            events.push(CapturedEvent {
                message: visitor.message.unwrap_or_default(),
                fields: visitor.fields,
            });
        }
    }
}

/// Run `body` with the capture layer installed as the thread-local default
/// subscriber and return the captured events.
///
/// The body must keep its async work on this thread (a current-thread
/// runtime inside the closure does that), or its events escape the capture.
pub fn capture(body: impl FnOnce()) -> Vec<CapturedEvent> {
    let layer = CaptureLayer::default();
    let subscriber = tracing_subscriber::registry().with(layer.clone());
    tracing::subscriber::with_default(subscriber, body);
    layer
        .events
        .lock()
        .map(|events| events.clone())
        .unwrap_or_default()
}
