//! Checkpoint lifecycle for the crawl engine — builders, save/restore path
//! and periodic saving, extracted verbatim from `engine.rs` (file-size Ola 1,
//! #2584). Inherent methods of [`Engine`]: the child module reaches the
//! engine's private fields, and the module stays private — no re-export is
//! needed because nothing here is part of the crate's public surface.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::RwLock;

use tracing::{debug, info, instrument, warn};

use super::Engine;
use crate::application::crawler::checkpoint::{
    log_checkpoint_save, persist_checkpoint_state, BannedDomain, BincodeCheckpoint, CheckpointPath,
    CheckpointStore, CrawlCheckpoint, CURRENT_CHECKPOINT_VERSION,
};
use crate::application::crawler::crawl_scheduler::CrawlScheduler;
use crate::application::crawler::progress::CrawlProgress;

impl Engine {
    /// Unified persistence — wraps `with_checkpoint` when `PersistenceMode` enables checkpointing.
    ///
    /// `Checkpoint` and `Full` variants configure periodic checkpointing via
    /// `with_checkpoint`; `Disabled` and `Resume` leave checkpoint disabled.
    /// On IO error creating the checkpoint directory, `with_checkpoint` logs
    /// `error!` and disables checkpoint without failing the crawl (CRC32
    /// atomic guarantees preserved).
    pub fn with_persistence(self, mode: crate::domain::persistence::PersistenceMode) -> Self {
        if let Some(cfg) = mode.checkpoint_cfg() {
            self.with_checkpoint(cfg.interval, cfg.dir.clone())
        } else {
            self
        }
    }

    /// Enable checkpoint persistence with the given interval and base directory.
    ///
    /// The checkpoint file is scoped per seed URL
    /// (`crawl_checkpoint_<seed-hash>.json`, F-01) so concurrent `--resume`
    /// runs for different sites sharing one base dir cannot collide.
    /// If the checkpoint directory cannot be created, checkpointing is disabled
    /// and an error is logged — the engine will NOT silently pretend to
    /// checkpoint while every save fails.
    #[instrument(skip(self), fields(interval = interval, base_dir = %base_dir.display()))]
    pub fn with_checkpoint(mut self, interval: u64, base_dir: PathBuf) -> Self {
        let cp_path = CheckpointPath::new(&base_dir);
        if let Err(e) = cp_path.ensure_dir() {
            tracing::error!(
                error = %e,
                path = %base_dir.display(),
                "checkpoint dir creation failed — disabling checkpoint"
            );
            return self;
        }

        let scoped = cp_path.file_for_seed(self.config.seed_url.as_str());
        self.load_checkpoint_state(&scoped);

        self.checkpoint_path = Some(scoped);
        self.checkpoint_interval = interval;
        self
    }

    /// Load the scoped checkpoint into `checkpoint_state`, starting fresh when
    /// absent or corrupt (F-01 scoping keeps concurrent sites disjoint).
    #[instrument(skip(self), fields(path = %path.display()))]
    fn load_checkpoint_state(&mut self, path: &Path) {
        match self.checkpoint_store.load(path) {
            Some(cp) => {
                info!(
                    "Resuming from checkpoint: {} visited, {} pages",
                    cp.visited.len(),
                    cp.pages_crawled
                );
                self.checkpoint_state = Some(cp);
            },
            None => {
                // Same `None` breadth as the session loader: missing, corrupt
                // or discarded-version (the discard warns with the path,
                // #1587) — never claim no file was found.
                warn!("No usable checkpoint found, starting fresh");
                self.checkpoint_state = Some(CrawlCheckpoint::new());
            },
        }
    }

    /// Save the current checkpoint to disk (non-blocking wrapper).
    pub(crate) async fn save_checkpoint(&self) {
        if let Some(path) = &self.checkpoint_path {
            let state = self.build_checkpoint_state().await;
            self.persist_checkpoint(state, path).await;
        }
    }

    /// Snapshot the current engine state into a checkpoint.
    pub(crate) async fn build_checkpoint_state(&self) -> CrawlCheckpoint {
        let visited_set: HashSet<String> = self.scheduler.snapshot_visited();
        let pages = self
            .pages_crawled
            .load(std::sync::atomic::Ordering::Relaxed);
        let banned = self
            .banned_domains
            .read()
            .map(|d| d.clone())
            .unwrap_or_default();
        CrawlCheckpoint {
            visited: visited_set,
            // Bounded by the run's own budget (#1234 / F-39). The frontier is an
            // execution artefact, not a site map: a crawl never visits more than
            // max_pages, so anything past that is dead weight by definition, and an
            // unbounded frontier made every later --resume drain a stale list first
            // because max_pages was never part of the checkpoint.
            queued: self
                .scheduler
                .snapshot_pending_bounded(self.config.max_pages)
                .await,
            pages_crawled: pages,
            banned_domains: banned,
            version: CURRENT_CHECKPOINT_VERSION,
        }
    }

