use super::tests_support::*;
use super::*;
use serial_test::serial;
use std::sync::{Arc, Mutex};

/// REQ-09: each record emits ONE structured tracing event carrying
/// tool/domain/success/duration_ms/pages (English field names/values).
///
/// `tracing` caches per-callsite `Interest` process-wide: if a NON-serial
/// test calls [`ScrapeMetrics::record`] without a subscriber active, its
/// thread registers the `"scrape recorded"` callsite as `Interest::never()`
/// and that decision is cached forever — this test's scoped `with_default`
/// subscriber would then never receive the event, even with `#[serial]`
/// (serial_test only excludes other `#[serial]` tests).
///
/// Fix: [`ensure_global_subscriber`] (invoked before capture) installs a
/// global fmt subscriber writing to sink, so every callsite registers with
/// `Interest::always()`; `set_global_default` also triggers a global
/// interest rebuild that recovers already-poisoned callsites. The global is
/// a fallback only — the per-test `with_default` below still overrides it
/// for capture on the test thread.
#[test]
#[serial]
fn record_emits_structured_tracing_event() {
    ensure_global_subscriber();
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::Layer;
    use tracing_subscriber::prelude::*;

    /// Collects `(field_name, value)` pairs as strings.
    struct Capture(Arc<Mutex<Vec<(String, String)>>>);

    impl Visit for Capture {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .expect("capture mutex")
                .push((field.name().to_string(), format!("{value:?}")));
        }
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0
                .lock()
                .expect("capture mutex")
                .push((field.name().to_string(), value.to_string()));
        }
        fn record_bool(&mut self, field: &Field, value: bool) {
            self.0
                .lock()
                .expect("capture mutex")
                .push((field.name().to_string(), value.to_string()));
        }
        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0
                .lock()
                .expect("capture mutex")
                .push((field.name().to_string(), value.to_string()));
        }
    }

    struct CaptureLayer {
        events: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = Capture(Arc::clone(&self.events));
            event.record(&mut visitor);
        }
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CaptureLayer {
        events: Arc::clone(&events),
    };
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let mut event = ScrapeEvent::new(
            "scrape_url",
            "example.com".to_string(),
            Outcome::Success,
            3,
            Duration::from_millis(120),
        );
        event.trace_id = Some("01949e0e-8b8e-7000-8000-000000000001".to_string());
        event.correlation_id =
            Some("00-01949e0e8b8e70008000000000000001-0000000000000042-01".to_string());
        ScrapeMetrics::default().record(event);
    });

    let captured = events.lock().expect("capture mutex");
    let field = |name: &str| -> Option<String> {
        captured
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(field("tool").as_deref(), Some("scrape_url"), "tool field");
    assert_eq!(
        field("domain").as_deref(),
        Some("example.com"),
        "domain field"
    );
    assert_eq!(field("success").as_deref(), Some("true"), "success field");
    assert_eq!(
        field("duration_ms").as_deref(),
        Some("120"),
        "duration_ms field"
    );
    assert_eq!(field("pages").as_deref(), Some("3"), "pages field");
    assert_eq!(
        field("trace_id").as_deref(),
        Some("01949e0e-8b8e-7000-8000-000000000001"),
        "trace_id field"
    );
    assert_eq!(
        field("correlation_id").as_deref(),
        Some("00-01949e0e8b8e70008000000000000001-0000000000000042-01"),
        "correlation_id field"
    );
}

