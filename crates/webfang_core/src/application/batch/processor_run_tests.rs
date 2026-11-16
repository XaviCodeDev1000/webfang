/// #1615 (F11 / G-3 / G-4 / G-5): these tests drive real fetches against
/// wiremock's 127.0.0.1, which the literal-IP entry guard now refuses.
/// `EnvGuard::entry_guard_off()` disarms the guard for its lifetime — the
/// named constructor is the repo's rule, so each call site stays one line.
fn entry_off() -> webfang_test_utils::EnvGuard {
    webfang_test_utils::EnvGuard::entry_guard_off()
}

use super::*;
use crate::application::batch::BatchJob;
use crate::domain::CrawlerConfig;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use url::Url;

// =====================================================================
// Bug R2-1: BudgetOverrides must survive the per-URL config rebuild
// =====================================================================

#[test]
fn per_url_config_carries_budget_overrides() {
    // The batch path rebuilds a fresh CrawlerConfig for every URL; the
    // operator overrides staged on the base config must ride along or the
    // Engine silently re-derives the auto tiers.
    let overrides = crate::domain::budget::BudgetOverrides {
        crawl: crate::domain::budget::tiers::CrawlConcurrency::new(3).ok(),
        rate_burst: crate::domain::budget::tiers::BurstPermits::new(7).ok(),
        ..crate::domain::budget::BudgetOverrides::default()
    };
    let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
        .budget_overrides(overrides)
        .build();

    let per_url =
        build_per_url_config("https://example.org/seed", &base).expect("valid per-URL seed");

    assert_eq!(
        per_url.budget_overrides.crawl.map(|c| c.get()),
        Some(3),
        "explicit --concurrency must survive the per-URL rebuild"
    );
    assert_eq!(
        per_url.budget_overrides.rate_burst.map(|b| b.get()),
        Some(7),
        "explicit --rate-limit-burst must survive the per-URL rebuild"
    );
}

#[test]
fn per_url_config_carries_jitter_ms() {
    // Same class as the budget overrides above: a knob staged on the
    // base config must survive the per-URL rebuild, or the per-URL
    // engine builds a limiter WITHOUT the jitter (#2610).
    let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
        .jitter_ms(250)
        .build();

    let per_url =
        build_per_url_config("https://example.org/seed", &base).expect("valid per-URL seed");

    assert_eq!(
        per_url.jitter_ms, 250,
        "the configured jitter must survive the per-URL rebuild"
    );
}

#[test]
fn per_url_config_forces_seed_only_scrape() {
    // #1215 (F-38): the batch path rebuilds a fresh CrawlerConfig for
    // every URL, and the rebuild MUST pin seed-only scope no matter what
    // crawl budget the base config carries — otherwise every seed is
    // BFS-expanded and export records duplicate (F-02).
    let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
        .max_depth(2)
        .max_pages(10)
        .build();

    let per_url =
        build_per_url_config("https://example.org/seed", &base).expect("valid per-URL seed");

    assert_eq!(
        per_url.max_depth, 0,
        "batch per-URL crawl must be seed-only (max_depth 0)"
    );
    assert_eq!(
        per_url.max_pages, 1,
        "batch per-URL crawl must fetch exactly one page (max_pages 1)"
    );
    assert_eq!(
        per_url.seed_url.as_str(),
        "https://example.org/seed",
        "the batch URL becomes the seed"
    );
}

#[test]
fn per_url_config_rejects_invalid_url() {
    let base = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
    let err = build_per_url_config("not a url", &base);
    assert!(matches!(err, Err(CrawlError::InvalidUrl(_))));
}

// =====================================================================
// #1428: the run-scoped limiter is built from the base config, once
// per run — one shared bucket, so `--delay-ms` paces across URLs.
// =====================================================================

#[test]
fn batch_rate_limiter_none_at_zero_delay() {
    // No pacing at all — no delay AND no jitter — means "no pacing": no
    // bucket is allocated and each engine keeps its historical
    // per-engine limiter (zero overhead, zero drift — the
    // `build_scrape_rate_limiter` contract in `scrape_flow.rs`).
    let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
        .delay_ms(0)
        .build();

    assert!(
        build_batch_rate_limiter(&base).is_none(),
        "zero delay must build no bucket"
    );
}