    /// Persist a checkpoint on a blocking thread, logging the outcome.
    async fn persist_checkpoint(&self, state: CrawlCheckpoint, path: &std::path::Path) {
        let outcome =
            persist_checkpoint_state(BincodeCheckpoint::new(), state, path.to_path_buf()).await;
        log_checkpoint_save(outcome);
    }

    /// Load checkpoint state if resuming (visited, queued, banned domains and
    /// the durable page counter — #1843).
    pub(crate) fn restore_checkpoint_state(&mut self) {
        if let Some(cp) = &self.checkpoint_state {
            Self::restore_visited(&mut self.scheduler, &cp.visited);
            Self::restore_queued(&mut self.scheduler, &cp.queued);
            Self::restore_banned(&self.banned_domains, &cp.banned_domains);
            Self::restore_pages_crawled(&self.pages_crawled, cp.pages_crawled);
        }
    }

    /// Restore the visited set from a checkpoint.
    fn restore_visited(scheduler: &mut CrawlScheduler, visited: &HashSet<String>) {
        if !visited.is_empty() {
            scheduler.restore_visited(visited);
            info!("Restored {} visited URLs from checkpoint", visited.len());
        }
    }

    /// Restore the pending frontier from a checkpoint.
    ///
    /// The list was truncated to the run's `max_pages` when it was written
    /// (#1234), so this can never re-inject an unbounded frontier.
    fn restore_queued(scheduler: &mut CrawlScheduler, queued: &[String]) {
        if !queued.is_empty() {
            scheduler.restore_pending(queued);
            info!("Restored {} queued URLs from checkpoint", queued.len());
        }
    }

    /// Restore banned domains from a checkpoint.
    fn restore_banned(banned_domains: &RwLock<Vec<BannedDomain>>, cp_banned: &[BannedDomain]) {
        if !cp_banned.is_empty() {
            if let Ok(mut banned) = banned_domains.write() {
                *banned = cp_banned.to_vec();
            }
            info!(
                "Restored {} banned domains from checkpoint",
                cp_banned.len()
            );
        }
    }

    /// Restore the persisted page counter from a checkpoint (#1843).
    ///
    /// `build_checkpoint_state` snapshots this atomic, so a resumed run that
    /// never restored it rewrote every later checkpoint with only its own
    /// count — regressing the durable total from N to M. The store runs even
    /// for a fresh-start checkpoint (`pages_crawled == 0`): it is a no-op
    /// against the zeroed constructor atomic, and it happens before any
    /// worker spawns, so `Relaxed` suffices. The log stays quiet for the 0
    /// case, matching the sibling restore helpers.
    fn restore_pages_crawled(counter: &AtomicU64, persisted: u64) {
        counter.store(persisted, std::sync::atomic::Ordering::Relaxed);
        if persisted > 0 {
            info!(pages = persisted, "Restored pages_crawled from checkpoint");
        }
    }

    /// Periodically save a checkpoint and emit a structured progress log.
    pub(crate) async fn maybe_save_periodic_checkpoint(&self, start: std::time::Instant) {
        if self.checkpoint_interval == 0 {
            return;
        }
        let pages = self
            .pages_crawled
            .load(std::sync::atomic::Ordering::Relaxed);
        if pages == 0 || !pages.is_multiple_of(self.checkpoint_interval) {
            return;
        }
        debug!("Periodic checkpoint save at {pages} pages");
        self.save_checkpoint().await;

        // Periodic structured progress log (issue #356 Fase 4)
        let progress = CrawlProgress::new(pages, self.config.max_pages, start.elapsed());
        info!(
            pages_crawled = pages,
            max_pages = self.config.max_pages,
            progress_pct = progress.progress_pct(),
            elapsed_secs = start.elapsed().as_secs(),
            pages_per_sec = progress.pages_per_sec(),
            eta_secs = progress.eta_secs(),
            trace_id = %self.correlation_id.trace_id(),
            "crawl progress"
        );
    }
}
