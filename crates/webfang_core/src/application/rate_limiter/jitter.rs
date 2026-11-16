//! The jitter gate of the shared rate limiter (#2610).
//!
//! The pacing floor lives in the limiter; this module owns the RANDOM part
//! of the interval — the cumulative schedule that turns a metronome into a
//! cadence a rate limiter cannot align its counting window with.
//!
//! It is a child module of [`super`] because the schedule is the limiter's
//! private business: only the limiter's own waits consult it.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rand::rngs::StdRng;
use rand::Rng;
// The seeded constructor is test-only, and `SeedableRng` is in scope only
// there: the production build never seeds by hand.
#[cfg(test)]
use rand::SeedableRng;
use tokio_util::sync::CancellationToken;

use super::RateLimitCancelled;
use crate::error::ScraperError;

/// How far ahead a scheduled slot may sit (#2610).
///
/// `Instant` arithmetic panics on overflow and `--jitter-ms` is unbounded
/// (like `--delay-ms`), so a slot advance that cannot be represented falls
/// back to this horizon instead of overflowing.
const SLOT_HORIZON: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// One reserved dispatch slot (#2610).
pub(super) struct Reservation {
    /// The instant the request may be dispatched at.
    pub(super) slot: tokio::time::Instant,
    /// Reservation id — the rollback drops exactly this reservation.
    id: u64,
}

/// Cumulative jitter schedule (#2610) — the metronome breaker.
///
/// Every paced request reserves the NEXT slot of a cumulative schedule, so
/// the gap between two dispatches is `delay_ms + U[0, jitter_ms]`: never
/// below the configured delay, and never the same twice.
///
/// The shape is the whole point, and it is not interchangeable with the two
/// obvious alternatives:
///
/// - A sleep drawn AFTER the permit yields `delay_ms + j_k - j_{k-1}`; the
///   difference of two independent draws dips BELOW the configured delay
///   exactly when the site is most sensitive.
/// - A sleep drawn BEFORE the permit is absorbed by the token bucket and
///   barely moves the cadence.
/// - Only the cumulative schedule keeps the floor AND decorrelates.
///
/// One instance lives per [`SharedRateLimiter`], so one sequence per
/// run/process: two runs never replay the same pattern (#2610).
pub(super) struct JitterSchedule {
    /// Base pacing period — the floor of every interval.
    base: Duration,
    /// Upper bound of the random addition, in milliseconds.
    jitter_ms: u64,
    /// Schedule state. Never held across an `.await`.
    state: Mutex<JitterState>,
}

/// Mutable schedule state — guarded by [`JitterSchedule::state`].
struct JitterState {
    /// The next slot a request may be dispatched at (`None` = none reserved).
    next_slot: Option<tokio::time::Instant>,
    /// Id handed to the next reservation.
    next_reservation: u64,
    /// The reservations still outstanding: id → where the schedule may place
    /// the next reservation once this one has added its advance to the chain.
    ///
    /// The chain is what makes the rollback slot-aware (#2750): a released
    /// reservation is dropped here, and the schedule is recomputed from what
    /// is left, so a cancellation that is not the latest one gives its slot
    /// back instead of leaving it stranded.
    outstanding: BTreeMap<u64, tokio::time::Instant>,
    /// The RNG, seeded from OS entropy per limiter.
    rng: StdRng,
}

impl JitterSchedule {
    /// Build a schedule from an explicit RNG seed source (#2749).
    ///
    /// The OS entropy source cannot be made to fail from a test, so the seed
    /// failure is reached through this seam instead of through the real one.
    /// The CALLER decides what a seed failure means: the limiter keeps the
    /// pacing and drops the randomization.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`] when the seed source fails: a
    /// predictable sequence is worse than no jitter at all.
    pub(super) fn from_rng<E: std::fmt::Display>(
        base: Duration,
        jitter_ms: u64,
        rng: Result<StdRng, E>,
    ) -> Result<Self, ScraperError> {
        let rng = rng.map_err(|e| {
            ScraperError::Config(format!(
                "no se pudo inicializar el generador de jitter: {e}"
            ))
        })?;
        Ok(Self {
            base,
            jitter_ms,
            state: Mutex::new(JitterState {
                next_slot: None,
                next_reservation: 0,
                outstanding: BTreeMap::new(),
                rng,
            }),
        })
    }

    /// Seeded schedule — deterministic sequences for tests (#2610).
    #[cfg(test)]
    fn seeded(base: Duration, jitter_ms: u64, seed: u64) -> Self {
        Self {
            base,
            jitter_ms,
            state: Mutex::new(JitterState {
                next_slot: None,
                next_reservation: 0,
                outstanding: BTreeMap::new(),
                rng: StdRng::seed_from_u64(seed),
            }),
        }
    }

