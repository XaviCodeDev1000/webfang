use super::tests_common::session_engine;
use super::*;
use url::Url;

/// ADR-0012 sub-slice 3.B-1c — the autoscale loop MUST read RAM via the
/// injected [`RamProbePort`], not via a hardcoded `ResourceGovernor` static
/// call. This test injects a high-pressure reading, runs the autoscale
/// background task, and asserts the shared concurrency level reaches
/// `Critical` without ever touching real sysinfo.
#[tokio::test]
async fn with_autoscale_uses_injected_ram_probe() {
    #[derive(Debug)]
    struct HighPressureProbe;
    impl crate::domain::ram_probe_port::RamProbePort for HighPressureProbe {
        fn ram_usage_percent(&self) -> crate::domain::ram_probe_port::RamUsagePercent {
            crate::domain::ram_probe_port::RamUsagePercent::new_clamped(95.0)
        }
    }
    impl crate::domain::ram_probe_port::Sealed for HighPressureProbe {}

    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).build();
    let engine = session_engine(config)
        .with_ram_probe(Arc::new(HighPressureProbe))
        .with_autoscale();

    // The probe returns 95% on every poll. The autoscale background loop
    // ticks at 5s; we wait long enough for at least one post-skip tick
    // to land, then verify the shared level moved to Critical. The
    // probe is `Arc<dyn RamProbePort>` so it MUST be the one polled —
    // not the production `SystemRamProbe`.
    let level = engine
        .scheduler
        .autoscale_level()
        .expect("with_autoscale must install a level")
        .clone();
    // Sleep slightly longer than one 5s tick so the spawn'd loop runs
    // at least once past the initial skip-tick.
    tokio::time::sleep(Duration::from_millis(5_100)).await;
    assert_ne!(
        level.get(),
        crate::application::crawler::concurrency_level::ConcurrencyLevel::Normal,
        "autoscale loop must have moved off Normal under 95% injected RAM pressure",
    );
    assert_eq!(
        level.get(),
        crate::application::crawler::concurrency_level::ConcurrencyLevel::Critical,
        "autoscale loop must reach Critical at 95% (>= 90% threshold)",
    );

    // End the 5s poll loop this test started (#1941) instead of leaking it
    // into the rest of the suite.
    engine.shutdown().await;
}

/// #1941: the RAM autoscale loop is an unbounded poll task (it loops on a
/// 5s tick forever). `with_autoscale` used to `tokio::spawn` it and drop
/// the `JoinHandle`, so nothing owned it — not the engine, not shutdown —
/// and the task outlived the crawl that created it, keeping the runtime
/// alive and polling a probe no one reads. The engine must hold the handle
/// and end it in `shutdown`, exactly like `signal_handle`.
#[tokio::test]
async fn autoscale_probe_handle_is_owned_and_aborted_by_shutdown() {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed).build();
    let engine = session_engine(config)
        .with_ram_probe(Arc::new(CountingProbe(Arc::new(
            std::sync::atomic::AtomicUsize::new(0),
        ))))
        .with_autoscale();

    let abort = engine
        .autoscale_handle
        .as_ref()
        .map(tokio::task::JoinHandle::abort_handle)
        .expect("with_autoscale must record the spawned probe handle");
    assert!(
        !abort.is_finished(),
        "the probe task must be live before shutdown, else this proves nothing"
    );

    engine.shutdown().await;

    // `abort()` marks the task; the runtime observes it on the next poll
    // pass, so yield a bounded number of times before reading the verdict.
    for _ in 0..32 {
        if abort.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        abort.is_finished(),
        "shutdown must abort the autoscale probe task instead of leaking it"
    );
}

/// Counter used by the test above; the value is never read — only the
/// fact that the task exists and stops is under test.
#[derive(Debug)]
struct CountingProbe(Arc<std::sync::atomic::AtomicUsize>);

impl RamProbePort for CountingProbe {
    fn ram_usage_percent(&self) -> crate::domain::ram_probe_port::RamUsagePercent {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::domain::ram_probe_port::RamUsagePercent::new_clamped(10.0)
    }
}

impl crate::domain::ram_probe_port::Sealed for CountingProbe {}
