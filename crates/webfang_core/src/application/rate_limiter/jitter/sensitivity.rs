//! T6 — the jitter sensitivity analysis (issue #2863).
//!
//! Test-only measurement of the PRODUCTION jitter schedule's slot arithmetic
//! ([`JitterSchedule::reserve_at`], a pure function of the clock reading)
//! against five model limiters, on a synthetic clock. Per the spec's S3/S4:
//! no sleeps, no network, no wall-clock timing — an analysis of the pacing
//! SHAPE, never a validation of the live gate, and no result transfers to a
//! real site.
//!
//! Spec: `odd/tasks/t6-sensitivity-analysis.md` (it quotes the lab plan's
//! S3/S6/S8 verbatim). The run parameters below are pre-registered there
//! (Log L3) BEFORE any run; changing one invalidates the run and forces a
//! fresh one (S2: no retroactive annexes).
//!
//! The three arms are driven by the same production schedule with different
//! parameters: **A** = fixed delay, **B** = `delay + U[0, J]` (the jitter),
//! **C** = fixed, rate-matched (`delay + J/2`). The structural comparison is
//! **B vs C** (same mean rate, different shape); "B < A while B ties C" is a
//! rate-only benefit, because the jitter always pays the mean-rate cost.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use rand::rngs::StdRng;
use rand::SeedableRng;

use super::JitterSchedule;

// ---- Run parameters (pre-registered, spec Log L3) --------------------------

/// The period every family's windows are sized in: the FIXED arm's spacing.
const PERIOD_MS: u64 = 100;

/// The jitter the B arm pays. 40 ms keeps the jittered arm's coefficient of
/// variation (≈ 0.096) straddling the regularity detector's thresholds.
const JITTER_MS: u64 = 40;

/// Requests per arm per seed (S2).
const REQUESTS: usize = 1_000;

