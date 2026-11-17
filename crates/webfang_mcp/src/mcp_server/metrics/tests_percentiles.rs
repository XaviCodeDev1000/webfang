use super::*;

// ---------------------------------------------------------------------
// OBS-M4 — duration percentiles (bounded, deterministic, reconstructible)
// ---------------------------------------------------------------------

/// OBS-M4: recording the known distribution `1..=100` ms makes the
/// percentiles *reconstructible*, which is the whole point — a mean
/// (`average_duration_ms == 50.5`) cannot answer any percentile question.
///
/// Nearest-rank, 1-indexed: `rank = ceil(p * n / 100)`, clamped to
/// `[1, n]`. For `n = 100` the ranks are 50 / 95 / 99, so the answers are
/// the 50th / 95th / 99th smallest sample — exact, zero tolerance.
#[test]
fn percentiles_reconstruct_a_known_distribution() {
    let mut m = ScrapeMetrics::default();
    for ms in 1..=100u64 {
        m.record(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            1,
            Duration::from_millis(ms),
        ));
    }

    let snap = m.snapshot();
    assert_eq!(
        snap.average_duration_ms, 50.5,
        "the mean is what OBS-M4 says cannot answer SLO questions"
    );
    assert_eq!(snap.duration_percentiles.p50_ms, Some(50), "p50");
    assert_eq!(snap.duration_percentiles.p95_ms, Some(95), "p95");
    assert_eq!(snap.duration_percentiles.p99_ms, Some(99), "p99");
    assert_eq!(snap.duration_percentiles.samples, 100, "all samples kept");
    assert_eq!(snap.duration_percentiles.observed, 100, "observed total");
    assert_eq!(snap.duration_percentiles.capacity, 1024, "documented cap");
    assert!(
        !snap.duration_percentiles.truncated,
        "100 samples are far below the cap, nothing is evicted"
    );
}

/// OBS-M4: the reservoir BOUNDS memory. Recording ten times the cap must
/// leave exactly `capacity` samples retained, `observed` counting every
/// event, and `truncated` telling the reader that the percentiles
/// describe the RECENT window rather than the whole process lifetime.
#[test]
fn duration_reservoir_is_bounded() {
    let mut m = ScrapeMetrics::default();
    // Cap-worth of ascending samples, then far more of a constant value
    // so the retained window's contents are unambiguous.
    for ms in 0..DURATION_SAMPLE_CAPACITY as u64 {
        m.record(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            1,
            Duration::from_millis(ms),
        ));
    }
    for _ in 0..(DURATION_SAMPLE_CAPACITY * 10) {
        m.record(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            1,
            Duration::from_millis(999),
        ));
    }

    let snap = m.snapshot();
    let p = snap.duration_percentiles;
    assert_eq!(p.samples, DURATION_SAMPLE_CAPACITY, "retained is capped");
    assert_eq!(
        p.observed,
        DURATION_SAMPLE_CAPACITY * 11,
        "observed counts every recorded event, not just the retained ones"
    );
    assert!(p.truncated, "an evicted sample must be declared");
    assert_eq!(
        p.p50_ms,
        Some(999),
        "the retained window is the LAST cap samples (all 999 ms)"
    );
    assert_eq!(p.p95_ms, Some(999), "p95 of the retained window");
    assert_eq!(p.p99_ms, Some(999), "p99 of the retained window");
}

/// OBS-M4: the ring keeps the MOST RECENT `capacity` samples, in order,
/// and the same recorded sequence always yields the same snapshot — no
/// wall clock, no RNG, no map-iteration order anywhere in the path.
#[test]
fn duration_reservoir_is_a_deterministic_last_n_ring() {
    let reservoir = || {
        let mut m = ScrapeMetrics::default();
        for ms in 1..=10u64 {
            m.record(ScrapeEvent::new(
                "scrape_url",
                "a.com".to_string(),
                Outcome::Success,
                1,
                Duration::from_millis(ms),
            ));
        }
        m.snapshot().duration_percentiles
    };
    assert_eq!(
        reservoir(),
        reservoir(),
        "the same event sequence must yield a byte-identical percentile set"
    );

    // Ring eviction: overwrite a 4-slot reservoir with 1..=6 and assert the
    // retained window is 3,4,5,6 — the last four, not the first four.
    let mut r = DurationReservoir::with_capacity(4);
    for ms in 1..=6u64 {
        r.record(ms);
    }
    let p = r.percentiles();
    assert_eq!(p.samples, 4, "ring never exceeds its capacity");
    assert_eq!(p.observed, 6, "observed is the lifetime total");
    assert!(p.truncated, "two samples were evicted");
    assert_eq!(p.p50_ms, Some(4), "median of {{3,4,5,6}} is the 2nd = 4");
    assert_eq!(p.p95_ms, Some(6), "p95 of {{3,4,5,6}} is the 4th = 6");
    assert_eq!(p.p99_ms, Some(6), "p99 of {{3,4,5,6}} is the 4th = 6");
}

/// OBS-M4: an empty accumulator reports `None` percentiles, never a
/// fabricated `0.0` — "no samples" and "every sample was instantaneous"
/// must not serialize the same way.
#[test]
fn percentiles_are_none_without_samples() {
    let snap = ScrapeMetrics::default().snapshot();
    assert_eq!(snap.duration_percentiles.p50_ms, None, "p50");
    assert_eq!(snap.duration_percentiles.p95_ms, None, "p95");
    assert_eq!(snap.duration_percentiles.p99_ms, None, "p99");
    assert_eq!(snap.duration_percentiles.samples, 0, "samples");
    assert!(!snap.duration_percentiles.truncated, "nothing evicted");
}
