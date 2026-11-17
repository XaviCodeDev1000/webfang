use super::*;

// ---------------------------------------------------------------------
// OBS-M6 — tool x domain cross-product
// ---------------------------------------------------------------------

/// OBS-M6: the cross-product is EXACT for a fixture small enough that no
/// cap can hide a mistake, which is what makes it answerable as
/// "which tool is slow for this domain".
#[test]
fn tool_domain_cross_product_is_exact() {
    let mut m = ScrapeMetrics::default();
    m.record(ScrapeEvent::new(
        "scrape_url",
        "a.com".to_string(),
        Outcome::Success,
        2,
        Duration::from_millis(10),
    ));
    m.record(ScrapeEvent::new(
        "scrape_url",
        "a.com".to_string(),
        Outcome::Error,
        0,
        Duration::from_millis(30),
    ));
    m.record(ScrapeEvent::new(
        "crawl_site",
        "a.com".to_string(),
        Outcome::Success,
        7,
        Duration::from_millis(100),
    ));
    m.record(ScrapeEvent::new(
        "scrape_url",
        "b.com".to_string(),
        Outcome::Success,
        1,
        Duration::from_millis(50),
    ));

    let snap = m.snapshot();
    assert_eq!(
        snap.tool_domains.keys().collect::<Vec<_>>(),
        vec!["crawl_site", "scrape_url"],
        "outer keys are tools, sorted"
    );
    let scrape_a = &snap.tool_domains["scrape_url"]["a.com"];
    assert_eq!(scrape_a.events, 2, "two scrape_url events on a.com");
    assert_eq!(scrape_a.pages, 2, "pages accumulate per pair");
    assert_eq!(scrape_a.errors, 1, "the Error event counts as an error");
    assert_eq!(
        scrape_a.duration_percentiles.p50_ms,
        Some(10),
        "median of {{10,30}} is the 1st nearest-rank sample"
    );
    assert_eq!(
        scrape_a.duration_percentiles.p95_ms,
        Some(30),
        "p95 of {{10,30}} is the 2nd"
    );
    assert_eq!(snap.tool_domains["crawl_site"]["a.com"].pages, 7, "crawl");
    assert_eq!(snap.tool_domains["scrape_url"]["b.com"].events, 1, "b.com");

    // Cross-check: the pairs re-sum to the flat totals already published.
    let events: usize = snap
        .tool_domains
        .values()
        .flat_map(|per_tool| per_tool.values())
        .map(|cell| cell.events)
        .sum();
    assert_eq!(events, snap.total_events, "pairs partition the events");
    let per_tool_pairs = snap.tool_domains["scrape_url"].len();
    assert_eq!(per_tool_pairs, 2, "scrape_url hit two domains");
}

/// OBS-M6: the cross-product NEVER invents a domain key the per-domain map
/// does not already show. Both axes resolve through the same
/// `MAX_TRACKED_DOMAINS` rule, so a domain past the cap is visible in
/// `domains` only as `"otros"` and here only as `(tool, "otros")` — a
/// reader who cannot see a domain in one map can never see it in the other.
#[test]
fn tool_domains_respect_the_per_domain_cap() {
    let mut m = ScrapeMetrics::default();
    for i in 0..(MAX_TRACKED_DOMAINS + 10) {
        m.record(ScrapeEvent::new(
            "scrape_url",
            format!("d{i:04}.com"),
            Outcome::Success,
            1,
            Duration::from_millis(1),
        ));
    }

    let snap = m.snapshot();
    assert!(
        !snap.tool_domains["scrape_url"].contains_key("d0500.com"),
        "a domain past the per-domain cap must not get its own cross-product cell"
    );
    assert_eq!(
        snap.tool_domains["scrape_url"][OVERFLOW_DOMAIN_KEY].events, 10,
        "the overflow domain reuses the documented bucket key"
    );
    assert_eq!(
        snap.tool_domains["scrape_url"][OVERFLOW_DOMAIN_KEY].events,
        snap.domains[OVERFLOW_DOMAIN_KEY].events,
        "overflow events are the same events in both maps"
    );
}

/// OBS-M6: the cross-product's OWN bound, and the fact that it is a
/// SECOND, INDEPENDENT cap. Writing this test is what surfaced why: with
/// one tool the per-domain cap (500) binds first and the pair cap is
/// unreachable, so a single-tool fixture "proves" nothing about the
/// cross-product's own bound. Two tools over 300 shared domains produce
/// 600 pairs — under the 500-domain cap, over the 512-pair cap — which is
/// the only shape where the two caps are distinguishable at all.
#[test]
fn tool_domain_pairs_are_capped_with_otros_overflow() {
    const DOMAINS: usize = 300;
    let mut m = ScrapeMetrics::default();
    for i in 0..DOMAINS {
        m.record(ScrapeEvent::new(
            "scrape_url",
            format!("p{i:05}.com"),
            Outcome::Success,
            1,
            Duration::from_millis(1),
        ));
    }
    for i in 0..DOMAINS {
        m.record(ScrapeEvent::new(
            "crawl_site",
            format!("p{i:05}.com"),
            Outcome::Success,
            1,
            Duration::from_millis(1),
        ));
    }

    let snap = m.snapshot();
    assert_eq!(
        snap.domains.len(),
        DOMAINS,
        "the per-domain cap is NOT reached: every domain keeps its own key"
    );
    assert!(
        !snap.tool_domains["scrape_url"].contains_key(OVERFLOW_DOMAIN_KEY),
        "the tool that filled the tracked cells needs no overflow bucket"
    );

    let crawl = &snap.tool_domains["crawl_site"];
    assert!(
        crawl.contains_key(OVERFLOW_DOMAIN_KEY),
        "the pair past the cap folds into the documented bucket"
    );
    let total_cells: usize = snap.tool_domains.values().map(BTreeMap::len).sum();
    assert_eq!(
        total_cells,
        MAX_TRACKED_TOOL_DOMAIN_PAIRS + 1,
        "the tracked cells plus exactly one overflow bucket"
    );
    assert_eq!(
        crawl[OVERFLOW_DOMAIN_KEY].events,
        DOMAINS * 2 - MAX_TRACKED_TOOL_DOMAIN_PAIRS,
        "600 pairs - 512 tracked = 88 events in the overflow bucket"
    );
    assert_eq!(
        snap.tool_domains
            .values()
            .flat_map(|per_tool| per_tool.values())
            .map(|cell| cell.events)
            .sum::<usize>(),
        snap.total_events,
        "no event is lost by the cap"
    );
}

/// OBS-M6 + the accepted `MAX_TRACKED_DOMAINS` residual: a per-tool
/// reservoir is capped too, so the cross-product cannot grow the process
/// footprint with `pairs * capacity` samples without bound.
#[test]
fn tool_domain_reservoirs_are_capped_independently() {
    let mut m = ScrapeMetrics::default();
    for ms in 0..(TOOL_DOMAIN_DURATION_CAPACITY * 5) as u64 {
        m.record(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            1,
            Duration::from_millis(ms),
        ));
    }
    let cell = &m.snapshot().tool_domains["scrape_url"]["a.com"];
    assert_eq!(cell.events, TOOL_DOMAIN_DURATION_CAPACITY * 5, "counters");
    assert_eq!(
        cell.duration_percentiles.samples, TOOL_DOMAIN_DURATION_CAPACITY,
        "per-pair reservoir keeps its own smaller cap"
    );
    assert_eq!(
        cell.duration_percentiles.observed,
        TOOL_DOMAIN_DURATION_CAPACITY * 5
    );
    assert!(cell.duration_percentiles.truncated, "eviction is declared");
}
