use super::*;
use crate::application::crawler::concurrency_level::ConcurrencyLevel;

fn url(path: &str) -> Url {
    Url::parse(&format!("https://example.com{path}")).unwrap()
}

fn disc(path: &str) -> DiscoveredUrl {
    let u = url(path);
    DiscoveredUrl::html(u.clone(), 0, u)
}

/// Test constructor wrapping the NonZero-gated newtype: a raw `usize`
/// bound is no longer representable (task 2.2b — D4 tier newtype).
fn sched(n: usize) -> CrawlScheduler {
    CrawlScheduler::new(CrawlConcurrency::new(n).expect("test concurrency non-zero"))
}

// -- record_visit --

#[test]
fn record_visit_first_true_duplicate_false() {
    let s = sched(4);
    assert!(s.record_visit("https://example.com/a"));
    assert!(!s.record_visit("https://example.com/a"));
    assert!(s.record_visit("https://example.com/b"));
}

#[test]
fn record_visit_mirror_skips_duplicates() {
    let s = sched(4);
    assert!(s.record_visit("https://example.com/a"));
    assert!(s.record_visit("https://example.com/b"));
    assert!(!s.record_visit("https://example.com/a"));
    let snap = s.snapshot_visited();
    assert_eq!(snap.len(), 2);
    assert!(snap.contains("https://example.com/a"));
    assert!(snap.contains("https://example.com/b"));
}

// -- restore_visited / snapshot_visited --

#[test]
fn restore_visited_is_idempotent() {
    let s = sched(4);
    assert!(s.record_visit("https://example.com/a"));
    let mut set = HashSet::new();
    set.insert("https://example.com/a".to_string());
    set.insert("https://example.com/b".to_string());
    s.restore_visited(&set);
    s.restore_visited(&set);
    let snap = s.snapshot_visited();
    assert_eq!(snap.len(), 2);
    assert!(snap.contains("https://example.com/a"));
    assert!(snap.contains("https://example.com/b"));
}

#[test]
fn snapshot_visited_roundtrips() {
    let s = sched(4);
    let original: HashSet<String> = [
        "https://example.com/a",
        "https://example.com/b",
        "https://example.com/c",
    ]
    .iter()
    .map(|u| (*u).to_string())
    .collect();
    s.restore_visited(&original);
    assert_eq!(s.snapshot_visited(), original);
}

// -- effective_concurrency / can_spawn --

/// Task 2.2(b): the scheduler's spawn bound follows the injected
/// [`BudgetModel`]'s `Operation.crawl` tier — never a raw configured
/// concurrency value.
#[test]
fn scheduler_bound_follows_injected_budget_model() {
    use crate::domain::budget::detector::FixedDetector;
    use crate::domain::budget::{BudgetModel, BudgetOverrides};

    fn scheduler_from_cores(cores: usize) -> CrawlScheduler {
        let detector = FixedDetector::with_detection(
            std::num::NonZeroUsize::new(cores).expect("test cores non-zero"),
            None,
        );
        let model = BudgetModel::build(BudgetOverrides::default(), &detector);
        CrawlScheduler::new(model.crawl())
    }

    // 4-core auto table → crawl 3.
    let s = scheduler_from_cores(4);
    assert!(s.can_spawn(2));
    assert!(!s.can_spawn(3));

    // 16-core auto table → crawl min(15, 8) = 8.
    let s = scheduler_from_cores(16);
    assert!(s.can_spawn(7));
    assert!(!s.can_spawn(8));
}

#[test]
fn effective_concurrency_defaults_to_base() {
    // Zero is unrepresentable since 2.2b: `CrawlConcurrency` rejects it
    // at construction (NonZero guard).
    assert_eq!(sched(7).effective_concurrency(), 7);
    assert_eq!(sched(1).effective_concurrency(), 1);
}

#[test]
fn effective_concurrency_applies_autoscale() {
    let mut s = sched(10);
    let level = Arc::new(SharedConcurrencyLevel::new());
    s.set_autoscale(Arc::clone(&level));
    assert_eq!(s.effective_concurrency(), 10);
    level.set(ConcurrencyLevel::Reduced);
    assert_eq!(s.effective_concurrency(), 5);
    level.set(ConcurrencyLevel::Critical);
    assert_eq!(s.effective_concurrency(), 0);
}

