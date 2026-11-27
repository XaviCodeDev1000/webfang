//! Crawl scheduling policy — visited tracking, work queue, and concurrency limits.
//!
//! Extracted from `engine.rs` (strangler fig Corte C, issue #440). Encapsulates
//! the *scheduling decisions* that were previously inlined in `Engine::run()`:
//! which URL to crawl next, deduplication of visited URLs, the pending-work
//! buffer drained from the shared [`UrlQueue`](crate::infrastructure::crawler::UrlQueue), and the autoscale-aware
//! concurrency limit.
//!
//! The `Engine` remains the *orchestration mechanism*: it owns the `JoinSet`,
//! spawns tasks via the `crawl_task` module, coordinates signal-driven shutdown,
//! and persists checkpoints. `CrawlScheduler` carries no task, shutdown, or
//! checkpoint state.
//!
//! # Shared state
//!
//! `visited`, `visited_urls`, and `queue` are `Arc`-shared with the spawned
//! per-page tasks (via `CrawlTaskCtx`): tasks dedup discovered links and push
//! them onto `queue`, while the scheduler drains `queue` into its local
//! `pending` buffer and decides what to crawl next. The scheduler owns the
//! canonical `Arc`s and exposes accessors so the `Engine` clones them into the
//! task context exactly once.
//!
//! # D6 lock-across-await audit (task 2.3, change stabilization-concurrency-budget)
//!
//! Functions rewired by commit 20806c7a (scheduler gating):
//!
//! | Function | `.await` points | Guard discipline | Verdict |
//! |---|---|---|---|
//! | `CrawlScheduler::new` | none (sync fn) | constructs `RwLock`/dedup state by value; no guard held past initialization expressions | PASS |
//! | `CrawlScheduler::effective_concurrency` | none (sync fn) | reads `SharedConcurrencyLevel` atomics only; no lock guard taken | PASS |
//!
//! Async callers (`snapshot_pending`, queue drain) touch the shared [`UrlQueue`](crate::infrastructure::crawler::UrlQueue)
//! whose sole `tokio::sync::Mutex` is confined to documented sync-only sections
//! (`infrastructure/crawler/url_queue.rs`, invariant AL-2).
//!
//! Enforcement: `#![deny(clippy::await_holding_lock)]` below fails the build if
//! a future edit ever holds a `std` lock guard across an `.await` in this module.

#![deny(clippy::await_holding_lock)]

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, RwLock};

use url::Url;

use super::concurrency_level::SharedConcurrencyLevel;
use crate::application::deduplicator::UrlDeduplicator;
use crate::domain::budget::CrawlConcurrency;
use crate::domain::crawler_port::UrlQueuePort;
use crate::domain::crawler_port::UrlSource;
use crate::domain::DiscoveredUrl;

/// Scheduling policy for a crawl: visited dedup, pending-work buffer, and
/// concurrency limits.
///
/// Owns the canonical `Arc`s for the visited set ([`UrlDeduplicator`]), the
/// checkpoint string mirror (`visited_urls`), and the shared discovery queue
/// ([`UrlQueue`](crate::infrastructure::crawler::UrlQueue)). These are `Arc`-shared with the per-page tasks, which push
/// discovered links onto `queue`; the scheduler drains that queue into a local
/// `pending` buffer and hands out the next URL to crawl.
pub(crate) struct CrawlScheduler {
    /// Lock-free dedup of visited URLs (hash set).
    visited: Arc<UrlDeduplicator>,
    /// String mirror of visited URLs for checkpoint persistence.
    visited_urls: Arc<RwLock<Vec<String>>>,
    /// URLs whose failure was permanent, so a resume must not retry them.
    /// Owned by the engine's completion path (one writer, the loop thread), so
    /// a plain set is enough — no `Arc`, because no task mutates it.
    settled: HashSet<String>,
    /// Shared discovery queue — tasks push discovered links here. Erased
    /// behind the domain port (ADR-0012-B unit 8); `Arc`-shared with the
    /// per-page tasks so enqueue-time dedup stays global.
    queue: Arc<dyn UrlQueuePort>,
    /// Local work buffer drained from `queue`; the scheduler pops from here.
    pending: VecDeque<DiscoveredUrl>,
    /// Base concurrency — the budget model's `Operation.crawl` tier.
    /// A plain `usize` would let zero or unclamped values in; the tier
    /// newtype makes an invalid bound unrepresentable (design D4).
    base_concurrency: CrawlConcurrency,
    /// Optional autoscale level for RAM-aware concurrency adjustment.
    autoscale_level: Option<Arc<SharedConcurrencyLevel>>,
}

