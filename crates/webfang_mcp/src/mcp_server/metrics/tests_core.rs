use super::tests_support::*;
use super::*;
use std::sync::{Arc, Mutex};

/// REQ-02: aggregates accumulate exactly (counts, per-domain, timing mean).
#[test]
fn aggregates_exact_numbers() {
    let mut m = ScrapeMetrics::default();
    record_fixture(&mut m);

    let snap = m.snapshot();
    assert_eq!(snap.total_events, 3, "total_events");
    assert_eq!(snap.success_count, 2, "success_count");
    assert_eq!(snap.error_count, 1, "error_count");
    assert_eq!(
        snap.domains.get("a.com"),
        Some(&DomainStats {
            events: 2,
            pages: 8,
            errors: 0
        }),
        "domain a.com aggregate"
    );
    assert_eq!(
        snap.domains.get("b.com"),
        Some(&DomainStats {
            events: 1,
            pages: 0,
            errors: 1
        }),
        "domain b.com aggregate"
    );
    assert_eq!(snap.average_duration_ms, 200.0, "average_duration_ms");
    assert!(!m.is_empty(), "non-empty after recording");
}

/// REQ-05: the 501st distinct domain aggregates into the `"otros"`
/// overflow bucket while global totals (events, outcomes, duration mean)
/// stay exact — totals are updated unconditionally before the cap.
///
/// 600 distinct domains, one event each, outcome cycling
/// Success/Error/Partial by `i % 3`, page count `(i % 5) + 1`, duration
/// cycling 10/20/30/40 ms. The first 500 keep their own keys; domains
/// 500..600 (100 events) merge: pages = 20 cycles × (1+2+3+4+5) = 300,
/// errors = the 67 events whose `i % 3 != 0`. Mean duration over all 600
/// events = exactly 25 ms.
#[test]
fn domains_capped_at_500_with_otros_overflow() {
    let mut m = ScrapeMetrics::default();
    for i in 0..600usize {
        let outcome = match i % 3 {
            0 => Outcome::Success,
            1 => Outcome::Error,
            _ => Outcome::Partial,
        };
        m.record(ScrapeEvent::new(
            "scrape_url",
            format!("d{i:04}.com"),
            outcome,
            (i % 5) + 1,
            Duration::from_millis(((i % 4) + 1) as u64 * 10),
        ));
    }

    let snap = m.snapshot();
    assert_eq!(snap.total_events, 600, "global total unaffected by cap");
    assert_eq!(snap.success_count, 200, "success_count exact");
    assert_eq!(snap.error_count, 200, "error_count exact");
    assert_eq!(snap.partial_count, 200, "partial_count exact");
    assert_eq!(snap.average_duration_ms, 25.0, "duration mean exact");

    assert_eq!(
        snap.domains.len(),
        501,
        "500 tracked domains + one overflow bucket"
    );
    assert_eq!(
        snap.domains.get("otros"),
        Some(&DomainStats {
            events: 100,
            pages: 300,
            errors: 67
        }),
        "overflow aggregates with identical semantics"
    );
    assert!(
        !snap.domains.contains_key("d0500.com"),
        "overflow domains must not get their own key"
    );
    assert_eq!(
        snap.domains.get("d0000.com"),
        Some(&DomainStats {
            events: 1,
            pages: 1,
            errors: 0
        }),
        "first tracked domain keeps exact stats"
    );
}

/// REQ-05: an already-tracked domain keeps aggregating under its own key
/// after the cap is reached (`contains_key` takes precedence); only NEW
/// domains past the cap land in `"otros"`.
#[test]
fn existing_domain_aggregates_after_cap_reached() {
    let mut m = ScrapeMetrics::default();
    for i in 0..500usize {
        m.record(ScrapeEvent::new(
            "scrape_url",
            format!("d{i:04}.com"),
            Outcome::Success,
            1,
            Duration::from_millis(1),
        ));
    }
    let snap = m.snapshot();
    assert_eq!(snap.domains.len(), 500, "cap not yet exceeded");
    assert!(!snap.domains.contains_key("otros"), "no overflow yet");

    // Domain #1 (already tracked) still aggregates under its own key.
    m.record(ScrapeEvent::new(
        "scrape_url",
        "d0000.com".to_string(),
        Outcome::Error,
        3,
        Duration::from_millis(1),
    ));
    let snap = m.snapshot();
    assert_eq!(snap.domains.len(), 500, "re-record adds no key");
    assert_eq!(
        snap.domains.get("d0000.com"),
        Some(&DomainStats {
            events: 2,
            pages: 4,
            errors: 1
        }),
        "existing domain keeps aggregating under its own key"
    );

    // A brand-new domain #502 lands in the overflow bucket.
    m.record(ScrapeEvent::new(
        "scrape_url",
        "newdomain.com".to_string(),
        Outcome::Error,
        7,
        Duration::from_millis(1),
    ));
    let snap = m.snapshot();
    assert_eq!(snap.domains.len(), 501, "cap + overflow bucket");
    assert!(
        !snap.domains.contains_key("newdomain.com"),
        "new domain past the cap must not get its own key"
    );
    assert_eq!(
        snap.domains.get("otros"),
        Some(&DomainStats {
            events: 1,
            pages: 7,
            errors: 1
        }),
        "first overflow event lands in otros"
    );

    // A second new domain keeps merging into the same bucket.
    m.record(ScrapeEvent::new(
        "scrape_url",
        "anothernew.com".to_string(),
        Outcome::Success,
        2,
        Duration::from_millis(1),
    ));
    assert_eq!(
        m.snapshot().domains.get("otros"),
        Some(&DomainStats {
            events: 2,
            pages: 9,
            errors: 1
        }),
        "overflow keeps aggregating"
    );
}

/// REQ-01: an unparseable domain is recorded as `"unknown"` and still counted.
#[test]
fn record_unknown_domain() {
    assert_eq!(domain_of("not a url"), "unknown", "unparseable → unknown");
    assert_eq!(
        domain_of("https://example.com/path?q=1"),
        "example.com",
        "parseable → host_str"
    );

    let mut m = ScrapeMetrics::default();
    m.record(ScrapeEvent::new(
        "scrape_url",
        domain_of("not a url"),
        Outcome::Success,
        1,
        Duration::from_millis(50),
    ));

    let snap = m.snapshot();
    assert_eq!(snap.total_events, 1, "unknown-domain event still counted");
    assert_eq!(
        snap.domains.get("unknown"),
        Some(&DomainStats {
            events: 1,
            pages: 1,
            errors: 0
        }),
        "unknown bucket present"
    );
}

/// REQ-07: 100 concurrent records lose nothing (no lost updates/deadlock).
#[tokio::test]
async fn concurrent_records_lose_nothing() {
    let metrics = Arc::new(Mutex::new(ScrapeMetrics::default()));
    let mut handles = Vec::with_capacity(100);
    for _ in 0..100 {
        let m = Arc::clone(&metrics);
        handles.push(tokio::spawn(async move {
            m.lock().expect("metrics mutex").record(ScrapeEvent::new(
                "scrape_url",
                "a.com".to_string(),
                Outcome::Success,
                1,
                Duration::from_millis(1),
            ));
        }));
    }
    for h in handles {
        h.await.expect("task must not panic");
    }

    let snap = metrics.lock().expect("metrics mutex").snapshot();
    assert_eq!(snap.total_events, 100, "no lost updates under concurrency");
}