#[test]
fn can_spawn_boundary() {
    let s = sched(3);
    assert!(s.can_spawn(0));
    assert!(s.can_spawn(2));
    assert!(!s.can_spawn(3));
    assert!(!s.can_spawn(4));
}

// -- next_url --

#[tokio::test]
async fn next_url_none_at_limit_leaves_pending_untouched() {
    let mut s = sched(1);
    s.seed(&url("/a")).await;
    assert!(s.has_pending_work());
    assert!(s.next_url(1).is_none());
    assert!(
        s.has_pending_work(),
        "pending buffer must not be consumed at the limit"
    );
    assert!(s.next_url(0).is_some());
    assert!(!s.has_pending_work());
}

#[tokio::test]
async fn next_url_skips_already_visited() {
    let mut s = sched(10);
    s.seed(&url("/a")).await;
    s.seed(&url("/b")).await;
    assert!(s.record_visit("https://example.com/a"));
    let next = s.next_url(0).unwrap();
    assert_eq!(next.url.path(), "/b");
}

#[tokio::test]
async fn next_url_marks_returned_as_visited() {
    let mut s = sched(10);
    s.seed(&url("/a")).await;
    let next = s.next_url(0).unwrap();
    assert_eq!(next.url.path(), "/a");
    assert!(
        !s.record_visit("https://example.com/a"),
        "returned URL must be marked visited"
    );
    assert!(s.snapshot_visited().contains("https://example.com/a"));
}

#[tokio::test]
async fn next_url_none_when_out_of_work() {
    let mut s = sched(10);
    assert!(s.next_url(0).is_none());
    s.seed(&url("/a")).await;
    assert!(s.next_url(0).is_some());
    assert!(s.next_url(0).is_none());
}

#[tokio::test]
async fn next_url_returns_fifo_order() {
    let mut s = sched(10);
    s.seed(&url("/a")).await;
    s.seed(&url("/b")).await;
    s.seed(&url("/c")).await;
    assert_eq!(s.next_url(0).unwrap().url.path(), "/a");
    assert_eq!(s.next_url(0).unwrap().url.path(), "/b");
    assert_eq!(s.next_url(0).unwrap().url.path(), "/c");
    assert!(s.next_url(0).is_none());
}

// -- has_pending_work / seed / drain_discovered --

#[tokio::test]
async fn has_pending_work_tracks_buffer() {
    let mut s = sched(10);
    assert!(!s.has_pending_work());
    s.seed(&url("/a")).await;
    assert!(s.has_pending_work());
    let _ = s.next_url(0).unwrap();
    assert!(!s.has_pending_work());
}

#[tokio::test]
async fn seed_pushes_to_pending_and_queue() {
    let mut s = sched(4);
    let q = s.queue();
    s.seed(&url("/seed")).await;
    assert!(s.has_pending_work());
    assert_eq!(q.len().await, 1);
    assert_eq!(s.next_url(0).unwrap().url.path(), "/seed");
}

#[tokio::test]
async fn drain_discovered_moves_queue_into_pending() {
    let mut s = sched(4);
    let q = s.queue();
    assert!(q.push_prioritized(disc("/x"), UrlSource::Link).await);
    assert!(!s.has_pending_work(), "pending empty before drain");
    s.drain_discovered().await;
    assert!(s.has_pending_work());
    assert_eq!(q.len().await, 0, "queue drained");
    assert_eq!(s.next_url(0).unwrap().url.path(), "/x");
}

// -- snapshot_pending / restore_pending (#517) --

#[tokio::test]
async fn snapshot_pending_captures_pending_and_queue() {
    let mut s = sched(4);
    // One URL sits in the pending buffer, one still in the shared queue.
    s.restore_pending(&["https://example.com/buffered".into()]);
    s.queue()
        .push_prioritized(disc("/queued"), UrlSource::Link)
        .await;
    let snap = s.snapshot_pending_bounded(usize::MAX).await;
    assert_eq!(snap.len(), 2, "both sources must be captured: {snap:?}");
    assert!(snap.iter().any(|u| u.ends_with("/buffered")));
    assert!(snap.iter().any(|u| u.ends_with("/queued")));
}