impl CrawlScheduler {
    /// Create a scheduler with fresh scheduling state, gated by the budget
    /// model's crawl tier.
    ///
    /// Builds the visited set, its checkpoint string mirror, and the shared
    /// discovery queue internally; these are `Arc`-shared with the per-page
    /// tasks via the accessors below.
    pub(crate) fn new(base_concurrency: CrawlConcurrency) -> Self {
        Self {
            visited: Arc::new(UrlDeduplicator::new()),
            visited_urls: Arc::new(RwLock::new(Vec::new())),
            settled: HashSet::new(),
            queue: crate::application::container::build_url_queue(),
            pending: VecDeque::new(),
            base_concurrency,
            autoscale_level: None,
        }
    }

    /// Enable RAM-aware autoscaling of the concurrency limit.
    pub(crate) fn set_autoscale(&mut self, level: Arc<SharedConcurrencyLevel>) {
        self.autoscale_level = Some(level);
    }

    /// Borrow the autoscale level handle (if any) — used by engine integration
    /// tests to assert the background poller has moved the level.
    #[cfg(test)]
    pub(crate) fn autoscale_level(&self) -> Option<&Arc<SharedConcurrencyLevel>> {
        self.autoscale_level.as_ref()
    }

    /// Clone the discovery queue for the shared task context.
    pub(crate) fn queue(&self) -> Arc<dyn UrlQueuePort> {
        Arc::clone(&self.queue)
    }

    /// Record a URL as visited (hash dedup + string mirror).
    ///
    /// Returns `true` if newly inserted, `false` if already visited.
    pub(crate) fn record_visit(&self, url: &str) -> bool {
        if self.visited.try_insert(url) {
            if let Ok(mut urls) = self.visited_urls.write() {
                urls.push(url.to_string());
            }
            true
        } else {
            false
        }
    }

    /// Restore visited URLs from a checkpoint (idempotent via dedup).
    pub(crate) fn restore_visited(&self, urls: &HashSet<String>) {
        for url in urls {
            self.record_visit(url);
        }
    }

    /// Mark a URL as permanently settled: its failure cannot be fixed by
    /// retrying it, so no resume may re-drive it.
    pub(crate) fn record_settled(&mut self, url: &str) {
        self.settled.insert(url.to_string());
    }

    /// Snapshot the settled set for checkpoint persistence.
    pub(crate) fn snapshot_settled(&self) -> HashSet<String> {
        self.settled.clone()
    }

    /// Restore the settled set from a checkpoint, so later saves of the same
    /// run never lose an earlier verdict.
    pub(crate) fn restore_settled(&mut self, settled: HashSet<String>) {
        self.settled.extend(settled);
    }

    /// Snapshot the visited-URL string mirror for checkpoint persistence.
    #[allow(clippy::expect_used)]
    pub(crate) fn snapshot_visited(&self) -> HashSet<String> {
        let urls = self
            .visited_urls
            .read()
            // LCOV_EXCL_LINE defensive: rwlock-poisoning — a poisoned lock leaves process state undefined
            .expect("visited_urls RwLock poisoned");
        urls.iter().cloned().collect()
    }

    /// The pending frontier truncated to the run's own page budget (#1234).
    ///
    /// Already-dequeued `pending` items come first — they are next in line — and
    /// the queue contributes only what fits, in true priority order. A crawl can
    /// never visit more than `max_pages`, so anything beyond that is dead weight
    /// by definition; F-39 measured 4955 queued URLs against 49 visited.
    pub(crate) async fn snapshot_pending_bounded(&self, budget: usize) -> Vec<String> {
        let mut urls: Vec<String> = self.pending.iter().map(|d| d.url.to_string()).collect();
        let remaining = budget.saturating_sub(urls.len());
        urls.extend(self.queue.snapshot_urls_bounded(remaining).await);
        urls.truncate(budget);
        urls
    }

    /// Restore pending (queued-but-unvisited) URLs from a checkpoint.
    ///
    /// Re-enqueues them into the local pending buffer as depth-0 HTML links.
    /// The visited set is restored separately and takes precedence: `next_url`
    /// skips any restored URL that was actually already visited, so a queued
    /// entry that raced into `visited` never gets re-crawled (#517).
    pub(crate) fn restore_pending(&mut self, urls: &[String]) {
        for raw in urls {
            if let Ok(url) = Url::parse(raw) {
                let discovered = DiscoveredUrl::html(url.clone(), 0, url);
                self.pending.push_back(discovered);
            }
        }
    }

