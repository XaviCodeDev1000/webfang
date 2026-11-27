#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! S15 steps 3-5 — challenge rate, open-loop latency and Tier B exclusion.
//!
//! All signals are domain values from `webfang_core`; all timelines are
//! injected, so two consecutive runs on the same fixture are identical.
//!
//! Run: cargo nextest run -p webfang_benchmark --test measurements_test

use webfang_benchmark::aggregate::{compute, CrawlSummary, TraceRecord};
use webfang_benchmark::cost::config::CostConfig;
use webfang_benchmark::measurements::{
    challenge_rate, from_spans, open_loop_percentiles, schedule, AttemptSignal,
};
use webfang_benchmark::report;
use webfang_benchmark::BenchmarkError;
use webfang_core::domain::downloader_port::DownloadError;
use webfang_core::domain::waf::{EvidenceSource, WafEvidence, WafTier, WafVerdict};
use webfang_core::error::ScraperError;

fn challenge_verdict() -> WafVerdict {
    WafVerdict {
        is_blocked: true,
        evidences: vec![WafEvidence {
            provider: "Cloudflare",
            tier: WafTier::Challenge,
            matched_pattern: "cf-turnstile",
            source: EvidenceSource::Body,
        }],
    }
}

fn fingerprint_verdict() -> WafVerdict {
    WafVerdict {
        is_blocked: true,
        evidences: vec![WafEvidence {
            provider: "Akamai",
            tier: WafTier::Fingerprint,
            matched_pattern: "akamai",
            source: EvidenceSource::Header,
        }],
    }
}

fn one_run() -> Vec<webfang_benchmark::aggregate::StrategyMetrics> {
    let summary = CrawlSummary {
        total_pages: 4,
        succeeded: 3,
        errors: 1,
        errors_waf: 1,
        errors_http: 0,
        errors_timeout: 0,
        errors_network: 0,
        errors_rate_limit: 0,
        errors_extraction: 0,
        errors_internal: 0,
        errors_panic: 0,
        duration_secs: 1.0,
        pages_per_sec: 4.0,
        trace_id: None,
    };
    let mut records = vec![TraceRecord::Summary(summary)];
    records.extend([10.0, 20.0, 30.0, 40.0].map(|d| TraceRecord::SpanClose { duration_ms: d }));
    vec![compute(&records, webfang_core::domain::JsStrategy::Static, 1024).expect("metrics")]
}

/// S15 step 3 — every one of the three WAF signal forms counts as one
/// challenge attempt; the denominator is ALL attempts, not just challenges.
#[test]
fn challenge_rate_counts_verdict_download_and_scraper_signals() {
    let signals = [
        AttemptSignal::from(&challenge_verdict()),
        AttemptSignal::from(&DownloadError::WafChallenge("captcha".to_string())),
        AttemptSignal::from(&ScraperError::waf_blocked(
            "https://example.test/",
            "Cloudflare",
        )),
        AttemptSignal::Clean,
    ];
    assert!(
        (challenge_rate(&signals) - 0.75).abs() < f64::EPSILON,
        "3 challenges out of 4 attempts must be 0.75, got {}",
        challenge_rate(&signals)
    );
}

/// Fingerprint-only verdicts and non-WAF errors are not challenge events.
#[test]
fn non_challenge_signals_do_not_inflate_the_rate() {
    let signals = [
        AttemptSignal::from(&fingerprint_verdict()),
        AttemptSignal::from(&DownloadError::timeout(30)),
        AttemptSignal::from(&ScraperError::readability("bad html")),
    ];
    assert_eq!(challenge_rate(&signals), 0.0);
}

/// A rate is a ratio. Presence (`> 0`) is not a rate; empty input is 0.0.
#[test]
fn challenge_rate_is_a_ratio_not_a_presence_flag() {
    let signals = [
        AttemptSignal::from(&DownloadError::WafChallenge("captcha".to_string())),
        AttemptSignal::Clean,
        AttemptSignal::Clean,
        AttemptSignal::Clean,
    ];
    assert!((challenge_rate(&signals) - 0.25).abs() < f64::EPSILON);
    assert_eq!(challenge_rate(&[]), 0.0);
}

/// S15 step 4 — the target rate fixes the schedule; no clock is read.
#[test]
fn schedule_is_fixed_rate_and_deterministic() {
    let first = schedule(4.0, 3, 100.0).expect("schedule");
    let second = schedule(4.0, 3, 100.0).expect("schedule");
    assert_eq!(first, vec![100.0, 350.0, 600.0]);
    assert_eq!(first, second, "same input must produce the same timeline");
}

#[test]
fn non_positive_target_rate_is_a_typed_error() {
    assert!(matches!(
        schedule(0.0, 3, 0.0),
        Err(BenchmarkError::Schedule(_))
    ));
}

/// Percentiles are computed from observed MINUS scheduled timestamps.
#[test]
fn open_loop_latency_uses_the_scheduled_timeline() {
    let scheduled = [0.0, 250.0, 500.0, 750.0];
    let observed = [10.0, 270.0, 530.0, 790.0];
    let latencies = open_loop_percentiles(&scheduled, &observed).expect("percentiles");
    assert_eq!(latencies.p50_ms, 20.0);
    assert_eq!(latencies.p95_ms, 40.0);
    assert_eq!(latencies.samples, 4);
}

#[test]
fn mismatched_timelines_are_a_typed_error() {
    assert!(matches!(
        open_loop_percentiles(&[0.0, 250.0], &[10.0]),
        Err(BenchmarkError::Schedule(_))
    ));
}

/// Two consecutive runs over the same span fixture produce identical numbers.
#[test]
fn open_loop_from_spans_is_byte_reproducible() {
    let spans = [10.0, 20.0, 30.0, 40.0];
    let first = from_spans(4.0, &spans).expect("first run");
    let second = from_spans(4.0, &spans).expect("second run");
    assert_eq!(first, second);
}

/// S15 step 3 as surfaced by the harness: a run's challenge rate is a ratio,
/// not the boolean `errors_waf > 0`.
#[test]
fn run_metrics_report_challenge_rate_as_a_ratio() {
    let metrics = &one_run()[0];
    assert!(
        (metrics.challenge_rate - 0.25).abs() < f64::EPSILON,
        "1 challenge over 4 attempts must be 0.25, got {}",
        metrics.challenge_rate
    );
}

/// S15 step 5 — Tier B competitor adapters and corpus hosts never appear as
/// published result rows; only the honest Tier A label does.
#[test]
fn tier_b_competitors_are_excluded_from_published_numbers() {
    let out = report::render(&one_run(), &CostConfig::default()).expect("report");
    let results = out
        .split("## Results (deterministic counts)")
        .nth(1)
        .expect("deterministic results table");
    for name in [
        "firecrawl",
        "crawl4ai",
        "scrapeme.dev",
        "developers.cloudflare.com",
    ] {
        assert!(
            !results.contains(name),
            "Tier B name `{name}` leaked into published results"
        );
    }
    assert!(
        out.contains("Tier B competitor adapters are excluded from published numbers"),
        "methodology must state the Tier B exclusion"
    );
}