/// The ten fixed seeds (S2) — arm B only; A and C are deterministic.
const SEEDS: [u64; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

/// The tie band in penalized requests per 1.000 (S2): below this difference
/// the two arms are tied.
const TIE_BAND: usize = 10;

/// Seeds needed for a structural verdict (S2): a family shows benefit or
/// damage when B wins or loses in at least 8 of the 10 seeds.
const VERDICT_SEEDS: usize = 8;

/// An arm at or above this fraction of penalized requests is reported as
/// saturated (the falsifier's "where the other arm is not already saturated"
/// clause, pre-registered in the spec's Log L3).
const SATURATION_FRACTION: f64 = 0.9;

/// The regularity detector's rolling window of inter-arrivals (L3).
const CV_WINDOW: usize = 20;

// ---- Family thresholds (S1's table, fixed now, reported for all three) -----

/// Family 1 — fixed-window counter: `K` requests per window of 10 periods.
const FIXED_WINDOW_MAX: [usize; 3] = [8, 10, 12];

/// Family 2 — token bucket: capacity in tokens, refill 1 token per period.
const BUCKET_CAPACITY: [u64; 3] = [6, 10, 14];

/// Family 3 — sliding window: `K` requests in any sliding window of 10 periods.
const SLIDING_WINDOW_MAX: [usize; 3] = [8, 10, 12];

/// Family 4 — burst detector: `B` requests within one period.
const BURST_SIZES: [usize; 3] = [2, 3, 4];

/// Family 5 — regularity detector: inter-arrival CV at or below the ceiling.
const CV_CEILINGS: [f64; 3] = [0.05, 0.10, 0.20];

/// The ten-period window families 1 and 3 are sized in (S1).
fn ten_period_window() -> Duration {
    Duration::from_millis(PERIOD_MS * 10)
}

/// The verdict string for "no measurable structural difference" (S2).
const NO_DIFFERENCE: &str = "no measurable structural difference";

// ---- Arms: the production schedule under three parameterizations ----------

/// The arrivals of `requests` dispatches, relative to the run's anchor,
/// produced by the PRODUCTION schedule's slot arithmetic. The synthetic
/// clock advances to each returned slot (S3): the caller issues the next
/// request exactly when the previous one departs, so the schedule stays
/// cumulative and never re-anchors on an idle `now`.
fn arrivals(base_ms: u64, jitter_ms: u64, seed: u64, requests: usize) -> Vec<Duration> {
    let schedule = JitterSchedule::from_rng(
        Duration::from_millis(base_ms),
        jitter_ms,
        Ok::<StdRng, std::convert::Infallible>(StdRng::seed_from_u64(seed)),
    )
    .expect("a fixed seed cannot fail to initialize the schedule");
    let epoch = tokio::time::Instant::from_std(std::time::Instant::now());
    let mut now = epoch;
    let mut slots = Vec::with_capacity(requests);
    for _ in 0..requests {
        let reservation = schedule.reserve_at(now);
        slots.push(reservation.slot.duration_since(epoch));
        now = reservation.slot;
    }
    slots
}

// ---- The five family evaluators (S1) ---------------------------------------
//
// Every evaluator is a pure function of the arrival instants and reports the
// number of PENALIZED requests. Window semantics are pre-registered (L3):
// fixed windows aligned at the anchor; sliding/burst windows trailing and
// half-open; an arrival exactly at a boundary belongs to the LATER window.

/// Family 1 — fixed-window counter: counts per `[k·W, (k+1)·W)` window and
/// penalizes every request beyond `max_per_window`.
fn fixed_window_counter(arrivals: &[Duration], window: Duration, max_per_window: usize) -> usize {
    let mut window_end = window;
    let mut in_window = 0usize;
    let mut penalized = 0usize;
    for &a in arrivals {
        while a >= window_end {
            window_end += window;
            in_window = 0;
        }
        in_window += 1;
        if in_window > max_per_window {
            penalized += 1;
        }
    }
    penalized
}

/// Family 2 — token bucket: starts full, refills continuously at 1 token per
/// period (fractional accrual), and penalizes an arrival that finds no token.
fn token_bucket(arrivals: &[Duration], capacity: u64, refill_period: Duration) -> usize {
    let period_secs = refill_period.as_secs_f64();
    let mut tokens = capacity as f64;
    let mut last = Duration::ZERO;
    let mut penalized = 0usize;
    for &a in arrivals {
        tokens = (tokens + (a - last).as_secs_f64() / period_secs).min(capacity as f64);
        last = a;
        if tokens >= 1.0 {
            tokens -= 1.0;
        } else {
            penalized += 1;
        }
    }
    penalized
}

/// Family 3 — sliding window: penalizes an arrival when the trailing
/// half-open window `[a−W, a)` already holds `max_in_window` requests.
fn sliding_window(arrivals: &[Duration], window: Duration, max_in_window: usize) -> usize {
    let mut oldest = 0usize;
    let mut penalized = 0usize;
    for (i, &a) in arrivals.iter().enumerate() {
        while a - arrivals[oldest] >= window {
            oldest += 1;
        }
        let in_window = i - oldest;
        if in_window >= max_in_window {
            penalized += 1;
        }
    }
    penalized
}

/// Family 4 — burst detector: penalizes an arrival that makes `burst`
/// requests fall within one trailing half-open period `[a−P, a)`.
fn burst_detector(arrivals: &[Duration], period: Duration, burst: usize) -> usize {
    let mut oldest = 0usize;
    let mut penalized = 0usize;
    for (i, &a) in arrivals.iter().enumerate() {
        while a - arrivals[oldest] >= period {
            oldest += 1;
        }
        let in_period = i - oldest;
        if in_period + 1 >= burst {
            penalized += 1;
        }
    }
    penalized
}

/// Family 5 — regularity detector: penalizes an arrival whose trailing
/// window of `window_len` inter-arrivals has a population CV at or below
/// `cv_ceiling` (traffic too regular to be human). The first `window_len`
/// arrivals are exempt (warmup).
fn regularity_detector(arrivals: &[Duration], cv_ceiling: f64, window_len: usize) -> usize {
    let mut penalized = 0usize;
    for i in window_len..arrivals.len() {
        // The `window_len` gaps ENDING at arrival `i`: `window_len + 1`
        // arrivals, from `i - window_len` through `i` inclusive.
        let gaps: Vec<f64> = arrivals[i - window_len..=i]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).as_secs_f64())
            .collect();
        let mean = gaps.iter().sum::<f64>() / window_len as f64;
        let variance =
            gaps.iter().map(|g| (g - mean) * (g - mean)).sum::<f64>() / window_len as f64;
        let cv = if mean > 0.0 {
            variance.sqrt() / mean
        } else {
            0.0
        };
        if cv <= cv_ceiling {
            penalized += 1;
        }
    }
    penalized
}

// ---- The decision rule (S2, symmetric) --------------------------------------

