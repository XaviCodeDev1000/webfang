//! Bounded dedup-cache machinery for the asset [`Downloader`]: insertion-order
//! ledger, FIFO eviction, in-flight RAII guard, and the capacity derivation from
//! the budget model. Split desde `adapters/downloader/mod.rs` bajo la regla
//! ~500 (#2584, Ola 1); movimiento mecanico.

use std::collections::{HashSet, VecDeque};

use dashmap::DashMap;

/// Entries retained per Asset-tier permit in the dedup cache.
pub const ASSET_CACHE_ENTRIES_PER_PERMIT: usize = 8_192;

/// Asset-tier permits assumed by [`DEFAULT_ASSET_CACHE_CAPACITY`].
///
/// Mirrors the budget model's default asset concurrency (3), so the default
/// bound is the same number the composition roots derive explicitly instead
/// of a second, invented constant.
pub const DEFAULT_ASSET_TIER_PERMITS: usize = 3;

/// Finite default dedup-cache bound for [`Downloader::new`](super::Downloader::new).
///
/// `Downloader::new` used to take the unbounded legacy path (`usize::MAX`), so
/// any caller that did not go through a composition root grew the registry
/// linearly with the crawl. A finite default makes the bound a property of the
/// type; explicit overrides still go through
/// [`Downloader::with_asset_cache_capacity`](super::Downloader::with_asset_cache_capacity).
pub const DEFAULT_ASSET_CACHE_CAPACITY: usize =
    ASSET_CACHE_ENTRIES_PER_PERMIT * DEFAULT_ASSET_TIER_PERMITS;

/// Derive the bounded asset-cache capacity from the budget model's Asset
/// tier. Single source of truth with every other derived bound.
#[must_use]
pub fn asset_cache_capacity(asset_tier_permits: usize) -> usize {
    asset_tier_permits
        .saturating_mul(ASSET_CACHE_ENTRIES_PER_PERMIT)
        .max(ASSET_CACHE_ENTRIES_PER_PERMIT)
}

/// Insertion-order ledger for the bounded dedup cache. A `HashSet` makes
/// membership O(1) so every new cache insert stays O(1) under the single
/// ledger mutex instead of scanning the whole FIFO queue.
#[derive(Default)]
pub(crate) struct InsertionLedger {
    queue: VecDeque<String>,
    seen: HashSet<String>,
}

impl InsertionLedger {
    pub(crate) fn push_new(&mut self, key: String) {
        if self.seen.insert(key.clone()) {
            self.queue.push_back(key);
        }
    }

    /// Rotate an entry (still tracked in `seen`) back to the tail.
    pub(crate) fn requeue(&mut self, key: String) {
        self.queue.push_back(key);
    }

    pub(crate) fn pop_oldest(&mut self) -> Option<String> {
        self.queue.pop_front()
    }

    /// Drop membership tracking for an entry that left the cache.
    pub(crate) fn forget(&mut self, key: &str) {
        self.seen.remove(key);
    }

    pub(crate) fn len(&self) -> usize {
        self.queue.len()
    }
}

/// Lifecycle class of a candidate cache cell during eviction.
pub(crate) enum CellState {
    /// Holds a successful `DownloadedAsset`.
    Initialized,
    /// Empty but a download is executing for this URL right now.
    InFlight,
    /// Empty with no active download: a permanent-failure leftover.
    Abandoned,
    /// Key already removed from the map (evicted elsewhere).
    Gone,
}

/// RAII marker for a URL whose download is actively executing. Inserted
/// before `get_or_try_init` and removed on drop — including tokio task
/// cancellation (#509), which drops the future and therefore the guard,
/// so a cancelled download can never leave a stale in-flight marker.
pub(crate) struct InFlightGuard<'a> {
    map: &'a DashMap<String, ()>,
    url: String,
}

impl<'a> InFlightGuard<'a> {
    pub(crate) fn new(map: &'a DashMap<String, ()>, url: &str) -> Self {
        map.insert(url.to_string(), ());
        Self {
            map,
            url: url.to_string(),
        }
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.map.remove(&self.url);
    }
}
