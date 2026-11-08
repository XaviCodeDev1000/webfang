use super::*;
use crate::application::crawler::engine::tests::{entry_off, nz, session_engine};
use crate::domain::budget::{BudgetModel, BudgetOverrides};
use crate::domain::CrawlerConfig;
use url::Url;
use wiremock::matchers::{path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Shutdown while a worker waits: cancellation must abort the parked
/// worker and `run()` must return within seconds, not after the 60s
/// rate-limit refill (#509 acceptance).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_aborts_rate_blocked_worker_and_run_returns_within_bound() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    let port = server.address().port();
    // Burst-decoupled (#302 D1): the token-bucket burst no longer tracks
    // `config.concurrency`, so blocking a worker behind the 60s refill
    // must not depend on burst == 1. Link MORE URLs than the maximum
    // possible burst (ceiling 16): at least one worker necessarily waits
    // for a refill that never comes before cancellation fires.
    let links: String = (0..20)
        .map(|i| format!(r#"<a href=\"http://127.0.0.1:{port}/linked{i}\">next</a>"#))
        .collect();
    Mock::given(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("<html><body>{links}</body></html>")),
        )
        .mount(&server)
        .await;
    Mock::given(path_regex("/linked\\d+".to_string()))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body>linked</body></html>"),
        )
        .mount(&server)
        .await;

    let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(1)
        .max_pages(50) // above link count: page count must be bounded by burst, not max_pages
        .concurrency(nz(1))
        .delay_ms(60_000) // 60s refill: without cancellation run() hangs ~1 min
        .timeout_secs(5)
        .ignore_robots(true)
        .build();

    let mut engine = session_engine(config);
    let cancel = engine.cancel_handle();

    let run_handle = tokio::spawn(async move { engine.run().await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let seed_received = server
            .received_requests()
            .await
            .map(|reqs| reqs.iter().any(|r| r.url.path() == "/"))
            .unwrap_or(false);
        if seed_received {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "seed request never arrived"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Let immediate-burst workers land so late-discovered workers are
    // genuinely parked on the 60s refill when cancellation fires.
    tokio::time::sleep(Duration::from_millis(100)).await;

    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(10), run_handle)
        .await
        .expect("run() must return within the bound after cancellation")
        .expect("spawned run task must not panic");
    let crawl = result.expect("cancelled crawl must still return a result");

    // #509 acceptance: blocked workers abort as control signals.
    assert_eq!(crawl.errors, 0, "cancelled tasks are control signals");
    // Burst ceiling bounds immediate fetches (seed + <= burst links);
    // the 60s refill means the cancelled run can NEVER fetch all 20.
    let max_burst = crate::domain::budget::clamp::MAX_CONCURRENCY_CEILING;
    assert!(
        crawl.total_pages <= 1 + max_burst,
        "total_pages={} exceeds seed + maximum burst {max_burst}",
        crawl.total_pages
    );
    assert!(
        crawl.total_pages < 20,
        "cancelled run must not fetch all 20 links (60s refill), got {}",
        crawl.total_pages
    );
}

/// Regression test for issue #590, bug #1: max_pages MUST be a hard
/// ceiling. The engine re-checks `collector.is_full()` AFTER
/// `process_completed_tasks()` and BEFORE `spawn_available_tasks()` so
/// concurrent completions past the limit cannot fan out more workers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_pages_hard_ceiling_no_overshoot() {
    let server = MockServer::start().await;
    let port = server.address().port();

    // Seed page links to many pages — without the fix, the engine would
    // keep spawning and overshoot max_pages by a wide margin.
    let links: String = (0..20)
        .map(|i| format!(r#"<a href="http://127.0.0.1:{port}/page{i}">link</a>"#))
        .collect();
    Mock::given(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("<html><body>{links}</body></html>")),
        )
        .mount(&server)
        .await;

    // Every linked page returns 200 with no further links.
    for i in 0..20 {
        Mock::given(path(format!("/page{i}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("<html><body>page {i}</body></html>")),
            )
            .mount(&server)
            .await;
    }

    let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(2)
        .max_pages(2) // Hard ceiling: must not exceed by much
        .concurrency(nz(5)) // High concurrency to trigger the race
        .delay_ms(1)
        .timeout_secs(5)
        .ignore_robots(true)
        .build();

    let mut engine = session_engine(config);
    let result = engine.run().await.expect("crawl must complete");
    engine.shutdown().await;

    // The fix guarantees the loop breaks as soon as counter >= max_pages.
    // Up to (spawn bound - 1) in-flight tasks may still land, so we allow
    // a small slack but assert it stays bounded. Since task 2.2b the
    // spawn bound is the model's Operation.crawl tier — the configured
    // `concurrency(5)` no longer gates spawning.
    let crawl_tier = BudgetModel::build(
        BudgetOverrides::default(),
        &crate::domain::budget::detector::SystemDetector,
    )
    .crawl()
    .get();
    assert!(
        result.total_pages <= 2 + crawl_tier,
        "total_pages={} must not overshoot max_pages=2 by more than the \
                 Operation.crawl tier slack ({crawl_tier})",
        result.total_pages
    );
    // Critical: must NOT have fetched all 20 links.
    assert!(
        result.total_pages < 20,
        "must not fetch all 20 links with max_pages=2, got {}",
        result.total_pages
    );
}

/// Regression test for #634: seed URL that matches an exclude pattern
/// MUST be filtered out — 0 pages crawled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_excluded_by_host_pattern_returns_empty() {
    let server = MockServer::start().await;
    let port = server.address().port();
    Mock::given(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"))
        .mount(&server)
        .await;

    let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(10)
        .exclude_pattern("127.0.0.1") // Exclude the seed host
        .ignore_robots(true)
        .build();

    let mut engine = session_engine(config);
    let result = engine.run().await.expect("engine run must succeed");
    engine.shutdown().await;

    assert_eq!(
        result.total_pages, 0,
        "seed matching exclude pattern must produce 0 pages, got {}",
        result.total_pages
    );
}

/// Regression test for #634: seed URL that does NOT match an exclude
/// pattern MUST still be crawled (sanity check).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_not_excluded_is_crawled() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    let port = server.address().port();
    Mock::given(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"))
        .mount(&server)
        .await;

    let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(10)
        .exclude_pattern("other-host.com") // Does NOT match seed
        .ignore_robots(true)
        .build();

    let mut engine = session_engine(config);
    let result = engine.run().await.expect("engine run must succeed");
    engine.shutdown().await;

    assert_eq!(
        result.total_pages, 1,
        "seed NOT matching exclude pattern must be crawled, got {} pages",
        result.total_pages
    );
}
