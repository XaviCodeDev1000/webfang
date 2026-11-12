# Fix #2749 — the jitter RNG failure must degrade the randomization, not the pacing

**Objective**: when the OS entropy source cannot seed the jitter RNG, the run keeps its
`--delay-ms` pacing (token bucket, no jitter gate) and the lost randomization is WARNed, instead
of degrading to "no limiter at all".
**Branch**: `fix/jitter-rng-fallback` · **Delivery**: one PR on `main` · **Runner**: `cargo nextest run`.

## Specs

- **S1** — "fallback a token bucket sin jitter": when the OS RNG cannot be seeded, the limiter is
  built WITHOUT the jitter gate; the token bucket keeps pacing the run at the configured delay.
  The seed failure must not abort the run: the crawl path (`build_machinery`) must not fail
  because of it, and the scrape/batch builders must not fall back to `None`.
- **S2** — "haz que la degradación se avise en el log (WARN) en lugar de ocultarse en silencio":
  the degradation emits a WARN that names the lost randomization (the configured `jitter_ms`, the
  kept `delay_ms` and the OS error).
- **S3** — "Mantén el invariante de jitter 0 idéntico a hoy": with `jitter_ms == 0` the RNG is
  never consulted and the behavior is byte-identical to today's — no WARN, no extra work.
- **S4** — "Un test que fuerce el fallo del RNG es la prueba clave": a regression test forces the
  seed failure and pins the degraded shape — the limiter is built, and its pacing floor is the
  configured delay.
- **S5** — "The change must keep `jitter_ms == 0` byte-identical to today's behavior, and must not
  silently turn a requested jitter into a fixed cadence without saying so in the log." (issue #2749)

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| T1 | S1, S4 | inline | done | seam: `new_with_rng` + `JitterSchedule::from_rng` |
| T2 | S2, S3, S4 | inline | done | RED observed, then GREEN |
| T3 | S1, S2 | inline | done | fallback + WARN in `SharedRateLimiter::new_with_rng` |
| T4 | S1–S5 | inline | pending | — |
| T5 | S1–S5 | inline | pending | — |

## Log

- **L1** (2026-11-12) — The maintainer's instruction, verbatim: "#2749 (RNG sin pacing) es el que
  priorizaría. Es una degradación que abre justo la protección pedida. Ya tiene ruta de fix y es
  pequeño: fallback a token bucket sin jitter." and "Mantén el invariante de jitter 0 idéntico a
  hoy y haz que la degradación se avise en el log (WARN) en lugar de ocultarse en silencio. Un test
  que fuerce el fallo del RNG es la prueba clave."
- **L2** (2026-11-12) — Issue #2749 already carries the analysis (found by the native review of the
  #2610 candidate, finding `R4-001`): `JitterSchedule::new` returns `ScraperError::Config` when
  `StdRng::try_from_os_rng()` fails, `SharedRateLimiter::new` propagates it with `?`, and the three
  limiter builders degrade to `None` — "no pacing at all", not "pacing without jitter".
- **L3** (2026-11-12) — The degraded shape is decided: the token bucket ALONE, which is exactly
  today's `--delay-ms` behavior (burst included). The issue's phrase "pacing floor is the
  configured delay" is read as the long-run rate, not as a per-request floor: the bucket grants a
  burst, and the burst is what `--delay-ms` alone has always done. The alternative (keep the
  jitter gate with a fixed advance) would preserve a per-request floor but would NOT be "a token
  bucket without jitter", and it would change the shape of a run the operator already had.
- **L4** (2026-11-12) — Seam first: `SharedRateLimiter::new_with_rng` and `JitterSchedule::from_rng`
  split the seed source out of `new` WITHOUT changing behavior, so the failure path becomes
  reachable from a test (the OS entropy source cannot be made to fail on demand). The regression
  test then observes RED against the current propagation, and the fallback turns it GREEN.
- **L5** (2026-11-12) — The seam is a CLOSURE, not a value: `new_with_rng(config, seed)` takes a
  `FnOnce() -> Result<StdRng, E>`, so the OS entropy source is consulted ONLY when a jitter was
  asked for (`jitter_ms > 0`). A value would have read the entropy even for `jitter_ms == 0` —
  a behavior change on the invariant S3 forbids.
- **L6** (2026-11-12) — The tests live in their own module (`rate_limiter/tests_rng_fallback.rs`)
  and the trace-capture helper moved to `rate_limiter/test_support.rs` (ONE implementation, shared
  with the existing tests): `rate_limiter.rs` went 951 → 935 lines and its ratchet baseline was
  LOWERED to 935 instead of raised. The AGENTS.md rule for a file that is already too big is to
  split it, and this change shrinks it.
- **L7** (2026-11-12) — RED observed: with the pre-fix behavior restored (the seed failure
  propagates), `an_unseedable_rng_keeps_the_pacing_and_warns` fails with "the seed failure must
  not abort the limiter: Config(\"no se pudo inicializar el generador de jitter: …\")" — the
  defect of #2749, caught by the test and not by the compiler. `jitter_zero_never_consults_the_
  seed_source` passes in both states: it is the invariant guard, not the defect proof.
- **L8** (2026-11-12) — GREEN: 40/40 in the limiter module (`cargo nextest run -p webfang_core
  --lib rate_limiter`), the two new tests included. The degraded limiter keeps the bucket's
  pacing: the measured wait after burning the single burst permit is ≥ the configured delay.
