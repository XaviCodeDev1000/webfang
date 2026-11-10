use super::tests::{entry_off, loop_harness, nz, pool_session, session_engine, TEST_WATCHDOG};
use super::*;
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::domain::CrawlError;
use url::Url;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Test helper: a session-built engine with an injected pipeline — the
/// seam a test wedges to hold a worker past its wait bound through a
/// real async await point (#2338).
fn session_engine_with_pipeline(config: CrawlerConfig, pipeline: Arc<PipelineExecutor>) -> Engine {
    Engine::from_session(pool_session(config, None, false, Some(pipeline)))
        .expect("engine must build")
}

/// The `max_depth(0)` shape shared by the two stall tests (#2313): one
/// dispatchable page, room for a second worker, and a frontier that
/// empties the moment the seed leaves — exactly one worker in flight.
fn stalled_shape() -> (
    Engine,
    Arc<CrawlTaskCtx>,
    tokio::task::JoinSet<Result<(), CrawlError>>,
) {
    let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_pages(1)
        .concurrency(nz(2))
        .build();
    let mut engine = session_engine(config);
    let (task_ctx, tasks) = loop_harness(&mut engine);
    (engine, task_ctx, tasks)
}

/// #2313 (strict-TDD RED): a worker that never completes must END the run
/// with an error at `worker_wait_bound`.
///
/// Pre-fix `wait_for_task` only warned — and with an empty frontier it did
/// not even wait, so the loop spun on a core and the run outlived every
/// timeout the runner had (240 s in CI, reported as `TERMINATING`). The 2 s
/// guard turns a regression into a failed assertion instead of a hung test.
#[tokio::test]
async fn wedged_worker_ends_the_run_instead_of_spinning() {
    let (mut engine, task_ctx, mut tasks) = stalled_shape();

    // A worker that never completes and never observes cancellation.
    tasks.spawn(async { std::future::pending::<Result<(), CrawlError>>().await });

    let bound = Duration::from_millis(100);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        engine.crawl_loop(
            &mut tasks,
            &task_ctx,
            std::time::Instant::now(),
            bound,
            TEST_WATCHDOG,
        ),
    )
    .await
    .expect("crawl_loop must return instead of spinning on a wedged worker");

    let err = result.expect_err("a wedged worker must fail the run");
    assert!(
        err.to_string().contains("crawl stalled"),
        "the failure must name the stall, got: {err}"
    );
    assert!(
        engine.cancel_token.is_cancelled(),
        "a stalled run must cancel the workers it is about to abandon"
    );
}

use crate::application::pipeline::{PipelineStage, ScrapedItem, StageOutcome};
use std::future::Future;
use std::pin::Pin;

/// A pipeline stage that never completes — the async wedge: the worker
/// awaits `process` forever, the same production shape as a hung stage
/// that ignores its own timeout and the cancel token (#2313). Unlike a
/// sync block, a pending future stays abortable, so the drain contract
/// stays exercisable.
struct NeverStage;

impl PipelineStage for NeverStage {
    fn name(&self) -> &str {
        "never"
    }

    fn process(
        &self,
        _item: ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = StageOutcome> + Send + '_>> {
        Box::pin(std::future::pending())
    }
}

/// #2338 (strict-TDD RED): a run a wedged worker kills must surface
/// `CrawlError::PartialRun`, not a bare `Internal`, so the caller keeps
/// whatever the crawl completed instead of discarding it. Also pins the
/// other half of the #2313 contract at the `run()` level: the loop error
/// propagates — deleting the `loop_result?` turns this test red.
///
/// The latency assertion pins the single-bound contract: the run ends at
/// loop detection + one short drain grace (~12s here), never the bound
/// twice — the old shape burned a second full 12s bound draining the
/// worker it had just declared wedged. The 20s ceiling keeps ~7s of
/// headroom over the 13s floor (CI-load tolerant) and still sits clearly
/// below the old 24s shape, so either regression flips the assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_run_returns_partial_results_in_the_error() {
    let _entry_off = entry_off();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"))
        .mount(&server)
        .await;
    let seed = Url::parse(&format!("{}/", server.uri())).expect("seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(10)
        .concurrency(nz(1))
        .timeout_secs(1)
        .ignore_robots(true)
        .build();
    let bound = Engine::worker_wait_bound(&config);
    let mut executor = PipelineExecutor::new();
    executor.add_stage(Box::new(NeverStage));
    let mut engine = session_engine_with_pipeline(config, Arc::new(executor));

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(40), engine.run())
        .await
        .expect("a stalled run must end within its own bounds");
    let elapsed = started.elapsed();
    let err = result.expect_err("a wedged worker must fail the run");
    assert!(
        err.to_string().contains("crawl stalled"),
        "the failure must name the stall, got: {err}"
    );
    let CrawlError::PartialRun { message, partial } = &err else {
        panic!("the stall must travel as PartialRun so partial results survive, got: {err:?}");
    };
    assert!(
        message.contains("crawl stalled"),
        "the rendered reason must be preserved, got: {message}"
    );
    assert!(
        partial.is_empty(),
        "no page reaches the collector through the wedged pipeline"
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "stall latency is one wait bound ({:?}) plus a short drain grace, never the bound twice; got {elapsed:?}",
        bound
    );
}

/// #2313: the shape of the four flaky tests — `max_depth(0)` empties the
/// frontier the moment the seed is dispatched, leaving exactly one worker
/// in flight. That is WAITING, not stalling: the loop must park on the
/// worker and finish `Ok`.
///
/// Pins the other side of the fix, so nobody "solves" the spin by failing
/// every iteration with an empty frontier.
#[tokio::test]
async fn empty_frontier_with_worker_in_flight_is_not_a_stall() {
    let (mut engine, task_ctx, mut tasks) = stalled_shape();

    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&finished);
    tasks.spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    });
    assert!(
        !engine.scheduler.has_pending_work(),
        "the fixture must reproduce the max_depth(0) shape: frontier empty"
    );

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        engine.crawl_loop(
            &mut tasks,
            &task_ctx,
            std::time::Instant::now(),
            Duration::from_secs(1),
            TEST_WATCHDOG,
        ),
    )
    .await
    .expect("the loop must return once the worker completes");

    assert!(
        result.is_ok(),
        "an empty frontier with a worker in flight is not a stall: {result:?}"
    );
    assert!(
        finished.load(std::sync::atomic::Ordering::SeqCst),
        "the loop must have waited for the worker instead of returning early"
    );
}
