//! Mechanical guarantees of the T6 harness — the spec's own invariants, not
//! the measurement's verdicts. Per S2 ("no retroactive annexes") NO test
//! asserts the pre-registered prediction or its refutation: the numbers in
//! the report are the numbers from the run, and a refutation is a reportable
//! outcome, not a failure. What IS pinned here: determinism, the floor, the
//! bucket-period assumption (S3), the rule's boundary arithmetic, and the
//! evaluators' hand-computed reference counts.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use super::*;

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

// ---- The synthetic clock drives the production schedule (S3) ---------------

/// The production semantics: the FIRST request of a run is dispatched
/// immediately (its slot is `now`).
#[test]
fn first_request_is_dispatched_immediately() {
    for (base, jitter) in [
        (PERIOD_MS, 0),
        (PERIOD_MS, JITTER_MS),
        (PERIOD_MS + JITTER_MS / 2, 0),
    ] {
        let run = arrivals(base, jitter, 7, REQUESTS);
        assert_eq!(run[0], Duration::ZERO, "first dispatch must be immediate");
    }
}

/// S3's stated assumption, checked: the jitter's cumulative slots are the
/// later of the two pacing layers, so every inter-slot gap is at least the
/// bucket's period (the bucket adds no spacing of its own) — and the schedule
/// floor itself (gap ≥ base) holds in every arm.
#[test]
fn the_floor_and_the_bucket_period_hold_in_every_arm() {
    for (base, jitter) in [
        (PERIOD_MS, 0),
        (PERIOD_MS, JITTER_MS),
        (PERIOD_MS + JITTER_MS / 2, 0),
    ] {
        let run = arrivals(base, jitter, 3, REQUESTS);
        let floor = ms(base);
        let below_floor: Vec<usize> = run
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[1] - pair[0] < floor)
            .map(|(i, _)| i)
            .collect();
        assert!(
            below_floor.is_empty(),
            "gap below the schedule floor {floor:?} at {below_floor:?}"
        );
    }
}

/// A and C are deterministic schedules: identical arrivals for ANY seed
/// (S2 — only arm B varies across seeds).
#[test]
fn deterministic_arms_do_not_vary_with_the_seed() {
    let one = arrivals(PERIOD_MS, 0, 1, REQUESTS);
    let two = arrivals(PERIOD_MS, 0, 2, REQUESTS);
    assert_eq!(one, two, "arm A must not vary with the seed");
    let c_one = arrivals(PERIOD_MS + JITTER_MS / 2, 0, 1, REQUESTS);
    let c_two = arrivals(PERIOD_MS + JITTER_MS / 2, 0, 2, REQUESTS);
    assert_eq!(c_one, c_two, "arm C must not vary with the seed");
}

/// A fixed seed reproduces the jittered arm exactly — the property the whole
/// in-crate harness exists to get (#2749: the binary seeds from OS entropy,
/// so an end-to-end comparison could not fix the seed).
#[test]
fn the_jittered_arm_reproduces_exactly_for_a_fixed_seed() {
    let one = arrivals(PERIOD_MS, JITTER_MS, 42, REQUESTS);
    let again = arrivals(PERIOD_MS, JITTER_MS, 42, REQUESTS);
    assert_eq!(one, again, "same seed must reproduce the same jitter draws");
    let other = arrivals(PERIOD_MS, JITTER_MS, 43, REQUESTS);
    assert_ne!(one, other, "different seeds must draw differently");
}

// ---- The rule's boundary arithmetic (S2) ------------------------------------

/// The tie band is inclusive: |B − C| ≤ 10 ties; a win needs B < C − 10 and
/// a loss B > C + 10.
#[test]
fn the_rule_labels_wins_losses_and_ties() {
    assert!(matches!(seed_outcome(0, 100), SeedOutcome::Win));
    assert!(matches!(seed_outcome(89, 100), SeedOutcome::Win), "89 < 90");
    assert!(
        matches!(seed_outcome(90, 100), SeedOutcome::Tie),
        "the boundary ties"
    );
    assert!(matches!(seed_outcome(95, 100), SeedOutcome::Tie));
    assert!(matches!(seed_outcome(100, 100), SeedOutcome::Tie));
    assert!(
        matches!(seed_outcome(110, 100), SeedOutcome::Tie),
        "the boundary ties"
    );
    assert!(
        matches!(seed_outcome(111, 100), SeedOutcome::Loss),
        "111 > 110"
    );
    assert!(matches!(seed_outcome(200, 0), SeedOutcome::Loss));
}