    /// Reserve the next dispatch slot.
    ///
    /// Pure with respect to time: `now` is the caller's clock reading, so
    /// the schedule is verifiable without sleeping.
    pub(super) fn reserve_at(&self, now: tokio::time::Instant) -> Reservation {
        let mut state = lock_schedule(&self.state);
        // A chain entry whose end is already in the past can no longer
        // constrain the schedule, so it is dropped: the map stays
        // proportional to the requests still in flight, not to the length of
        // the run.
        state.outstanding.retain(|_, end| *end > now);
        // A slot in the past means the limiter was idle: the schedule
        // re-anchors on `now` instead of replaying a backlog.
        let slot = match state.next_slot {
            Some(next) if next > now => next,
            _ => now,
        };
        let extra = Duration::from_millis(state.rng.random_range(0..=self.jitter_ms));
        let advance = self.base.saturating_add(extra);
        let end = slot_after(slot, advance, now);
        state.next_slot = Some(end);
        let id = state.next_reservation;
        state.next_reservation += 1;
        state.outstanding.insert(id, end);
        Reservation { slot, id }
    }

    /// Give a cancelled reservation's slot back to the schedule.
    ///
    /// A cancelled request never reached the network, so its slot must not
    /// keep the next request waiting. The rollback is slot-aware (#2750): the
    /// cancelled reservation is dropped from the chain and the schedule is
    /// recomputed from what is still outstanding, so a cancellation that is
    /// not the latest one gives its slot back too — the waiters of a
    /// cancelled batch wake up in arbitrary order, and with a latest-only
    /// rollback every cancellation but one left its slot behind.
    ///
    /// What is NOT reclaimed is a slot a later reservation was already built
    /// on top of: those waiters hold absolute deadlines, so pulling the
    /// schedule back under them would let a later request overtake an earlier
    /// one. The schedule only collapses once nothing is outstanding.
    pub(super) fn release(&self, reservation: &Reservation) {
        let mut state = lock_schedule(&self.state);
        state.outstanding.remove(&reservation.id);
        state.next_slot = state.outstanding.values().copied().max();
    }
}

/// The instant the schedule may place the next reservation at, once the
/// reservation at `slot` has added `advance` to the chain (#2610).
///
/// `Instant` arithmetic panics on overflow and `--jitter-ms` is unbounded
/// (like `--delay-ms`), so an advance that cannot be represented falls back
/// to the horizon instead of overflowing. `now` is the caller's clock
/// reading, so the fallback stays deterministic for the caller.
fn slot_after(
    slot: tokio::time::Instant,
    advance: Duration,
    now: tokio::time::Instant,
) -> tokio::time::Instant {
    slot.checked_add(advance)
        .or_else(|| now.checked_add(SLOT_HORIZON))
        .unwrap_or(now)
}

/// Lock the schedule state, recovering from poisoning.
///
/// A poisoned mutex is never fatal and never silent: the guard still grants
/// exclusive access, so the schedule keeps its invariants.
fn lock_schedule(state: &Mutex<JitterState>) -> MutexGuard<'_, JitterState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl super::SharedRateLimiter {
    /// Wait for the jitter slot, if this run asked for one (#2610).
    ///
    /// The FIRST request of a run is dispatched immediately (its slot is
    /// `now`); every later one waits for the cumulative slot, so the burst
    /// of the token bucket is NOT a burst on the wire. A run of N requests
    /// therefore takes about `(N-1) * (delay + jitter/2)`.
    pub(super) async fn await_jitter_turn(&self) {
        let Some(schedule) = self.jitter.as_ref() else {
            return;
        };
        let reservation = schedule.reserve_at(tokio::time::Instant::now());
        tokio::time::sleep_until(reservation.slot).await;
    }

    /// Wait for the jitter slot, abandoning it if `cancel` fires first
    /// (#2610).
    ///
    /// A cancelled wait releases its reserved slot: the request never
    /// reached the network, so the slot must not keep the next one waiting.
    pub(super) async fn await_jitter_turn_or_cancel(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), RateLimitCancelled> {
        let Some(schedule) = self.jitter.as_ref() else {
            return Ok(());
        };
        let reservation = schedule.reserve_at(tokio::time::Instant::now());
        tokio::select! {
            () = tokio::time::sleep_until(reservation.slot) => Ok(()),
            () = cancel.cancelled() => {
                schedule.release(&reservation);
                Err(RateLimitCancelled)
            },
        }
    }
}

#[cfg(all(test, not(miri)))]
mod sensitivity;
#[cfg(all(test, not(miri)))]
mod tests;
