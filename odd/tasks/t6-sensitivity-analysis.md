# Feature: T6 — jitter sensitivity analysis (five limiter families, three arms, ten seeds)

**Issue:** #2863 (`status:approved` applied 2026-11-16 on the maintainer's explicit instruction)
**Branch:** `test/t6-jitter-sensitivity`
**Worktree:** `~/Projects/Rust/webfang-worktrees/test-t6-jitter-sensitivity`
**Delivery:** one PR on `main`; harness code is test-only, in-crate (`#[cfg(test)]`), no production change. The lab's plan lives on `test/local-lab` (`odd/tasks/local-lab-plan.md`); the normative fragments are quoted here verbatim so this document is self-contained.
**Runner:** `cargo nextest run <harness>`, `bash scripts/ci_fast_gate.sh`
**Origin of the spec:** the local-laboratory plan (branch `test/local-lab`, commit `d02a18c7` — the S3/S6 amendment committed BEFORE any comparative run, per the maintainer's L5 corrections a–e).

## Specs

### S1 — The comparison is pre-registered (from the lab's S3, verbatim)

> "El adversario puede dar la respuesta que tú le pongas […] Lo honesto es reformularlo como
> **análisis de sensibilidad**: qué familias de limiter hacen que el pacing importe y cuáles no."

Five limiter families, implemented as local evaluators: a fixed-window counter, a token
bucket (mean rate), a sliding window, a burst detector, and an inter-arrival regularity
detector.

**Pre-registered prediction (written before any run).**

> "Mi predicción, que debes escribir **antes** de ejecutar: contra los tres primeros, el jitter
> no ayuda y solo baja la tasa media (`delay + J/2`); contra detectores de ráfaga o regularidad,
> sí."

**Rate-matched control arms.**

| arm | pacing | mean rate |
|---|---|---|
| A — fixed | `delay` | `delay` |
| B — jittered | `delay + U[0, J]` | `delay + J/2` |
| C — fixed, rate-matched | `delay + J/2` | `delay + J/2` |

The structural effect of the jitter is **B vs C** (same mean rate, different shape). "B ≈ C"
in the first three families is the **expected** outcome, not a guaranteed one: B has variance,
so against a counting limiter whose threshold sits just above C's regular rate, some of B's
windows exceed it. **The jitter can hurt.** The decision rule is therefore symmetric (S2).

**Threshold sweep (fixed now, reported for all three).**

| family | thresholds (fixed now) |
|---|---|
| 1 — fixed-window counter | window = 10 periods; K ∈ {8, 10, 12} requests per window |
| 2 — token bucket | capacity ∈ {6, 10, 14} tokens; refill = 1 token per period |
| 3 — sliding window | window = 10 periods; K ∈ {8, 10, 12} requests in any sliding window |
| 4 — burst detector | ≥ B ∈ {2, 3, 4} requests within one period |
| 5 — regularity detector | inter-arrival coefficient of variation ≤ {0.05, 0.10, 0.20} |

If the conclusion changes with the threshold, the family is reported as
**threshold-dependent**, with the per-threshold numbers, never summarized by its most
flattering threshold.

**Falsifier:** the prediction is refuted by B < C in any of families 1–3, or by B ≈ C in
either of families 4–5, at a threshold where the other arm is not already saturated.

### S2 — The decision rule (from the lab's S6, verbatim)

- *Seeds*: **10 fixed seeds** per arm. The jitter RNG is seedable only in-crate
  (`SharedRateLimiter::new_with_rng` / `JitterSchedule::from_rng` are module-private); the
  binary seeds from OS entropy, so an end-to-end comparison could not fix the seed — hence the
  in-crate harness.
- *Sample*: **1.000 requests per arm per seed**, generated on a synthetic clock (S4).
- *What varies*: **only arm B**. A and C are deterministic schedules, so their arrivals are
  identical across seeds and repetitions. The comparison is B(seed_i) against the single
  deterministic C.
- *"≈"*: the absolute difference in penalized requests per 1.000 is **≤ 10** (one percent of
  the arm's requests). Below that threshold the two arms are **tied**.
- *Counting*: a seed is a **win** for B when B < C − 10, a **loss** when B > C + 10, and a
  **tie** otherwise. Ties count as neither and are reported: a family where every seed ties is
  "no measurable difference".
- *Decision (symmetric)*: a family shows a **structural benefit** when B wins in ≥ 8 of the
  10 seeds, and **structural damage** when B loses in ≥ 8 of the 10 seeds. Anything else is
  reported as "no measurable structural difference". A **rate-only** benefit is B < A while
  B ≈ C. Both results are reported with the mean-rate cost, because the jitter always pays it.
- *No retroactive annexes*: the numbers in the report are the numbers from the run; if the
  harness changes, the run is repeated from scratch and the previous one is discarded.

### S3 — Synthetic time, not the wall clock (from the lab's S6, verbatim)

The two pacing layers use different clocks: the jitter gate reads **tokio's** clock
(`tokio::time::Instant` and `tokio::time::sleep_until` in `rate_limiter/jitter.rs`), so it *is*
pausable; the token bucket is **governor's own** clock, so it is **not** pausable by
`tokio::time::pause`; the repository's `MockClock` implements the **domain's** `Clock` port
(`domain/clock.rs`), not governor's `governor::clock::Clock`, so it cannot stand in for the
bucket either.

The analysis therefore does not sleep at all: the arrival instants come from the **production
jitter schedule's own slot arithmetic**, which is a pure function of the clock reading
(`reserve_at(now)` takes `now` as a parameter). The synthetic clock advances by the amounts
the schedule returns, and the family evaluators consume the resulting instants. The token
bucket's role is to grant the permits; in steady state the jitter's cumulative slots are the
later of the two, so the bucket adds no spacing of its own — a stated assumption, checked by
comparing the schedule's slots against the bucket's period.

### S4 — What the analysis will NOT show (from the lab's S8, verbatim)

- no laboratory result transfers to any real site — the limiter families are models, not the
  observed behavior of any host;
- the laboratory does not tell whether `--jitter-ms` helps against any host; the field
  comparison remains **not determined**;
- **the sensitivity analysis measures the SLOT ARITHMETIC, not the end-to-end gate.** Its
  arrival instants come from the production schedule's own `reserve_at` — the cumulative slot
  arithmetic — evaluated on a synthetic clock. It does **not** exercise the gate end to end
  (the token bucket's permits, the real sleep, the network). It is therefore an analysis of
  the pacing *shape* the limiter produces, and it must never be read as validation of the
  live limiter;
- the comparison's decision rule ("≥ 8 of 10 seeds") is a **one-sided sign test at p ≈ 0.055**
  (P(X ≥ 8 | p = 0.5, n = 10) = 56/1024): it **decides what the report says**, it does not
  assert an effect. No laboratory conclusion is a statistical claim, and none of them is a
  claim about a real site.

### S5 — Authorized scope and checks

- New test-only code inside `crates/webfang_core/src/application/rate_limiter/` (module-private
  access required for the seeded RNG). No production behavior change; `Risk: none` expected,
  verified by the existing suite staying green.
- Every file ≤ ~500 lines (repo ratchet); the harness may split into harness + evaluators
  modules.
- Checks: `cargo nextest run` (new harness + full `rate_limiter` unit tests), full pre-commit
  gate, `bash scripts/ci_fast_gate.sh` with the coverage report read, `docs/test-inventory.json`
  regenerated in the same commit if the guard drifts.

## Tasks

| id | task | specs | route | status | commit |
|---|---|---|---|---|---|
| T6.1 | Harness: synthetic clock + the three arms (A/B/C) driven by the production `reserve_at`, seeded in-crate | S1, S2, S3 | inline | **done** — 13/13 mechanical tests green | (this commit) |
| T6.2 | The five family evaluators with their three fixed thresholds each | S1 | inline | **done** — hand-computed reference cases pinned | (this commit) |
| T6.3 | The measurement run: 10 seeds × 1.000 requests, the per-family/per-threshold verdicts under the S2 rule, the S8 declarations | S2, S4 | inline | **done** — report captured (see L4) | (this commit) |
| T6.4 | Delivery: issue linkage, gates, PR | S5 | inline | pending | — |

## Log

- **L1** (2026-11-16, user request verbatim): «adelante con estos dos tests, luego T6» — after
  the audit that showed the lab's production value already landed via #2803 and only the T5
  tests + the T6 run remained on `test/local-lab`. T6 is the last piece of the lab that can
  produce new knowledge.
- **L2** (2026-11-16): T6 is a measurement program, not a behavior change — the deliverable is
  the harness (test code) + the report (numbers + per-family verdicts + the S8 declarations).
  The report goes in the PR and the issue; no retroactive annexes (S2).
- **L3** (2026-11-16, PRE-REGISTERED before any comparative run — the parameters S1/S2/S6 left
  free, fixed here and in the harness constants before the first run):

  - **period = delay = 100 ms** (every family's windows are sized in units of the FIXED arm's
    spacing); **J = 40 ms** — B's coefficient of variation (≈ 0.096) straddles the regularity
    detector's thresholds {0.05, 0.10, 0.20}; **C base = delay + J/2 = 120 ms**.
  - **Seeds 1..=10** (arm B only); **1.000 requests per arm per seed**.
  - **Window semantics**: fixed windows aligned at the run anchor, half-open
    `[k·W, (k+1)·W)`; the sliding and burst windows are trailing half-open `[a−W, a)`; an
    arrival exactly at a window boundary belongs to the LATER window. The token bucket starts
    full and refills continuously at 1 token per period (fractional accrual).
  - **Regularity detector**: population std/mean over the trailing **20** inter-arrivals; the
    first 20 arrivals of a run are exempt (warmup).
  - **Saturation**: an arm is saturated at a threshold when ≥ **90%** of its requests are
    penalized.
  - **Rate-only benefit** (S2's "B < A while B ≈ C"): the structural outcome is "no measurable
    structural difference" **and** mean(B penalized) + 10 < A penalized.
  - The harness holds these values as named constants; a change to any of them invalidates the
    run and forces a fresh one (S2, no retroactive annexes).
- **L4** (2026-11-16): the measurement RAN (nextest `--success-output=final`, the
  `the_full_measurement_covers_every_cell_and_reproduces` test; 13/13 harness tests green; the
  run reproduces identically across two invocations). The full table is in the delivery PR and
  on issue #2863. The headline outcomes, all mechanical under the pre-registered rule:

  - **The falsifier FIRED for family 4 (burst detector)**: B ≈ C at all three thresholds
    (0/0/10 ties, zero penalized in BOTH arms) with C **not** saturated — the pre-registered
    prediction ("B helps against burst or regularity detectors") is refuted at the
    slot-arithmetic level: the schedule's floor (gap ≥ delay = the period) already caps any
    trailing one-period window at one request, so no burst exists for the jitter to break.
  - **Family 5 (regularity detector) confirmed the prediction**: structural benefit at
    CV ≤ 0.05 (B̄ = 0.2 vs C = 980) and CV ≤ 0.1 (B̄ = 609.8 vs C = 980), 10/0/0 seeds — with
    C saturated at every threshold, the by-construction tautology S3 acknowledged, flagged
    `C-sat`. At CV ≤ 0.2 both arms saturate and tie (980 = 980).
  - **Family 3 (sliding window) at K=8 shows a saturated-regime benefit** (B̄ = 867 vs C = 992,
    10/0/0) — but A and C are both saturated there (992 ≥ 90%), so the falsifier's own
    "not already saturated" clause BLOCKS the refutation of the prediction for families 1–3.
    Reported as measured, with the flags.
  - **Family 1 at K=8 shows the rate-only benefit** (B̄ = 42 vs A = 200 while B ≈ C = 40,
    10/10 ties): the jitter's only gain against the fixed arm is the lower mean rate it always
    pays — exactly the prediction's first half.
  - Family 2 (token bucket) and family 4 penalize nothing in any arm: all ties, all zeros.
  - **No retroactive annexes**: no test asserts the prediction or its refutation; the report's
    numbers are the numbers from this run, and the S4 declarations travel with them.
- **L5** (2026-11-16): the guards forced the harness's layout. Two findings worth keeping:
  (1) `crates/webfang_core/src/application/rate_limiter.rs` is GRANDFATHERED by the file-size
  ratchet and cannot grow one line, so the module could not be declared there — it lives at
  `rate_limiter/jitter/sensitivity.rs`, declared in `jitter.rs` (247 lines, not grandfathered).
  (2) `check_dead_tests.sh` flags ANY `*/tests/*` path under crates/ that is not reachable from
  a cargo-registered test target — and its reachability walk starts only at test targets, never
  at `lib.rs`, so a src unit-test directory named `tests/` is ALWAYS "dead" by that heuristic
  (`jitter/tests.rs` survives only because `tests.rs` is a file, not a path segment). The
  harness's test file is therefore named `sensitivity/mechanical.rs`, not `tests.rs`.
