use super::tests::{loop_harness, nz, session_engine};
use super::*;
use url::Url;

/// #2338: `run()` derives the pause watchdog from the operator config —
/// raising `pause_give_up_after_secs` must reach the verdicts, not stay
/// behind the hardcoded 30-minute ceiling the tests still carry.
#[test]
fn pause_watchdog_comes_from_the_crawler_config() {
    let seed = Url::parse("https://example.com").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .pause_alert_after_secs(30)
        .pause_give_up_after_secs(3600)
        .build();

    let watchdog = PauseWatchdog::from_config(&config);
    assert_eq!(watchdog.alert_after, Duration::from_secs(30));
    assert_eq!(watchdog.give_up_after, Duration::from_secs(3600));
    assert!(
        matches!(
            watchdog.verdict(Duration::from_secs(3599)),
            PauseVerdict::Report
        ),
        "a pause inside the raised ceiling is reported, never given up on"
    );
    assert!(matches!(
        watchdog.verdict(Duration::from_secs(3600)),
        PauseVerdict::GiveUp
    ));
}

/// #2313 sibling: an autoscale `Critical` pause has work to dispatch, no
/// worker in flight and no completion to wait on.
///
/// It must not spin (a core per paused crawl) and — the point of this test —
/// it must NOT be judged by `worker_wait_bound`: a deliberate throttle is not
/// a hung worker. The pause survives several worker bounds and only ends at
/// its own give-up threshold, with the state that explains why.
#[tokio::test]
async fn autoscale_pause_is_judged_by_its_own_watchdog() {
    use crate::application::crawler::concurrency_level::{
        ConcurrencyLevel, SharedConcurrencyLevel,
    };

    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_pages(10)
        .concurrency(nz(2))
        .build();
    let mut engine = session_engine(config);
    // Buffered work without touching the discovery queue: the loop must see
    // a frontier it cannot dispatch.
    engine
        .scheduler
        .restore_pending(&["http://127.0.0.1:9/".to_string()]);

    let level = Arc::new(SharedConcurrencyLevel::new());
    level.set(ConcurrencyLevel::Critical);
    engine.scheduler.set_autoscale(level);

    let (task_ctx, mut tasks) = loop_harness(&mut engine);
    // 20 ms worker bound vs 180 ms give-up: the pause outlives the worker
    // bound nine times over, so a run that dies at the bound fails here.
    let bound = Duration::from_millis(20);
    let pause = PauseWatchdog {
        alert_after: Duration::from_millis(60),
        give_up_after: Duration::from_millis(180),
    };
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        engine.crawl_loop(&mut tasks, &task_ctx, started, bound, pause),
    )
    .await
    .expect("a pause that never lifts must end the run, not spin on it");
    let elapsed = started.elapsed();

    let err = result.expect_err("a pause past its ceiling must fail the run");
    let message = err.to_string();
    assert!(
        elapsed >= pause.give_up_after,
        "a deliberate pause must outlive worker_wait_bound ({bound:?}): it ended after {elapsed:?}"
    );
    assert!(
        message.contains("crawl paused"),
        "the failure must name the pause, not a stall, got: {message}"
    );
    assert!(
        message.contains("pending=1"),
        "the report must carry the frontier size, got: {message}"
    );
}

/// The pause thresholds are a pure decision: wait, report once, give up.
/// Pinned at the exact boundaries so a future tuning cannot silently turn
/// the alert into a failure (or the failure into a hang).
#[test]
fn pause_verdict_waits_reports_then_gives_up() {
    let pause = PauseWatchdog {
        alert_after: Duration::from_secs(300),
        give_up_after: Duration::from_secs(1800),
    };
    assert!(matches!(pause.verdict(Duration::ZERO), PauseVerdict::Wait));
    assert!(matches!(
        pause.verdict(Duration::from_secs(299)),
        PauseVerdict::Wait
    ));
    assert!(matches!(
        pause.verdict(Duration::from_secs(300)),
        PauseVerdict::Report
    ));
    assert!(matches!(
        pause.verdict(Duration::from_secs(1799)),
        PauseVerdict::Report
    ));
    assert!(matches!(
        pause.verdict(Duration::from_secs(1800)),
        PauseVerdict::GiveUp
    ));
}