/// One seed's outcome for the structural comparison B vs C.
enum SeedOutcome {
    /// B < C − tie band.
    Win,
    /// B > C + tie band.
    Loss,
    /// Within the tie band: counts as neither.
    Tie,
}

/// S2: a seed is a win for B when B < C − 10, a loss when B > C + 10, and a
/// tie otherwise (ties count as neither).
fn seed_outcome(b: usize, c: usize) -> SeedOutcome {
    if b + TIE_BAND < c {
        SeedOutcome::Win
    } else if b > c + TIE_BAND {
        SeedOutcome::Loss
    } else {
        SeedOutcome::Tie
    }
}

/// S2 (symmetric): structural benefit at ≥ 8 winning seeds, structural
/// damage at ≥ 8 losing seeds, anything else no measurable difference.
fn structural_verdict(wins: usize, losses: usize) -> &'static str {
    if wins >= VERDICT_SEEDS {
        "structural benefit"
    } else if losses >= VERDICT_SEEDS {
        "structural damage"
    } else {
        NO_DIFFERENCE
    }
}

/// One family at one threshold, measured across the ten seeds.
#[derive(Debug, Clone, PartialEq)]
struct FamilyRow {
    family: &'static str,
    threshold: String,
    /// Arm A's penalized count (deterministic, one run).
    a: usize,
    /// Arm B's mean penalized count over the ten seeds.
    b_mean: f64,
    /// Arm C's penalized count (deterministic, one run).
    c: usize,
    wins: usize,
    losses: usize,
    ties: usize,
    verdict: &'static str,
    /// "B < A while B ≈ C" (S2): the structural outcome is "none" and B's
    /// mean sits below A by more than the tie band — the only benefit is the
    /// lower mean rate the jitter always pays for.
    rate_only: bool,
    /// Saturation flags (L3: ≥ 90% of the arm's requests penalized).
    a_saturated: bool,
    b_saturated: bool,
    c_saturated: bool,
}

/// Runs one (family, threshold) cell: B per seed against the single C.
fn family_row(
    family: &'static str,
    threshold: String,
    a: usize,
    b_per_seed: Vec<usize>,
    c: usize,
) -> FamilyRow {
    let b_mean = b_per_seed.iter().sum::<usize>() as f64 / b_per_seed.len() as f64;
    let mut wins = 0usize;
    let mut losses = 0usize;
    let mut ties = 0usize;
    for &b in &b_per_seed {
        match seed_outcome(b, c) {
            SeedOutcome::Win => wins += 1,
            SeedOutcome::Loss => losses += 1,
            SeedOutcome::Tie => ties += 1,
        }
    }
    let verdict = structural_verdict(wins, losses);
    let rate_only = verdict == NO_DIFFERENCE && b_mean + (TIE_BAND as f64) < a as f64;
    let saturated = |n: usize| n as f64 >= SATURATION_FRACTION * REQUESTS as f64;
    FamilyRow {
        family,
        threshold,
        a,
        b_mean,
        c,
        wins,
        losses,
        ties,
        verdict,
        rate_only,
        a_saturated: saturated(a),
        b_saturated: saturated(b_mean as usize),
        c_saturated: saturated(c),
    }
}

