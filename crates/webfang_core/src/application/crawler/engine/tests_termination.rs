use super::tests_common::session_engine;
use super::*;
use crate::domain::CrawlError;
use url::Url;

/// #2338: the partial payload is the summary a completed run would have
/// returned, truncated to what the collector already held — URLs, error
/// count and breakdown, with the rendered failure preserved verbatim.
#[tokio::test]
async fn fail_run_with_partials_carries_collected_urls() {
    use crate::application::crawler::collector::CrawlMessage;
    use crate::domain::DiscoveredUrl;

    let seed = Url::parse("https://example.com").expect("valid seed URL");
    let mut engine = session_engine(CrawlerConfig::builder(seed).build());
    let collected = DiscoveredUrl::html(
        Url::parse("https://example.com/page").expect("valid page URL"),
        0,
        Url::parse("https://example.com/").expect("valid parent URL"),
    );
    engine
        .collector
        .send(CrawlMessage::success(collected))
        .await
        .expect("collector channel must be open");

    let err = engine
        .fail_run_with_partials(CrawlError::Internal("crawl stalled: test".to_string()))
        .await;

    let CrawlError::PartialRun { message, partial } = err else {
        panic!("the failure must carry the partials, got: {err:?}");
    };
    assert!(
        message.contains("crawl stalled"),
        "the rendered reason must be preserved, got: {message}"
    );
    assert_eq!(partial.total_pages, 1, "the collected page must survive");
    assert_eq!(partial.urls.len(), 1);
}

/// Source that resolves immediately with a fixed event name.
///
/// Stands in for the platform console source without touching the OS, so
/// the assertion is about the engine's contract (drain on the INJECTED
/// source) rather than about a signal a test may not deliver.
fn immediate_source(name: &'static str) -> TerminationSource {
    TerminationSource::new(Arc::new(move || {
        let name: &'static str = name;
        Box::pin(async move { Some(name) })
    }))
}

/// Source that never resolves — the shape a degraded/absent platform
/// source has.
fn pending_source() -> TerminationSource {
    TerminationSource::new(Arc::new(|| Box::pin(std::future::pending())))
}

/// Source whose registration the OS REJECTED: the wait resolves to
/// `None` instead of an event name (#509's degradation shape).
fn rejected_source() -> TerminationSource {
    TerminationSource::new(Arc::new(|| Box::pin(async { None::<&'static str> })))
}

/// #1808: the engine drains on the source it was given, not on one it
/// reaches for itself. Before the port existed, `spawn_signal_handler`
/// named the infrastructure console source directly, so an injected source
/// could not be observed at all — and `application` reached outward.
#[tokio::test]
async fn an_injected_termination_source_drives_the_engine_shutdown() {
    let shutdown: ShutdownSignal = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel = CancellationToken::new();
    let handle = Engine::spawn_signal_handler(
        Arc::clone(&shutdown),
        cancel.clone(),
        immediate_source("TEST_TERMINATION_EVENT"),
    );

    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("the handler must return once the injected source fires")
        .expect("the handler task must not panic");

    assert!(
        shutdown.load(std::sync::atomic::Ordering::SeqCst),
        "the injected source must set the run's shutdown flag"
    );
    assert!(
        cancel.is_cancelled(),
        "the injected source must fire the run's single cancellation authority (ADR-0016)"
    );
}

/// Negative case for the same contract: a source that never delivers
/// leaves the run alone. This is what a rejected Windows console-event
/// registration degrades to, and it must NOT be read as "shutting down
/// now" by the handler itself.
#[tokio::test]
async fn a_never_delivering_source_leaves_the_run_alone() {
    let shutdown: ShutdownSignal = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel = CancellationToken::new();
    let handle =
        Engine::spawn_signal_handler(Arc::clone(&shutdown), cancel.clone(), pending_source());
    assert!(!handle.is_finished(), "a pending source must not resolve");

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !shutdown.load(std::sync::atomic::Ordering::SeqCst),
        "an undelivered event must not set the shutdown flag"
    );
    assert!(
        !cancel.is_cancelled(),
        "an undelivered event must not cancel the run"
    );
    handle.abort();
}

/// The default is the Ctrl+C-only source: awaiting it installs nothing
/// that panics and stays cancellable, which is what every construction
/// site that never injects a platform source relies on.
#[tokio::test]
async fn the_default_source_is_a_cancellable_ctrl_c_wait() {
    let waiter = tokio::spawn(TerminationSource::default().wait());
    tokio::task::yield_now().await;
    assert!(
        !waiter.is_finished(),
        "no test may deliver Ctrl+C, so the default source must stay pending"
    );
    waiter.abort();
    assert!(waiter.await.is_err(), "abort must release the wait");
}

/// #1835: a REJECTED registration is not a termination event. The engine
/// must leave the run alone — the shutdown flag unset and the
/// cancellation token live — so the run keeps going and waits for an
/// explicit cancel. The CLI shutdown guard answers the same way, and both
/// say `TERMINATION_REGISTRATION_REJECTED`.
#[tokio::test]
async fn a_rejected_registration_leaves_the_run_alone() {
    let shutdown: ShutdownSignal = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel = CancellationToken::new();
    let handle =
        Engine::spawn_signal_handler(Arc::clone(&shutdown), cancel.clone(), rejected_source());

    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("the handler must return once the rejected source reports itself")
        .expect("the handler task must not panic");

    assert!(
        !shutdown.load(std::sync::atomic::Ordering::SeqCst),
        "a rejected registration must not set the shutdown flag (#1835)"
    );
    assert!(
        !cancel.is_cancelled(),
        "a rejected registration must not cancel the run (#1835)"
    );
}
