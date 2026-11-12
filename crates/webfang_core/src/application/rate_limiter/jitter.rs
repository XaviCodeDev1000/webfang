//! The jitter gate of the shared rate limiter (#2610).
//!
//! The pacing floor lives in the limiter; this module owns the RANDOM part
//! of the interval — the cumulative schedule that turns a metronome into a
//! cadence a rate limiter cannot align its counting window with.
//!
//! It is a child module of [`super`] because the schedule is the limiter's
//! private business: only the limiter's own waits consult it.

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
    /// Reservation id — the rollback only fires for the latest reservation.
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
    /// Id of the latest reservation.
    last_reservation: u64,
    /// Id handed to the next reservation.
    next_reservation: u64,
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
                last_reservation: 0,
                next_reservation: 0,
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
                last_reservation: 0,
                next_reservation: 0,
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
        // A slot in the past means the limiter was idle: the schedule
        // re-anchors on `now` instead of replaying a backlog.
        let slot = match state.next_slot {
            Some(next) if next > now => next,
            _ => now,
        };
        let extra = Duration::from_millis(state.rng.random_range(0..=self.jitter_ms));
        let advance = self.base.saturating_add(extra);
        state.next_slot = Some(
            slot.checked_add(advance)
                .or_else(|| now.checked_add(SLOT_HORIZON))
                .unwrap_or(now),
        );
        let id = state.next_reservation;
        state.next_reservation += 1;
        state.last_reservation = id;
        Reservation { slot, id }
    }

    /// Give a cancelled reservation's slot back to the schedule.
    ///
    /// A cancelled request never reached the network, so its slot must not
    /// keep the next request waiting. Only the LATEST reservation may be
    /// released: a later reservation already built its own slot on top of
    /// this one, and reclaiming it would reorder the schedule.
    pub(super) fn release(&self, reservation: &Reservation) {
        let mut state = lock_schedule(&self.state);
        if state.last_reservation == reservation.id {
            state.next_slot = Some(reservation.slot);
        }
    }
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
mod tests {
    use super::super::{RateLimiterConfig, SharedRateLimiter};
    use super::*;

    // ============================================================================
    // #2610: jitter pacing — the metronome breaker
    // ============================================================================

    /// A base instant for the pure schedule tests — no runtime and no
    /// sleeping, so the schedule is verified as arithmetic.
    fn schedule_base() -> tokio::time::Instant {
        tokio::time::Instant::from_std(std::time::Instant::now())
    }

    /// Every gap the schedule produces stays inside `[base, base + jitter]`
    /// (#2610, AC1).
    ///
    /// The floor is the whole contract: a jittered run must never dispatch
    /// FASTER than the configured delay, and never slower than the delay
    /// plus the jitter it asked for.
    #[test]
    fn jitter_slots_stay_inside_the_configured_range() {
        let base = Duration::from_millis(100);
        let jitter_ms = 50;
        let schedule = JitterSchedule::seeded(base, jitter_ms, 42);
        let start = schedule_base();

        let mut previous: Option<tokio::time::Instant> = None;
        for _ in 0..50 {
            let slot = schedule.reserve_at(start).slot;
            if let Some(previous) = previous {
                let gap = slot - previous;
                assert!(
                    gap >= base && gap <= base + Duration::from_millis(jitter_ms),
                    "gap {gap:?} outside [{base:?}, {:?}]",
                    base + Duration::from_millis(jitter_ms)
                );
            }
            previous = Some(slot);
        }
    }

    /// The same seed replays the same sequence; a different seed does not
    /// (#2610: reproducible in tests, unpredictable between runs).
    #[test]
    fn jitter_sequence_is_reproducible_for_a_seed() {
        let base = Duration::from_millis(100);
        // ONE anchor for both draws: the schedule is a function of the
        // instant it is asked about, so the anchor has to be shared for the
        // comparison to be about the RNG and nothing else.
        let start = schedule_base();
        let draw = |seed: u64| {
            let schedule = JitterSchedule::seeded(base, 500, seed);
            (0..20)
                .map(|_| schedule.reserve_at(start).slot)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            draw(7),
            draw(7),
            "the same seed must replay the same schedule"
        );
        assert_ne!(
            draw(7),
            draw(8),
            "different seeds must not replay the same schedule"
        );
    }

