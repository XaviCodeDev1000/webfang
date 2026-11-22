use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use url::Url;
use wiremock::matchers::method;
use wiremock::{Mock, Request, Respond, ResponseTemplate};

use super::{build_scrape_rate_limiter, scrape_urls};
use crate::application::crawl_options::CrawlOptions;

// ===== scrape-phase rate limiting (#P4-4) =====

/// Server-side arrival recorder.
///
/// `wiremock::Request` (0.6.5) carries no timestamp, so `received_requests()`
/// cannot answer "when did this arrive?". `Respond::respond` runs inside the
/// mock server's request handler BEFORE the template's `set_delay` is awaited
/// (`mock_server/hyper.rs:34-51`), so the instant recorded here IS the
/// server-side arrival time the spacing assertion needs.
#[derive(Clone)]
struct Arrivals {
    template: ResponseTemplate,
    seen: Arc<Mutex<Vec<Instant>>>,
}

impl Respond for Arrivals {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Instant::now());
        self.template.clone()
    }
}

/// Body rich enough for the extractor to return content rather than an
/// `ExtractionFailed` error, so the assertions measure cadence only.
const RATE_LIMIT_PAGE_HTML: &str = "<html><head><title>Rate limit probe</title></head>\
    <body><main><article><h1>Rate limit probe</h1>\
    <p>Substantive article text, long enough for the content extractor to\
    consider this a real page rather than an empty shell document.</p>\
    <p>A second paragraph of substantive prose keeps the quality score above\
    the extraction floor used by the pipeline.</p>\
    </article></main></body></html>";

/// #P4-4: `--delay-ms` must gate the SCRAPE path, not only discovery.
///
/// Before the fix the flag reached the crawl Engine's token bucket and
/// nothing else: every scrape-phase fetch ran back-to-back. The two seeds
/// below therefore arrive ~50 ms apart (mock latency only) when the wiring
/// regresses and ~400 ms apart when it holds.
#[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
#[tokio::test]
async fn scrape_phase_refetch_respects_rate_limit() {
    // The seeds are wiremock loopback literals, which the SSRF entry guard
    // rejects in production — same allowance the robots-blocked test uses.
    let _guard = webfang_test_utils::EnvGuard::with(&[(
        crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    )]);

    // 400 ms token period against a 50 ms mock latency: the two numbers are
    // deliberately far apart so a spacing inside the mock's own cost proves
    // the limiter never gated the fetch.
    const DELAY_MS: u64 = 400;
    const MOCK_LATENCY_MS: u64 = 50;

    let server = wiremock::MockServer::start().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("GET"))
        .respond_with(Arrivals {
            template: ResponseTemplate::new(200)
                .set_body_string(RATE_LIMIT_PAGE_HTML)
                .set_delay(Duration::from_millis(MOCK_LATENCY_MS)),
            seen: seen.clone(),
        })
        // Exactly two arrivals: one per seed, nothing else.
        .expect(2)
        .mount(&server)
        .await;

    let urls: Vec<Url> = (0..2)
        .map(|i| Url::parse(&format!("{}/page/{i}", server.uri())).expect("valid URL"))
        .collect();

    let mut opts = CrawlOptions::default();
    opts.network.delay_ms = DELAY_MS;
    // Burst 1 is what makes the wait observable: the derived default (≈ 8 on
    // this host) grants both seeds immediately and masks the period entirely.
    opts.budget_overrides.rate_burst =
        Some(crate::domain::budget::BurstPermits::new(1).expect("burst 1 is valid"));
    // robots.txt would add a request per domain; ignoring it keeps the mock
    // at exactly two arrivals so the measurement is page-fetch only.
    opts.crawl.ignore_robots = true;

    let (results, failures, blocked) = scrape_urls(
        &urls,
        &crate::domain::config::ScraperConfig::default(),
        &opts,
        &crate::application::progress_observer::NoopObserver,
        None,
        None,
        &crate::domain::CorrelationId::new(),
        &CancellationToken::new(),
        &[],
    )
    .await
    .expect("setup must succeed");

    assert_eq!(results.len(), 2, "both seeds must be scraped");
    assert!(failures.is_empty(), "no seed may fail, got: {failures:?}");
    assert_eq!(blocked, 0, "nothing may be robots-blocked here");

    let arrivals = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(arrivals.len(), 2, "one arrival per seed");
    let spacing = arrivals[1] - arrivals[0];

    // The brief's literal bound: the gap must exceed twice the mock's own
    // latency, so it cannot be explained by the response delay.
    assert!(
        spacing >= Duration::from_millis(2 * MOCK_LATENCY_MS),
        "arrival spacing {spacing:?} is within the mock's own latency — the limiter did not gate the scrape path"
    );
    // The real invariant: a burst-1 bucket refills one permit per DELAY_MS,
    // so consecutive arrivals sit at least three quarters of a period apart.
    // Not the full period: a shared CI runner can absorb that much scheduling
    // slack between the grant and the arrival, and a false red here would be
    // worse than a slightly looser bound.
    assert!(
        spacing >= Duration::from_millis(DELAY_MS * 3 / 4),
        "arrival spacing {spacing:?} must be >= 0.75x the {DELAY_MS}ms token period"
    );
}

/// No pacing at all — no delay AND no jitter — must build NO bucket:
/// not a bucket with a 1 ms floor. That is what keeps an unthrottled run
/// free of any added await.
#[cfg_attr(
    miri,
    ignore = "quanta CPUID detection (raw-cpuid inline asm) unsupported by Miri"
)]
#[test]
fn scrape_rate_limiter_is_built_only_when_pacing_was_asked_for() {
    let opts = CrawlOptions::default();
    assert!(
        build_scrape_rate_limiter(&opts).is_some(),
        "the default positive --delay-ms must gate the scrape path"
    );

    let mut opts = CrawlOptions::default();
    opts.network.delay_ms = 0;
    assert!(
        build_scrape_rate_limiter(&opts).is_none(),
        "--delay-ms 0 with no jitter must build no bucket and add no await"
    );

    // A jitter alone IS a pacing request (#2610) — and the crawl path
    // already answers that way (it clamps the base delay to 1 ms), so
    // the scrape path must not answer "no pacing" for the same input.
    opts.network.jitter_ms = 500;
    assert!(
        build_scrape_rate_limiter(&opts).is_some(),
        "a jitter without a delay must still build the pacing bucket"
    );
}
