//! Crawl loop and wait cluster of the crawl engine — the main loop, the
//! bounded wait/dispatch steps and their stall and pause verdicts, extracted
//! verbatim from `engine.rs` (file-size Ola 1, #2584). Inherent methods of
//! [`Engine`]: the child module reaches the engine's private fields, and the
//! module stays private — no re-export is needed because nothing here is
//! part of the crate's public surface.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn, Instrument};

use super::Engine;
use super::{PauseTracker, PauseVerdict, PauseWatchdog, WaitOutcome};
use crate::application::crawler::crawl_scheduler::CrawlScheduler;
use crate::application::crawler::crawl_task::{handle_crawl_result, run_crawl_task};
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::domain::CrawlError;

/// Poll interval while the engine has work it cannot dispatch and no worker in
/// flight to wait on — an autoscale `Critical` pause (#2313).
///
/// The pause is a legitimate state (RAM > 90%): it must not spin a core, and it
/// must not be mistaken for a stall on the first tick. Polling is what makes
/// the difference between "paused" and "wedged" observable at all.
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(50);

impl Engine {
    /// Main crawl loop — process completed tasks, drain discovered links,
    /// checkpoint periodically, spawn new tasks, and wait for work.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlError::Internal`] when the run can no longer make
    /// progress, instead of spinning or waiting forever (#2313):
    ///
    /// - a worker outlives `worker_wait_bound` without completing, or
    /// - there is work to dispatch, no worker in flight to free a slot, and the
    ///   autoscale pause outlives [`PauseWatchdog`]'s give-up threshold.
    ///
    /// A stalled run is a FAILED run. The pages collected so far are not a
    /// crawl result, and reporting one would hide the wedge the way the old
    /// `warn!`-and-re-check did — the loop kept a 240 s hang alive in CI.
    ///
    /// A paused run is NOT a stalled one: `worker_wait_bound` never applies to
    /// the pause, because a deliberate throttle and a hung worker are different
    /// failures with different time constants.
    pub(super) async fn crawl_loop(
        &mut self,
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        task_ctx: &Arc<CrawlTaskCtx>,
        start: std::time::Instant,
        worker_wait_bound: Duration,
        pause: PauseWatchdog,
    ) -> Result<(), CrawlError> {
        let mut tracker = PauseTracker::default();

        loop {
            // Check shutdown signal
            if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Shutdown signal received — saving checkpoint and exiting");
                // Unblock workers parked on rate-limit/governor waits (#509)
                // before saving state; cancel() is idempotent.
                self.cancel_run();
                self.save_checkpoint().await;
                break;
            }

            // Check if we've reached max pages (sin lock - atomic)
            if self.collector.is_full(self.config.max_pages) {
                info!("Reached max pages limit: {}", self.config.max_pages);
                self.cancel_run();
                break;
            }

            // Process completed tasks FIRST (non-blocking)
            self.process_completed_tasks(tasks);

            // Re-check max_pages AFTER processing — concurrent completions may
            // have pushed the collector past the limit while we were spawning.
            // Without this, spawn_available_tasks() below would keep fanning out
            // new workers beyond max_pages (issue #590, bug #1).
            if self.collector.is_full(self.config.max_pages) {
                info!(
                    "Reached max pages limit after processing: {}",
                    self.config.max_pages
                );
                self.cancel_run();
                break;
            }

            // Drain discovered links from the deduplicated UrlQueue
            self.scheduler.drain_discovered().await;

            // Termination: no frontier AND no worker in flight.
            //
            // Checked AFTER the reap and the drain, never before: a completing
            // worker pushes its discoveries onto the shared queue, so "no
            // pending work" is only true once everything that finished has
            // been drained. Deciding to exit before draining drops the last
            // page's links — the busy spin used to hide this by draining the
            // queue thousands of times per fetch (#2313).
            if self.is_drained(tasks) {
                break;
            }

            // Periodic checkpoint save
            self.maybe_save_periodic_checkpoint(start).await;

            // Spawn new tasks capped by the remaining budget (issue #1599):
            // at most `remaining` spawns, so collected + in-flight never
            // exceeds max_pages at any dispatch decision. The tier bound
            // still applies inside `next_url` per iteration, so the
            // effective dispatch is min(tier_slots, remaining) — a no-op
            // while budget is ample. Single-threaded spawn keeps the cap
            // race-free.
            let remaining =
                Self::remaining_budget(self.config.max_pages, self.collector.len(), tasks.len());
            Self::spawn_available_tasks(&mut self.scheduler, tasks, task_ctx, remaining);

            // Nothing was dispatchable this iteration (frontier empty or tier
            // full), so a completion — or a lifted pause — is the only thing
            // that can move the engine forward.
            self.wait_or_stop(tasks, worker_wait_bound, pause, &mut tracker)
                .await?;
        }
        Ok(())
    }

    /// Whether the run has nothing left to do: no frontier and no worker in
    /// flight.
    ///
    /// The `tasks.is_empty()` half is what makes this safe to ask after the
    /// reap and the drain: a worker pushes its discoveries before returning, so
    /// an empty set means every enqueue already happened (#2313).
    fn is_drained(&self, tasks: &tokio::task::JoinSet<Result<(), CrawlError>>) -> bool {
        !self.scheduler.has_pending_work() && tasks.is_empty()
    }

    /// Advance the loop by one wait (#2313): park on an in-flight worker, sleep
    /// out an autoscale pause, or end the run.
    ///
    /// # Errors
    ///
    /// Returns the stall or the pause give-up failure — the two ways a run with
    /// work left can no longer make progress.
    async fn wait_or_stop(
        &mut self,
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        worker_wait_bound: Duration,
        pause: PauseWatchdog,
        tracker: &mut PauseTracker,
    ) -> Result<(), CrawlError> {
        match self.wait_for_task(tasks, worker_wait_bound).await {
            WaitOutcome::Progress => {
                tracker.reset();
                Ok(())
            },
            WaitOutcome::Idle => {
                let paused_for = tracker.elapsed();
                self.wait_out_pause(paused_for, pause, &mut tracker.reported)
                    .await
            },
            WaitOutcome::Stalled => {
                // Unblock the other workers before bailing out: the wedged one
                // ignores this, but the drain should not have to burn its whole
                // bound on workers that would have stopped.
                self.cancel_run();
                Err(self.stalled(worker_wait_bound, tasks.len()))
            },
        }
    }

    /// Apply the pause watchdog to one idle iteration (#2313).
    ///
    /// `Idle` means work is pending and nothing is in flight: only the
    /// autoscale level can move the engine, so the run either waits, reports,
    /// or gives up. Sleeping here instead of looping is what keeps a paused
    /// crawl off a core, and `worker_wait_bound` deliberately has no say: a
    /// deliberate throttle is not a hung worker.
    async fn wait_out_pause(
        &self,
        paused_for: Duration,
        pause: PauseWatchdog,
        reported: &mut bool,
    ) -> Result<(), CrawlError> {
        match pause.verdict(paused_for) {
            PauseVerdict::Wait => {},
            PauseVerdict::Report => {
                if !*reported {
                    *reported = true;
                    warn!(
                        paused_secs = paused_for.as_secs(),
                        pending = self.scheduler.pending_len(),
                        effective_concurrency = self.scheduler.effective_concurrency(),
                        "crawl paused by autoscale past the alert threshold — still waiting"
                    );
                }
            },
            PauseVerdict::GiveUp => {
                self.cancel_run();
                return Err(CrawlError::Internal(format!(
                    "crawl paused: autoscale held the crawl at Critical for {}s without lifting (pending={}) — the pause never recovered",
                    paused_for.as_secs(),
                    self.scheduler.pending_len(),
                )));
            },
        }
        tokio::time::sleep(IDLE_POLL_INTERVAL).await;
        Ok(())
    }

    /// The failure a run reports when a worker outlives `worker_wait_bound`
    /// (#2313).
    ///
    /// The numbers are the diagnosis — in-flight workers and frontier size are
    /// what separates "one page wedged" from "the loop is stuck" — so they
    /// travel with the error instead of only reaching a log line.
    fn stalled(&self, worker_wait_bound: Duration, in_flight: usize) -> CrawlError {
        CrawlError::Internal(format!(
            "crawl stalled: a worker exceeded the {}s wait bound without completing (in_flight={}, pending={})",
            worker_wait_bound.as_secs(),
            in_flight,
            self.scheduler.pending_len(),
        ))
    }

    /// Process completed tasks (non-blocking) and update error counters.
    fn process_completed_tasks(&self, tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>) {
        while let Some(result) = tasks.try_join_next() {
            handle_crawl_result(result, &self.error_count, &self.error_breakdown);
        }
    }

    /// Remaining dispatch budget for one `crawl_loop` iteration (issue #1599).
    ///
    /// Caps new spawns so `collected + in_flight` never exceeds `max_pages`
    /// at any dispatch decision. Saturating arithmetic by construction:
    /// over-budget states clamp to zero instead of underflowing.
    pub(super) fn remaining_budget(max_pages: usize, collected: usize, in_flight: usize) -> usize {
        max_pages.saturating_sub(collected.saturating_add(in_flight))
    }

    /// Spawn new tasks capped by the remaining budget and the
    /// (autoscale-aware) concurrency limit.
    ///
    /// The scheduler checks the tier limit before popping and skips URLs
    /// that were already visited, marking each handed-out URL as visited.
    /// `remaining` decrements per actual spawn and `None` (tier-full or
    /// frontier-empty) stops early, so both bounds hold simultaneously.
    fn spawn_available_tasks(
        scheduler: &mut CrawlScheduler,
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        task_ctx: &Arc<CrawlTaskCtx>,
        mut remaining: usize,
    ) {
        while remaining > 0 {
            match scheduler.next_url(tasks.len()) {
                Some(discovered_url) => {
                    // Spawn task — single Arc clone instead of 18 individual clones
                    let task_ctx = Arc::clone(task_ctx);
                    tasks.spawn(
                        async move { run_crawl_task(task_ctx, discovered_url).await }
                            .in_current_span(),
                    );
                    remaining -= 1;
                },
                None => break,
            }
        }
    }

    /// Wait for one task, bounded so a wedged worker cannot hang the loop
    /// (#509).
    ///
    /// Called after every dispatch attempt, so by construction nothing can be
    /// spawned right now: either the frontier is empty or the tier is full.
    /// Both states have exactly one way out — an in-flight worker finishing.
    ///
    /// The guard used to also require pending work (#2313), which meant a run
    /// whose frontier emptied right after the seed — every `max_depth(0)`
    /// crawl — skipped the wait entirely and busy-spun this loop for the whole
    /// fetch, one core per run. Waiting unconditionally on the in-flight set is
    /// what turns that spin into a park.
    async fn wait_for_task(
        &self,
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        worker_wait_bound: Duration,
    ) -> WaitOutcome {
        if tasks.is_empty() {
            // No worker can complete, so there is nothing to wait ON: only an
            // external state change (the autoscale level) unblocks the loop.
            return WaitOutcome::Idle;
        }

        match tokio::time::timeout(worker_wait_bound, tasks.join_next()).await {
            Ok(Some(result)) => {
                handle_crawl_result(result, &self.error_count, &self.error_breakdown);
                WaitOutcome::Progress
            },
            Ok(None) => WaitOutcome::Progress,
            Err(_elapsed) => {
                warn!(
                    bound_secs = worker_wait_bound.as_secs(),
                    in_flight = tasks.len(),
                    "worker wait exceeded bound — ending the run"
                );
                WaitOutcome::Stalled
            },
        }
    }
}

#[cfg(test)]
#[cfg(not(miri))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests;