/// OBS-M4: the reconstruction bridge. The sample the reservoir retains is
/// the SAME integer millisecond value the `scrape recorded` tracing event
/// publishes, so a `jq` pass over the `--trace-file` JSONL can recompute
/// these numbers from the raw events. This test pins that equality: the
/// multiset of captured `duration_ms` fields, sorted and ranked with the
/// documented nearest-rank rule, must equal the snapshot's percentiles.
#[test]
#[serial]
fn retained_samples_equal_the_published_duration_ms_events() {
    ensure_global_subscriber();
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::Layer;
    use tracing_subscriber::prelude::*;

    struct Capture(Arc<Mutex<Vec<String>>>);
    impl Visit for Capture {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "duration_ms" {
                self.0
                    .lock()
                    .expect("capture mutex")
                    .push(format!("{value:?}"));
            }
        }
        fn record_u64(&mut self, field: &Field, value: u64) {
            if field.name() == "duration_ms" {
                self.0
                    .lock()
                    .expect("capture mutex")
                    .push(value.to_string());
            }
        }
    }
    struct CaptureLayer {
        events: Arc<Mutex<Vec<String>>>,
    }
    impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = Capture(Arc::clone(&self.events));
            event.record(&mut visitor);
        }
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer {
        events: Arc::clone(&events),
    });

    let mut m = ScrapeMetrics::default();
    tracing::subscriber::with_default(subscriber, || {
        for ms in 1..=100u64 {
            m.record(ScrapeEvent::new(
                "scrape_url",
                "a.com".to_string(),
                Outcome::Success,
                1,
                Duration::from_millis(ms),
            ));
        }
    });

    // The jq side: `[.[] | select(.message? == "scrape recorded")
    //              | .fields.duration_ms] | sort`.
    // `message` is TOP-LEVEL in the JSONL, not inside `fields` —
    // FileTraceLayer writes `record["message"]` beside `record["fields"]`.
    // Selecting `.fields.message` yields an empty set and silently
    // reports `samples: 0`, which looks like "no scrapes" rather than
    // like a broken query.
    let mut from_jsonl: Vec<u64> = events
        .lock()
        .expect("capture mutex")
        .iter()
        .filter_map(|v| v.parse::<u64>().ok())
        .collect();
    from_jsonl.sort_unstable();
    assert_eq!(from_jsonl.len(), 100, "one event per recorded scrape");
    // Nearest-rank, exactly as the snapshot computes it.
    let near = |p: usize| from_jsonl[(p * from_jsonl.len()).div_ceil(100) - 1];
    assert_eq!(near(50), 50, "jq-side p50");
    assert_eq!(near(95), 95, "jq-side p95");
    assert_eq!(near(99), 99, "jq-side p99");

    let snap = m.snapshot();
    assert_eq!(
        snap.duration_percentiles.p50_ms,
        Some(near(50)),
        "p50 agrees"
    );
    assert_eq!(
        snap.duration_percentiles.p95_ms,
        Some(near(95)),
        "p95 agrees"
    );
    assert_eq!(
        snap.duration_percentiles.p99_ms,
        Some(near(99)),
        "p99 agrees"
    );
}

/// #698: events with no identity (`None` trace_id/correlation_id — e.g.
/// synthetic test events) must emit NO trace_id/correlation_id fields at
/// all, so the presence of a key always implies a real identity.
#[test]
#[serial]
fn record_omits_identity_when_none() {
    ensure_global_subscriber();
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::Layer;
    use tracing_subscriber::prelude::*;

    /// Collects field names only (values are irrelevant here).
    struct Keys(Arc<Mutex<Vec<String>>>);

    impl Visit for Keys {
        fn record_debug(&mut self, field: &Field, _value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .expect("keys mutex")
                .push(field.name().to_string());
        }
    }

    struct KeysLayer {
        keys: Arc<Mutex<Vec<String>>>,
    }

    impl<S: tracing::Subscriber> Layer<S> for KeysLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = Keys(Arc::clone(&self.keys));
            event.record(&mut visitor);
        }
    }

    let keys = Arc::new(Mutex::new(Vec::new()));
    let layer = KeysLayer {
        keys: Arc::clone(&keys),
    };
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        ScrapeMetrics::default().record(ScrapeEvent::new(
            "scrape_url",
            "example.com".to_string(),
            Outcome::Success,
            1,
            Duration::from_millis(10),
        ));
    });

    let keys = keys.lock().expect("keys mutex");
    assert!(!keys.contains(&"trace_id".to_string()), "no trace_id key");
    assert!(
        !keys.contains(&"correlation_id".to_string()),
        "no correlation_id key"
    );
}