/// The symmetric verdict needs 8 of 10 seeds in the SAME direction; 7 wins
/// are "no measurable structural difference" by design (p ≈ 0.055).
#[test]
fn the_verdict_needs_eight_of_ten_seeds() {
    assert_eq!(structural_verdict(8, 0), "structural benefit");
    assert_eq!(structural_verdict(10, 0), "structural benefit");
    assert_eq!(structural_verdict(7, 0), NO_DIFFERENCE);
    assert_eq!(structural_verdict(0, 8), "structural damage");
    assert_eq!(structural_verdict(0, 7), NO_DIFFERENCE);
    assert_eq!(structural_verdict(5, 5), NO_DIFFERENCE);
}

// ---- The evaluators' hand-computed reference counts -------------------------

/// Family 1: window 1000 ms, K=2. Arrivals [0, 100, 200, 1000, 1100]:
/// window [0,1000) holds 3 > 2 → the third is penalized; [1000,2000) holds
/// 2 ≤ 2. Exactly 1 penalized.
#[test]
fn fixed_window_counts_the_reference_case() {
    let run = [ms(0), ms(100), ms(200), ms(1000), ms(1100)];
    assert_eq!(fixed_window_counter(&run, ms(1000), 2), 1);
}

/// Family 2: capacity 2, refill 1 token per 100 ms, starts full. Arrivals
/// [0, 10, 20, 30] demand 10 tokens per period where 1 refills: the first
/// two spend the full bucket, the last two find no token. Exactly 2.
#[test]
fn token_bucket_counts_the_reference_case() {
    let run = [ms(0), ms(10), ms(20), ms(30)];
    assert_eq!(token_bucket(&run, 2, ms(100)), 2);
}

/// Family 3: window 300 ms, K=2. Arrivals [0, 100, 200, 300]: the third
/// arrival sees 2 priors inside [a−300, a), the fourth sees 2 again (the
/// arrival at 0 leaves the window). Exactly 2 penalized.
#[test]
fn sliding_window_counts_the_reference_case() {
    let run = [ms(0), ms(100), ms(200), ms(300)];
    assert_eq!(sliding_window(&run, ms(300), 2), 2);
}

/// Family 4: period 100 ms, B=2. Arrivals [0, 100, 110, 220]: the arrival at
/// 100 does NOT see the one at 0 (a gap of exactly one period belongs to the
/// LATER window); the arrival at 110 sees the one at 100; the arrival at 220
/// sees none. Exactly 1 penalized.
#[test]
fn burst_detector_counts_the_reference_case() {
    let run = [ms(0), ms(100), ms(110), ms(220)];
    assert_eq!(burst_detector(&run, ms(100), 2), 1);
}

/// Family 5: a window of 2 gaps, ceiling 0.1. Arrivals [0, 100, 200, 320]:
/// the third arrival's gaps [100, 100] have CV 0; the fourth's gaps
/// [100, 120] have CV ≈ 0.0909. Both at or below the ceiling. Exactly 2
/// (the first 2 arrivals are warmup).
#[test]
fn regularity_detector_counts_the_reference_case() {
    let run = [ms(0), ms(100), ms(200), ms(320)];
    assert_eq!(regularity_detector(&run, 0.1, 2), 2);
}

/// The boundary semantics the reference case above pins: a gap of EXACTLY
/// one period does not put two arrivals in the same trailing window.
#[test]
fn a_gap_of_exactly_one_period_is_not_a_burst() {
    let run = [ms(0), ms(100)];
    assert_eq!(burst_detector(&run, ms(100), 2), 0);
    assert_eq!(sliding_window(&run, ms(100), 2), 0);
}

// ---- The full measurement ----------------------------------------------------

/// The measurement covers every family at every threshold, respects its own
/// accounting (counts within [0, REQUESTS], the seed outcomes sum to the
/// seed count), and is REPRODUCIBLE: the same constants produce the same
/// rows, because every input is a fixed seed (S2). Running it also prints
/// the report (nextest: `--success-output=final`).
#[test]
fn the_full_measurement_covers_every_cell_and_reproduces() {
    let first = measure();
    let expected_cells = FIXED_WINDOW_MAX.len()
        + BUCKET_CAPACITY.len()
        + SLIDING_WINDOW_MAX.len()
        + BURST_SIZES.len()
        + CV_CEILINGS.len();
    assert_eq!(
        first.len(),
        expected_cells,
        "5 families × 3 thresholds = 15 cells"
    );
    for row in &first {
        assert!(
            row.a <= REQUESTS && row.c <= REQUESTS,
            "counts must be within the run"
        );
        assert!(row.b_mean >= 0.0 && row.b_mean <= REQUESTS as f64);
        assert_eq!(
            row.wins + row.losses + row.ties,
            SEEDS.len(),
            "every seed is labeled exactly once"
        );
        assert!(!row.verdict.is_empty());
    }

    let second = measure();
    // FamilyRow derives PartialEq: identical constants must produce
    // bit-identical rows (every input is a fixed seed — S2).
    assert_eq!(first, second, "the measurement must reproduce");

    println!("{}", report(&first));
}
