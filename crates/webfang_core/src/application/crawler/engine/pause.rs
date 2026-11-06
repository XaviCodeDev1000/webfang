//! Pause primitives for the crawl loop (#2313, #2338).
//!
//! Extracted from `engine.rs` to keep that module within the file-size
//! ratchet. The crawl loop in `engine.rs` consumes these via
//! `pub use pause::{..}`.

use std::time::Duration;

use crate::domain::CrawlerConfig;

/// Why one bounded wait in [`Engine::crawl_loop`] returned (#2313).
///
/// The distinction is the whole point of the fix: a wait that produced a
/// completion is progress, a wait with nothing to await is an autoscale pause,
/// and a wait that ran out with a worker still in flight is a stall the run
/// must not survive silently.
pub enum WaitOutcome {
    /// A worker completed (or the task set emptied) — engine state moved.
    Progress,
    /// Nothing in flight and nothing dispatchable: only an external state
    /// change (the autoscale level) can unblock the loop.
    Idle,
    /// A worker outlived `worker_wait_bound` without completing.
    Stalled,
}

/// What to do about a deliberate autoscale pause that has lasted `paused_for`.
pub enum PauseVerdict {
    /// Short pause — keep waiting silently.
    Wait,
    /// Long pause — report it once as a distinct event, keep waiting.
    Report,
    /// The pause outlived every plausible recovery — give up on the run.
    GiveUp,
}

/// Wall-clock thresholds for a DELIBERATE pause (#2313).
///
/// Deliberately separate from `worker_wait_bound`: that one bounds a WORKER
/// that should have finished, this one bounds a THROTTLE that is working as
/// designed. Judging both with one deadline is the bug — a RAM-pressure pause
/// is not a hung fetch, and killing a healthy crawl because the machine is at
/// 92% RAM is worse than waiting.
///
/// `Idle` is the only state this applies to, and it is typed rather than
/// inferred: the loop can only reach it with work pending, no worker in
/// flight, and `effective_concurrency() == 0`, which `CrawlConcurrency`'s
/// non-zero invariant makes reachable only through `ConcurrencyLevel::Critical`.
#[derive(Debug, Clone, Copy)]
pub struct PauseWatchdog {
    /// Emit the prolonged-pause event after this much paused time.
    pub alert_after: Duration,
    /// Give up on the run after this much paused time.
    pub give_up_after: Duration,
}

impl PauseWatchdog {
    /// Derive the pause thresholds from the operator configuration (#2338):
    /// a legitimately longer RAM-pressure ride-out raises
    /// `pause_give_up_after_secs` instead of losing the crawl to a hardcoded
    /// 30-minute ceiling. The defaults (300/1800) live on `CrawlerConfig`;
    /// the 30-min give-up ceiling matches the `global-timeout` of the
    /// `default` nextest profile, so a run that would be killed by its own
    /// harness anyway reports the reason first.
    pub fn from_config(config: &CrawlerConfig) -> Self {
        Self {
            alert_after: Duration::from_secs(config.pause_alert_after_secs),
            give_up_after: Duration::from_secs(config.pause_give_up_after_secs),
        }
    }

    /// Classify a pause by how long it has lasted.
    pub fn verdict(&self, paused_for: Duration) -> PauseVerdict {
        if paused_for >= self.give_up_after {
            PauseVerdict::GiveUp
        } else if paused_for >= self.alert_after {
            PauseVerdict::Report
        } else {
            PauseVerdict::Wait
        }
    }
}

/// One no-progress stretch of the crawl loop (#2313).
///
/// Kept as a single value so the loop's control flow does not have to carry two
/// correlated fields — when the stretch started and whether it was reported —
/// through every iteration.
#[derive(Debug, Default)]
pub struct PauseTracker {
    /// When the current stretch started; `None` while workers are in flight.
    since: Option<std::time::Instant>,
    /// Whether the prolonged-pause event already fired for this stretch.
    pub reported: bool,
}

impl PauseTracker {
    /// How long the engine has been unable to dispatch anything.
    pub fn elapsed(&mut self) -> Duration {
        self.since
            .get_or_insert_with(std::time::Instant::now)
            .elapsed()
    }

    /// A worker completed: the stretch is over.
    pub fn reset(&mut self) {
        self.since = None;
        self.reported = false;
    }
}
