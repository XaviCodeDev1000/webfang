//! The tests of the jitter gate (#2610, #2750).
//!
//! They live in their own file so the schedule itself stays small enough to
//! read in one sitting — and the file-size ratchet keeps it that way.

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

/// #2750: a cancelled reservation that is NOT the latest gives its slot
/// back too, as soon as no later reservation is still outstanding.
///
/// The cancellation order is the real one: the waiters of a cancelled
/// batch wake up in arbitrary order, so the highest id is not the first
/// to be released. The middle reservation's slot used to stay in the
/// schedule, and the next request paid for it.
#[test]
fn cancelled_reservations_leave_no_stranded_slot() {
    let base = Duration::from_millis(100);
    let schedule = JitterSchedule::seeded(base, 50, 19);
    let start = schedule_base();

    // The oldest reservation stays outstanding: it is the one still in
    // flight, and the chain has to be rebuilt around it.
    schedule.reserve_at(start);
    let second = schedule.reserve_at(start);
    let third = schedule.reserve_at(start);

    // The newest and the middle one are cancelled.
    schedule.release(&third);
    schedule.release(&second);

    let next = schedule.reserve_at(start);
    assert_eq!(
        next.slot,
        second.slot,
        "the cancelled reservations kept their slots: the next request was pushed to \
         {next_slot:?} instead of {expected:?} — one extra slot of delay",
        next_slot = next.slot,
        expected = second.slot,
    );
}

/// #2750: a fully cancelled batch leaves the schedule empty.
///
/// The extreme case of the stranded slot: when nothing is outstanding the
/// next request must be dispatched at once, not one schedule-length later.
#[test]
fn a_fully_cancelled_batch_dispatches_the_next_request_at_once() {
    let base = Duration::from_millis(100);
    let schedule = JitterSchedule::seeded(base, 50, 23);
    let start = schedule_base();

    let first = schedule.reserve_at(start);
    let second = schedule.reserve_at(start);
    let third = schedule.reserve_at(start);

    // Released worst-first: no release finds the latest reservation.
    schedule.release(&second);
    schedule.release(&first);
    schedule.release(&third);

    let next = schedule.reserve_at(start);
    assert_eq!(
        next.slot, start,
        "no reservation is outstanding, so the schedule must re-anchor on the \
         caller's instant instead of replaying a cancelled backlog"
    );
}

/// #2750: the chain stays proportional to the requests still in flight.
///
/// A reservation whose slot has already been dispatched can no longer
/// constrain the schedule, so it is dropped: a long run must not grow the
/// schedule with every page it paces.
#[test]
fn the_chain_drops_reservations_already_dispatched() {
    let base = Duration::from_millis(100);
    let schedule = JitterSchedule::seeded(base, 50, 29);
    let start = schedule_base();

    schedule.reserve_at(start);
    schedule.reserve_at(start);
    schedule.reserve_at(start);

    // The limiter was idle long enough for every slot above to pass.
    schedule.reserve_at(start + Duration::from_secs(60));

    let state = lock_schedule(&schedule.state);
    assert_eq!(
        state.outstanding.len(),
        1,
        "the chain must hold only the reservation still in flight, not the \
         history of the run"
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