    /// Seed the crawl: push the seed onto the discovery queue (highest priority)
    /// and onto the local pending buffer.
    pub(crate) async fn seed(&mut self, seed_url: &Url) {
        let discovered = DiscoveredUrl::html(seed_url.clone(), 0, seed_url.clone());
        self.queue
            .push_prioritized(discovered.clone(), UrlSource::Seed)
            .await;
        self.pending.push_back(discovered);
    }

    /// Drain discovered links from the shared queue into the pending buffer.
    pub(crate) async fn drain_discovered(&mut self) {
        self.pending.append(&mut self.queue.drain_all().await);
    }

    /// Whether there are URLs left to schedule.
    #[must_use]
    pub(crate) fn has_pending_work(&self) -> bool {
        !self.pending.is_empty()
    }

    /// URLs buffered for dispatch — diagnostic payload for a stalled run
    /// (#2313). A stall report without the frontier size cannot tell "nothing
    /// to do" from "plenty to do and no slot to do it in".
    #[must_use]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Effective concurrency limit (autoscale-aware).
    ///
    /// Autoscale levels stay WITHIN the model's `Operation.crawl` ceiling:
    /// Normal → full tier value, Reduced → half, Critical → 0 (pause). No
    /// re-clamp needed — the tier is already ≤ the global ceiling and every
    /// scaled-down result is bounded by it.
    #[must_use]
    pub(crate) fn effective_concurrency(&self) -> usize {
        match self.autoscale_level.as_ref() {
            Some(level) => level.effective_concurrency(self.base_concurrency.get()),
            None => self.base_concurrency.get(),
        }
    }

    /// Whether another task may be spawned given `in_flight` running tasks.
    #[must_use]
    pub(crate) fn can_spawn(&self, in_flight: usize) -> bool {
        in_flight < self.effective_concurrency()
    }

    /// Next URL to crawl, or `None` if at the concurrency limit or out of work.
    ///
    /// Checks the concurrency limit *before* popping (so the pending buffer is
    /// left untouched when no task may spawn), then skips already-visited URLs,
    /// marking the returned URL as visited.
    pub(crate) fn next_url(&mut self, in_flight: usize) -> Option<DiscoveredUrl> {
        if !self.can_spawn(in_flight) {
            return None;
        }
        while let Some(discovered) = self.pending.pop_front() {
            if self.record_visit(discovered.url.as_str()) {
                return Some(discovered);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;

// ============================================================================
// Task 5.1 memory probe — visited_urls string mirror growth (BEFORE numbers).
// Writes one line to WEBFANG_MEMORY_REPORT_PATH; no byte assertions by design
// (Q3 MEASURE FIRST — data decides, not preferences).
// ============================================================================
#[cfg(test)]
mod memory_probe_tests {
    use super::*;
    use crate::infrastructure::observability::memory_probe;

    #[cfg_attr(
        miri,
        ignore = "200k-iteration measurement probe hangs Miri past the lane cap (run 35876002219); no assertions, report-file side effect"
    )]
    #[test]
    fn probe_visited_mirror_growth_200k_urls() {
        const N: usize = 200_000;
        let scheduler =
            CrawlScheduler::new(crate::domain::budget::CrawlConcurrency::new(8).expect("8 > 0"));
        let before = memory_probe::rss_bytes();

        for i in 0..N {
            let url = format!("https://probe.example.com/section-{}/page/{i}", i % 64);
            assert!(scheduler.record_visit(&url), "probe URL must be new");
        }

        let after = memory_probe::rss_bytes();
        let mirror_len = scheduler
            .visited_urls
            .read()
            .map(|urls| urls.len())
            .unwrap_or(usize::MAX);
        assert_eq!(mirror_len, N, "mirror holds exactly one entry per URL");

        memory_probe::append_report(
            "BEFORE — visited_urls mirror",
            &format!(
                "entries={mirror_len} rss_before={} rss_after={} delta={}",
                memory_probe::fmt_rss(before),
                memory_probe::fmt_rss(after),
                memory_probe::fmt_rss(after.and_then(|a| before.map(|b| a.saturating_sub(b)))),
            ),
        );
    }
}