#[test]
fn batch_rate_limiter_some_for_a_jitter_without_a_delay() {
    // A jitter alone IS a pacing request (#2610) — the crawl path
    // already answers that way, so the batch path must not answer
    // "no pacing" for the same input.
    let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
        .delay_ms(0)
        .jitter_ms(500)
        .build();

    assert!(
        build_batch_rate_limiter(&base).is_some(),
        "a jitter without a delay must still build the run-scoped bucket"
    );
}

#[cfg_attr(
    miri,
    ignore = "quanta CPUID detection (raw-cpuid inline asm) unsupported by Miri"
)]
#[test]
fn batch_rate_limiter_some_at_positive_delay() {
    let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
        .delay_ms(400)
        .build();

    assert!(
        build_batch_rate_limiter(&base).is_some(),
        "a positive delay must build the run-scoped bucket"
    );
}

/// #1369 seam pin: every knob of the batch `EngineOptions` must come from
/// an explicit source — the robots value from the base config, the sink
/// from the processor, and the knobs the batch has no source for set to
/// the values the old `crawl_site` entry hardcoded (never a silent
/// default that could drift with `EngineOptions::default()`).
#[test]
fn batch_engine_options_make_every_knob_source_explicit() {
    let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
        .ignore_robots(true)
        .build();
    let options = build_batch_engine_options(&base, None);
    assert!(
        options.ignore_robots,
        "the batch's robots source is the base config"
    );
    assert!(
        options.checkpoint_path.is_none(),
        "batch has no checkpoint source — must stay off explicitly"
    );
    assert!(
        !options.session_pool_enabled,
        "batch has no pool source — must stay off explicitly"
    );
    assert_eq!(
        options.js_strategy,
        JsStrategy::Static,
        "batch has no js-strategy source — must stay static explicitly"
    );
    assert!(
        options.downloader_factory.is_none(),
        "no factory means the recorded strategy cannot render — the
         static batch path must not grow one by accident"
    );
    assert!(
        options.content_sink.is_none(),
        "no sink attached leaves the batch metadata-only (the #631 gotcha)"
    );
    assert!(
        options.rate_limiter.is_none(),
        "the pacer is NOT part of the options bag — `process_batch_cancellable` wires \
             the run-scoped bucket from `build_batch_rate_limiter` onto the built options (#1428)"
    );

    let sink = Arc::new(crate::application::crawler::content_sink::InMemoryContentSink::new());
    let options = build_batch_engine_options(&base, Some(sink));
    assert!(
        options.content_sink.is_some(),
        "the processor's shared sink must ride on the options"
    );

    let enforcing = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
    assert!(
        !build_batch_engine_options(&enforcing, None).ignore_robots,
        "the default config's respect-robots must flow through both ways"
    );
}

/// #1375 drift guard: the transport scalars the batch does NOT source
/// explicitly ride `..Default::default()` in
/// [`build_batch_engine_options`]. That equivalence was audited against
/// the old hardcoded `crawl_site_inner` policy when #1369 migrated the
/// batch; this pin re-derives the same values from independent literals
/// (never from `EngineOptions::default()` itself), so a future evolution
/// of the default breaks THIS test with a named knob instead of silently
/// re-policing every `--batch` transport knob.
///
/// If this test fails after an intentional default change, the batch
/// policy must be re-audited and the new values pinned here — the fix is
/// never to delete the pin (that reintroduces silent drift, #1375).
#[test]
fn batch_engine_options_transport_scalars_pin_the_audited_policy() {
    let base = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
    let options = build_batch_engine_options(&base, None);

    // Expected values are literals copied from the audited
    // `crawl_site_inner` TransportPolicy (engine.rs #1369 migration),
    // NOT reads of `EngineOptions::default()`.
    assert_eq!(
        options.checkpoint_interval, 100,
        "checkpoint interval the old entry hardcoded"
    );
    assert_eq!(
        options.tls_emulation,
        wreq_util::Profile::Chrome145,
        "TLS fingerprint the old entry hardcoded"
    );
    assert!(!options.ignore_waf, "WAF bypass stays off in batch");
    assert_eq!(
        options.max_retries, 3,
        "retry ceiling the old entry hardcoded"
    );
    assert_eq!(
        options.backoff_base_ms, 1000,
        "backoff base the old entry hardcoded"
    );
    assert_eq!(
        options.backoff_max_ms, 10_000,
        "backoff ceiling the old entry hardcoded"
    );
    assert_eq!(
        options.obscura_binary, "obscura",
        "hybrid layer-2 binary default the old entry hardcoded"
    );
    assert_eq!(
        options.post_load_wait,
        crate::domain::PostLoadWait::Idle,
        "chromium settlement default the old entry hardcoded"
    );
    assert!(
        !options.autoscale_enabled,
        "RAM autoscaling stays off in batch"
    );
}

