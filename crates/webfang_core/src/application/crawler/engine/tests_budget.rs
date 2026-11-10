use super::tests::{loop_harness, nz, session_engine, TEST_WATCHDOG};
use super::*;
use crate::domain::budget::detector::FixedDetector;
use crate::domain::budget::tiers::{BurstPermits, CrawlConcurrency};
use crate::domain::budget::{BudgetModel, BudgetOverrides};
use url::Url;

/// Spec scenario (Q1 DECOUPLE): raising the configured crawler
/// concurrency must NOT move the rate-limiter burst — the burst is a
/// budget-model tier derived from the detector seam, independent of
/// `CrawlerConfig.concurrency`.
#[tokio::test]
async fn raising_crawler_concurrency_leaves_rate_limiter_burst_unchanged() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let low = CrawlerConfig::builder(seed.clone())
        .concurrency(nz(1))
        .build();
    let high = CrawlerConfig::builder(seed).concurrency(nz(16)).build();

    let engine_low = session_engine(low);
    let engine_high = session_engine(high);

    assert_eq!(
        engine_low.budget.burst().get(),
        engine_high.budget.burst().get(),
        "burst must not track crawler concurrency"
    );
    assert!(engine_high.budget.burst().get() > 0);
}

/// The RateLimiterConfig construction point derives its burst from the
/// model: with a FixedDetector of 4 cores the auto table yields crawl
/// (= default burst) 3, regardless of any crawler concurrency value.
#[test]
fn rate_limiter_config_burst_comes_from_budget_model() {
    let detector =
        FixedDetector::with_detection(std::num::NonZeroUsize::new(4).expect("non-zero"), None);
    let budget = BudgetModel::build(BudgetOverrides::default(), &detector);
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).concurrency(nz(16)).build();

    let rl_config = rate_limiter_config(&config, &budget);
    assert_eq!(rl_config.delay_ms, config.delay_ms);
    assert_eq!(
        rl_config.concurrency,
        budget.burst().get(),
        "burst must come from the model, not config.concurrency"
    );
    assert_eq!(rl_config.concurrency, 3, "4-core auto table value");
}

/// Triangulation sweep: across the whole 1..=32 core range the
/// construction point emits exactly the model's burst tier, whatever
/// the configured crawler concurrency claims.
#[test]
fn rate_limiter_burst_sweep_matches_model_across_core_counts() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    for cores in 1..=32usize {
        let detector = FixedDetector::with_detection(
            std::num::NonZeroUsize::new(cores).expect("cores non-zero"),
            None,
        );
        let budget = BudgetModel::build(BudgetOverrides::default(), &detector);
        for concurrency in [1usize, 8, 16] {
            let config = CrawlerConfig::builder(seed.clone())
                .concurrency(nz(concurrency))
                .build();
            assert_eq!(
                rate_limiter_config(&config, &budget).concurrency,
                budget.burst().get(),
                "burst diverged from model at cores={cores} concurrency={concurrency}"
            );
        }
    }
}

/// Bug R2-1: an explicit operator override carried on `CrawlerConfig`
/// must reach the Engine — `build_machinery` (consumed by
/// `from_session`) feeds `config.budget_overrides`
/// into `BudgetModel::build`, so the scheduler spawn bound (Operation.crawl)
/// and rate-limiter burst reflect the override instead of the auto table.
#[tokio::test]
async fn session_engine_consumes_config_budget_overrides() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let overrides = BudgetOverrides {
        crawl: CrawlConcurrency::new(7).ok(),
        rate_burst: BurstPermits::new(9).ok(),
        ..BudgetOverrides::default()
    };
    let config = CrawlerConfig::builder(seed)
        .concurrency(nz(1))
        .budget_overrides(overrides)
        .build();

    let engine = session_engine(config);

    assert_eq!(
        engine.budget.crawl().get(),
        7,
        "explicit --concurrency must reach the Engine scheduler bound"
    );
    assert_eq!(
        engine.budget.burst().get(),
        9,
        "explicit --rate-limit-burst must reach the Engine rate limiter"
    );
}

/// Triangulation: with default (no-op) overrides the Engine reproduces
/// exactly the model derived from those same default overrides through
/// the canonical detector seam — the channel changes nothing when unset.
#[tokio::test]
async fn session_engine_default_overrides_keep_auto_derivation() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let expected = BudgetModel::build(
        BudgetOverrides::default(),
        &crate::domain::budget::detector::SystemDetector,
    );
    let config = CrawlerConfig::builder(seed).concurrency(nz(16)).build();

    let engine = session_engine(config);

    assert_eq!(
        engine.budget.crawl().get(),
        expected.crawl().get(),
        "default overrides must keep the auto-derived crawl tier"
    );
    assert_eq!(
        engine.budget.burst().get(),
        expected.burst().get(),
        "default overrides must keep the auto-derived burst tier"
    );
}

/// Remaining-budget edges (issue #1599, strict-TDD RED): the dispatch
/// cap must block spawns exactly when the budget is exhausted while
/// leaving tier-bound dispatch untouched when budget is ample.
#[test]
fn remaining_budget_zero_when_collector_full() {
    assert_eq!(
        Engine::remaining_budget(3, 3, 0),
        0,
        "remaining == 0 spawns nothing"
    );
}

#[test]
fn remaining_budget_zero_when_collected_plus_in_flight_reaches_max() {
    assert_eq!(
        Engine::remaining_budget(3, 2, 1),
        0,
        "collected + in_flight == max_pages blocks dispatch"
    );
}

#[test]
fn remaining_budget_leaves_ample_budget_untouched() {
    assert_eq!(
        Engine::remaining_budget(10, 2, 2),
        6,
        "ample budget must not throttle tier-bound dispatch"
    );
}

#[test]
fn remaining_budget_saturates_without_underflow() {
    assert_eq!(
        Engine::remaining_budget(3, 4, 2),
        0,
        "collected + in_flight > max_pages saturates to zero"
    );
}

/// Break-path parity (issue #1599, strict-TDD RED): with the collector
/// already full before completions are processed, the first
/// (pre-processing) budget-exit path in `crawl_loop` must invoke run
/// cancellation exactly as the second (post-processing) path does — no
/// spawned worker may outlive the budget decision beyond the bounded
/// drain. Pre-fix the first path breaks without `cancel_run`, so this
/// fails; post-fix both paths cancel.
#[tokio::test]
async fn first_budget_exit_break_path_cancels_in_flight_work() {
    use crate::application::crawler::collector::CrawlMessage;
    use crate::domain::DiscoveredUrl;

    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed.clone())
        .max_pages(1)
        .concurrency(nz(2))
        .build();
    let mut engine = session_engine(config);
    // Pending work so the loop body runs; the collector is already full
    // so the FIRST budget-exit path is taken before any spawn.
    engine.scheduler.seed(&engine.config.seed_url).await;
    engine
        .collector
        .send(CrawlMessage::success(DiscoveredUrl::html(
            seed.clone(),
            0,
            seed.clone(),
        )))
        .await
        .expect("collector send must succeed");
    let (task_ctx, mut tasks) = loop_harness(&mut engine);
    engine
        .crawl_loop(
            &mut tasks,
            &task_ctx,
            std::time::Instant::now(),
            Duration::from_secs(1),
            TEST_WATCHDOG,
        )
        .await
        .expect("a budget exit is not a stall");

    assert!(
        engine.cancel_token.is_cancelled(),
        "first budget-exit break path must cancel in-flight work like the second path"
    );
    assert!(
        tasks.is_empty(),
        "no worker may be spawned after the budget decision"
    );
}