    /// The gate covers the FIRST request too (#2610): the token bucket's burst
    /// is not a burst on the wire.
    #[test]
    fn jitter_gate_covers_the_first_request() {
        let base = Duration::from_millis(100);
        let schedule = JitterSchedule::seeded(base, 50, 3);
        let start = schedule_base();

        let first = schedule.reserve_at(start).slot;
        assert_eq!(first, start, "the first request is dispatched immediately");

        let second = schedule.reserve_at(start).slot;
        assert!(
            second >= start + base,
            "the second request must wait at least the base delay, got {:?}",
            second - start
        );
    }

    /// A cancelled reservation gives its slot back (#2610): the request never
    /// reached the network, so the next one must not wait for a slot nobody
    /// used.
    #[test]
    fn cancelled_reservation_releases_its_slot() {
        let base = Duration::from_millis(100);
        let schedule = JitterSchedule::seeded(base, 50, 11);
        let start = schedule_base();

        let cancelled = schedule.reserve_at(start);
        schedule.release(&cancelled);

        let next = schedule.reserve_at(start);
        assert_eq!(
            next.slot, cancelled.slot,
            "the released slot must be available again"
        );
    }

    /// A release never steals a slot a LATER reservation already built on top
    /// of (#2610): the schedule stays monotonic.
    #[test]
    fn release_does_not_steal_a_later_reservation() {
        let base = Duration::from_millis(100);
        let schedule = JitterSchedule::seeded(base, 50, 13);
        let start = schedule_base();

        let first = schedule.reserve_at(start);
        let second = schedule.reserve_at(start);
        schedule.release(&first);

        let third = schedule.reserve_at(start);
        assert!(
            third.slot >= second.slot + base,
            "the later reservation keeps the schedule in order: {:?} then {:?}",
            second.slot,
            third.slot
        );
    }

    /// An idle limiter re-anchors on the current instant instead of
    /// replaying a backlog (#2610).
    #[test]
    fn idle_limiter_reanchors_instead_of_replaying_a_backlog() {
        let base = Duration::from_millis(100);
        let schedule = JitterSchedule::seeded(base, 50, 17);
        let start = schedule_base();

        schedule.reserve_at(start);
        let much_later = start + Duration::from_secs(60);
        let slot = schedule.reserve_at(much_later).slot;
        assert_eq!(slot, much_later, "an idle limiter dispatches immediately");
    }

    /// `jitter_ms == 0` builds NO gate: the token bucket stays the only
    /// pacing mechanism, burst included (#2610, AC3).
    #[test]
    fn jitter_zero_builds_no_gate() {
        let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(100, 5)).unwrap();
        assert!(limiter.jitter.is_none(), "no jitter gate without jitter");

        let jittered =
            SharedRateLimiter::new(&RateLimiterConfig::new(100, 5).with_jitter(250)).unwrap();
        assert!(
            jittered.jitter.is_some(),
            "the gate exists when the jitter is on"
        );
    }

    /// The first request of a jittered run is NOT delayed (#2610): the
    /// schedule starts at `now`, it does not push the first page back.
    #[tokio::test]
    async fn jitter_never_delays_the_first_request() {
        let delay = Duration::from_millis(500);
        let limiter = SharedRateLimiter::new(
            &RateLimiterConfig::new(delay.as_millis() as u64, 1).with_jitter(5_000),
        )
        .unwrap();

        let started = std::time::Instant::now();
        limiter.until_ready().await;
        let first = started.elapsed();

        assert!(
            first < delay,
            "the first request waited {first:?} — the jitter delayed it"
        );
    }

    /// N jittered requests never dispatch faster than the configured delay
    /// (#2610, AC1).
    ///
    /// The floor is what this can prove without flaking: the schedule's slots
    /// are absolute, so the elapsed total is bounded below by
    /// `(N-1) * delay` no matter how late a wake-up lands. The upper bound
    /// and the spread are proven deterministically by the slot tests above.
    #[tokio::test]
    async fn jittered_pacing_never_dispatches_below_the_base_delay() {
        let delay = Duration::from_millis(40);
        let limiter = SharedRateLimiter::new(
            &RateLimiterConfig::new(delay.as_millis() as u64, 1).with_jitter(20),
        )
        .unwrap();

        let start = std::time::Instant::now();
        for _ in 0..5 {
            limiter.until_ready().await;
        }
        let elapsed = start.elapsed();

        assert!(
            elapsed >= 4 * delay,
            "5 jittered requests took {elapsed:?} — the pacing floor was broken"
        );
    }
}