/// Shared in-flight gauge + six-node star topology (seed + 5 leaves) for
/// the R2-1 diagnostics: counts every request and the high-water mark of
/// concurrent responses so the scheduler bound derived from the operator
/// override is observable end to end.
struct SixNodeGauge {
    inflight: Arc<AtomicUsize>,
    max_inflight: Arc<AtomicUsize>,
    total_requests: Arc<AtomicUsize>,
    seed_uri: String,
}

impl Clone for SixNodeGauge {
    fn clone(&self) -> Self {
        Self {
            inflight: Arc::clone(&self.inflight),
            max_inflight: Arc::clone(&self.max_inflight),
            total_requests: Arc::clone(&self.total_requests),
            seed_uri: self.seed_uri.clone(),
        }
    }
}

impl wiremock::Respond for SixNodeGauge {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let current = self.inflight.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        self.max_inflight.fetch_max(current, AtomicOrdering::SeqCst);
        self.total_requests.fetch_add(1, AtomicOrdering::SeqCst);
        // Force overlap when the scheduler bound allows parallel fetches,
        // so an over-broad bound is actually observed by the high-water mark.
        std::thread::sleep(std::time::Duration::from_millis(30));
        self.inflight.fetch_sub(1, AtomicOrdering::SeqCst);
        if request.url.path() == "/" {
            let links: String = (0..5)
                .map(|i| format!(r#"<a href="{}/p{i}">n{i}</a>"#, self.seed_uri))
                .collect();
            wiremock::ResponseTemplate::new(200)
                .set_body_string(format!("<html><body>{links}</body></html>"))
        } else {
            wiremock::ResponseTemplate::new(200).set_body_string("<html><body>leaf</body></html>")
        }
    }
}

/// Six-node diagnostic (bug R2-1, scope pinned by #1215): with `crawl = 1`
/// staged as an operator override on the batch base config, the Engine
/// reached through `BatchProcessor.process_single_url ->
/// crawl_site_with_options`
/// must still honor the explicit `--concurrency` override — and, since
/// #1215, it scrapes the seed ONLY: the base config's `max_depth 1` /
/// `max_pages 10` must NOT expand into the 5 discovered leaves, even
/// though the same gauge topology would expose such an expansion.
#[cfg(not(miri))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_mode_scrapes_seed_only_with_concurrency_override() {
    let _entry_off = entry_off();
    use crate::domain::budget::{
        tiers::{BurstPermits, CrawlConcurrency},
        BudgetOverrides,
    };

    let server = wiremock::MockServer::start().await;
    let gauge = SixNodeGauge {
        inflight: Arc::new(AtomicUsize::new(0)),
        max_inflight: Arc::new(AtomicUsize::new(0)),
        total_requests: Arc::new(AtomicUsize::new(0)),
        seed_uri: server.uri(),
    };
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(gauge.clone())
        .mount(&server)
        .await;

    let seed = Url::parse(&format!("{}/", server.uri())).unwrap();
    let base_config = CrawlerConfig::builder(seed)
        .max_depth(1)
        .max_pages(10)
        .concurrency(std::num::NonZeroUsize::new(16).expect("16 is non-zero")) // configured value must be beaten by the override
        .timeout_secs(5)
        .ignore_robots(true)
        .budget_overrides(BudgetOverrides {
            crawl: CrawlConcurrency::new(1).ok(),
            rate_burst: BurstPermits::new(4).ok(),
            ..BudgetOverrides::default()
        })
        .build();

    let job = BatchJob::new(
        "r2-1-six-node".to_string(),
        vec![format!("{}/", server.uri())],
        base_config,
    );
    let processor = BatchProcessor::new(2).unwrap();
    let result = processor.process_batch(job).await.unwrap();

    assert_eq!(result.succeeded, 1, "the single batch URL must succeed");
    assert_eq!(
        gauge.total_requests.load(AtomicOrdering::SeqCst),
        1,
        "#1215: batch scrapes the seed only — the 5 discovered leaves must NOT be crawled"
    );
    assert_eq!(
        gauge.max_inflight.load(AtomicOrdering::SeqCst),
        1,
        "override crawl=1 must cap concurrent fetches at 1 through batch mode"
    );
}