/// #1234 / F-39 — the persisted frontier can never exceed the budget the run
/// already set for itself.
///
/// The pathological file F-39 measured held 4955 queued URLs against 49
/// visited, and the next `--resume` drained all of it first because
/// `max_pages` was never part of the checkpoint. Deleting the field was tried
/// and measured: it strands resume at zero pages. The bound is the fix, and
/// it lives here, next to the queue it truncates.
#[tokio::test]
async fn snapshot_pending_bounded_caps_at_the_page_budget() {
    let mut s = sched(4);
    s.restore_pending(&["https://example.com/buffered".into()]);
    for i in 0..50 {
        s.queue()
            .push_prioritized(disc(&format!("/q{i}")), UrlSource::Link)
            .await;
    }
    assert_eq!(
        s.snapshot_pending_bounded(usize::MAX).await.len(),
        51,
        "baseline: one buffered + fifty queued",
    );

    let bounded = s.snapshot_pending_bounded(10).await;
    assert_eq!(
        bounded.len(),
        10,
        "the frontier must never exceed the run's own budget (#1234)",
    );
    assert!(
        bounded.iter().any(|u| u.ends_with("/buffered")),
        "already-dequeued work is next in line and must survive truncation: {bounded:?}",
    );

    assert_eq!(
        s.snapshot_pending_bounded(100).await.len(),
        51,
        "a budget larger than the frontier must neither invent nor drop entries",
    );
}

#[tokio::test]
async fn snapshot_pending_is_nondestructive() {
    let mut s = sched(4);
    s.restore_pending(&["https://example.com/buffered".into()]);
    s.queue()
        .push_prioritized(disc("/queued"), UrlSource::Link)
        .await;
    let _ = s.snapshot_pending_bounded(usize::MAX).await;
    assert!(s.has_pending_work(), "pending buffer must survive snapshot");
    assert_eq!(s.queue().len().await, 1, "queue must survive snapshot");
}

#[test]
fn restore_pending_reenqueues_for_next_url() {
    let mut s = sched(4);
    s.restore_pending(&[
        "https://example.com/p1".into(),
        "https://example.com/p2".into(),
    ]);
    assert!(s.has_pending_work());
    assert_eq!(s.next_url(0).unwrap().url.path(), "/p1");
    assert_eq!(s.next_url(0).unwrap().url.path(), "/p2");
    assert!(!s.has_pending_work());
}

#[test]
fn restore_pending_skips_unparseable_urls() {
    let mut s = sched(4);
    s.restore_pending(&["not-a-url".into(), "https://example.com/ok".into()]);
    assert_eq!(s.next_url(0).unwrap().url.path(), "/ok");
    assert!(!s.has_pending_work());
}

#[tokio::test]
async fn restore_pending_then_next_url_skips_visited() {
    let mut s = sched(4);
    s.record_visit("https://example.com/done");
    s.restore_pending(&[
        "https://example.com/done".into(),
        "https://example.com/fresh".into(),
    ]);
    assert_eq!(
        s.next_url(0).unwrap().url.path(),
        "/fresh",
        "restored URL already visited must be skipped"
    );
    assert!(!s.has_pending_work());
}

#[tokio::test]
async fn snapshot_restore_pending_roundtrips() {
    let mut s = sched(4);
    s.restore_pending(&["https://example.com/buffered".into()]);
    s.queue()
        .push_prioritized(disc("/queued"), UrlSource::Link)
        .await;
    let snap = s.snapshot_pending_bounded(usize::MAX).await;

    let mut s2 = sched(4);
    s2.restore_pending(&snap);
    let mut paths: Vec<String> = Vec::new();
    while let Some(d) = s2.next_url(0) {
        paths.push(d.url.path().to_string());
    }
    assert_eq!(
        paths.len(),
        2,
        "all pending URLs must be restored: {paths:?}"
    );
    assert!(paths.contains(&"/buffered".into()));
    assert!(paths.contains(&"/queued".into()));
}
