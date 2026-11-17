use super::*;
use std::time::Duration;

static GLOBAL_SUBSCRIBER_INIT: std::sync::Once = std::sync::Once::new();

/// Set a global fmt subscriber (sink writer) so every `tracing` callsite
/// registers with `Interest::always()` instead of the `Interest::never()`
/// that gets cached process-wide when a thread hits a callsite with no
/// subscriber active (see [`record_emits_structured_tracing_event`]).
pub(super) fn ensure_global_subscriber() {
    GLOBAL_SUBSCRIBER_INIT.call_once(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .finish(),
        );
    });
}

/// Build the canonical fixture: 2 success events on `a.com` (counts 3, 5)
/// and 1 error event on `b.com` (count 0), durations 100/200/300ms, all via
/// the `scrape_url` tool. Mean duration = (100+200+300)/3 = 200ms.
pub(super) fn record_fixture(m: &mut ScrapeMetrics) {
    m.record(ScrapeEvent::new(
        "scrape_url",
        "a.com".to_string(),
        Outcome::Success,
        3,
        Duration::from_millis(100),
    ));
    m.record(ScrapeEvent::new(
        "scrape_url",
        "a.com".to_string(),
        Outcome::Success,
        5,
        Duration::from_millis(200),
    ));
    m.record(ScrapeEvent::new(
        "scrape_url",
        "b.com".to_string(),
        Outcome::Error,
        0,
        Duration::from_millis(300),
    ));
}