/// The full measurement: five families × three thresholds, ten seeds.
fn measure() -> Vec<FamilyRow> {
    // A and C are deterministic schedules: one run each (S2).
    let a = arrivals(PERIOD_MS, 0, 0, REQUESTS);
    let c = arrivals(PERIOD_MS + JITTER_MS / 2, 0, 0, REQUESTS);
    let b_runs: Vec<Vec<Duration>> = SEEDS
        .iter()
        .map(|&seed| arrivals(PERIOD_MS, JITTER_MS, seed, REQUESTS))
        .collect();
    let b_counts = |count: &dyn Fn(&[Duration]) -> usize| -> Vec<usize> {
        b_runs.iter().map(|run| count(run)).collect()
    };

    let mut rows = Vec::new();
    for &k in &FIXED_WINDOW_MAX {
        let a_count = fixed_window_counter(&a, ten_period_window(), k);
        let c_count = fixed_window_counter(&c, ten_period_window(), k);
        rows.push(family_row(
            "fixed-window counter",
            format!("K={k} per 10-period window"),
            a_count,
            b_counts(&|run| fixed_window_counter(run, ten_period_window(), k)),
            c_count,
        ));
    }
    for &capacity in &BUCKET_CAPACITY {
        let a_count = token_bucket(&a, capacity, Duration::from_millis(PERIOD_MS));
        let c_count = token_bucket(&c, capacity, Duration::from_millis(PERIOD_MS));
        rows.push(family_row(
            "token bucket",
            format!("capacity={capacity}, refill 1/period"),
            a_count,
            b_counts(&|run| token_bucket(run, capacity, Duration::from_millis(PERIOD_MS))),
            c_count,
        ));
    }
    for &k in &SLIDING_WINDOW_MAX {
        let a_count = sliding_window(&a, ten_period_window(), k);
        let c_count = sliding_window(&c, ten_period_window(), k);
        rows.push(family_row(
            "sliding window",
            format!("K={k} in any 10-period window"),
            a_count,
            b_counts(&|run| sliding_window(run, ten_period_window(), k)),
            c_count,
        ));
    }
    for &burst in &BURST_SIZES {
        let a_count = burst_detector(&a, Duration::from_millis(PERIOD_MS), burst);
        let c_count = burst_detector(&c, Duration::from_millis(PERIOD_MS), burst);
        rows.push(family_row(
            "burst detector",
            format!("≥{burst} within one period"),
            a_count,
            b_counts(&|run| burst_detector(run, Duration::from_millis(PERIOD_MS), burst)),
            c_count,
        ));
    }
    for &ceiling in &CV_CEILINGS {
        let a_count = regularity_detector(&a, ceiling, CV_WINDOW);
        let c_count = regularity_detector(&c, ceiling, CV_WINDOW);
        rows.push(family_row(
            "regularity detector",
            format!("CV ≤ {ceiling}"),
            a_count,
            b_counts(&|run| regularity_detector(run, ceiling, CV_WINDOW)),
            c_count,
        ));
    }
    rows
}

// ---- The report -------------------------------------------------------------

/// Renders the measurement as the report's table plus the S4 declarations.
/// The numbers are the numbers from the run (S2); the declarations state what
/// the analysis does NOT show (S4).
fn report(rows: &[FamilyRow]) -> String {
    let mut out = String::new();
    out.push_str("T6 — jitter sensitivity analysis (spec: odd/tasks/t6-sensitivity-analysis.md)\n");
    out.push_str(&format!(
        "run parameters (pre-registered): period=delay={PERIOD_MS}ms  jitter={JITTER_MS}ms  \
         C base={base}ms  seeds={seeds}  requests/arm/seed={REQUESTS}\n",
        base = PERIOD_MS + JITTER_MS / 2,
        seeds = SEEDS.len(),
    ));
    out.push_str(
        "family                threshold                        A    B(mean)    C    \
         W/L/T    verdict                   flags\n",
    );
    for row in rows {
        let mut flags = String::new();
        if row.a_saturated {
            flags.push_str(" A-sat");
        }
        if row.b_saturated {
            flags.push_str(" B-sat");
        }
        if row.c_saturated {
            flags.push_str(" C-sat");
        }
        if row.rate_only {
            flags.push_str(" rate-only");
        }
        out.push_str(&format!(
            "{family:<20} {threshold:<32} {a:>4} {b_mean:>9.1} {c:>4}    \
             {wins}/{losses}/{ties}    {verdict:<24} {flags}\n",
            family = row.family,
            threshold = row.threshold,
            a = row.a,
            b_mean = row.b_mean,
            c = row.c,
            wins = row.wins,
            losses = row.losses,
            ties = row.ties,
            verdict = row.verdict,
            flags = if flags.is_empty() {
                "—"
            } else {
                flags.as_str()
            },
        ));
    }
    out.push_str(
        "Declarations (S4): no result transfers to a real site — the limiter families are \
         models, not observed hosts. This measures the SLOT ARITHMETIC of the production \
         schedule on a synthetic clock, never the end-to-end gate (no permits, no sleep, no \
         network), and it is not validation of the live limiter. The decision rule is a \
         one-sided sign test at p ≈ 0.055: it decides what this report says, it does not \
         assert an effect. The field comparison remains not determined.\n",
    );
    out
}

// The harness's mechanical guarantees (determinism, the floor, the rule's
// boundary arithmetic, the evaluators' reference counts). Named so no file
// sits under a `tests/` directory: check_dead_tests.sh flags any `*/tests/*`
// path that is not reachable from a cargo-registered test target, and the
// src unit-test tree never is (the walk starts only at test targets).
mod mechanical;
