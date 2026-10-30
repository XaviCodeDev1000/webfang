# Mission: test/wall-clock-class-sweep — class sweep of wall-clock budget tests

Closes #2533. One PR per defect class (third generation of the test-determinism
cluster: #2271, #2331, #2533; playbook = #2498).

## Specs

- S1: "issue #2533 primero — y ojo al patrón sistémico: es la tercera generación del cluster test-determinismo en dos días (#2271, #2331 cerraron ayer, esta nació ayer del fix de #2490)."
- S2: "Cada fix de timeout que aterriza deja un test con presupuesto de reloj que flakea en windows cargado."
- S3: "La medicina de raíz no es taponar este test: un barrido de clase — auditar todos los tests que afirman presupuestos de wall-clock y convertirlos a aserciones de propiedad del producto, como ya hiciste en #2498."
- S4: "Un PR por clase, no por test."

## Audit result (2026-10-30, main c05b5b60)

### Actionable (this PR)

| Target | Defect | Action |
|---|---|---|
| `SITEMAP_FETCH_DEADLINE_BUDGET` = 15s (`sitemap_fetch_timeout_test.rs`) | margin 1.76x over healthy 8.5s (#2533, FLKY-FL evidence latencies [15.02s, 15.01s]) | 15s → 60s (~7x; 2×60s < nextest 180s/240s ceilings; < 113s buggy regime #2490) + evidence doc comment |
| `engine_js_strategy_respects_config_timeout` (`engine_js_strategy_timeout_test.rs`) | `elapsed < 10s` upper bound, margin 2x over healthy ~4.5s; property already pinned by `requests.len() == 2` + `total_pages == 0` + `errors >= 1` | drop the elapsed assert; widen the in-process `timeout(15s)` hang-guard → 30s (7.4x healthy, below the 60s #280 buggy regime of 2 × 30s attempts); update module docs |
| `a_healthy_attempt_returns_without_paying_the_budget` (`tests/common/cli_harness.rs`) | `elapsed < 1s` upper bound on a sub-ms operation | convert to the observable (harness-recorded waits), not a wall clock |

### Audited-OK (no change; rationale recorded in PR body)

- `progress_integration.rs` ×3 — `CI_SAFE_BOOKKEEPING_BOUND` 5s over µs work (~1000x), documented stall canary.
- `sitemap_behavioral.rs` — 2s perf budget over ms-scale parse (~40x), substantive assert is `urls.len() == 1500`.
- `scraper_service_test.rs` paced-batch — `< 30s` stall guard, deliberately loose (#1962).
- `remote_embedding_resilience_test.rs` — two-sided documented bounds (floor ≥ 2s = the wire-honor signal; ceiling 30s = 30x over ~1s real).
- `waf_gauntlet_test.rs`, `scraping_coverage_test.rs` — lower bounds (≥), load-safe direction.
- `stress_test.rs` — feature-gated (console), elapsed used for metrics only.
- `cli_harness.rs` `SPAWN_LATENCY_BUDGET` = 60s — verified as #2498 left it.
- `cli_harness.rs` `an_overrunning_attempt_is_abandoned_at_its_budget` — budget × 5 on harness-measured waits (#2063 pattern).

## Tasks

- T1: `SITEMAP_FETCH_DEADLINE_BUDGET` 15 → 60 + doc comment carrying the #2533 evidence (latencies, healthy 8.5s). **done**
- T2: engine test — drop `elapsed < 10s`, guard 15s → 30s, docs updated. **done**
- T3: harness healthy-attempt pin → assert on recorded waits. **done**
- T4: affected tests + full gate green. **done**
- T5: work-unit commits + PR (Closes #2533). **done**

## Log

- L1: original request (S1–S4, verbatim in Specs).
- L2: class audit complete (see table); Engram mirror saved.
- L3: T1+T2+T3 implemented; engine test green, sitemap test green, harness suite green.
- L4: full pre-commit gate green (check / clippy all-targets all-features / fmt --check / rustdoc -D warnings).
- L5: PR #2537 opened (Closes #2533); nextest run on the touched targets green; CI queued.
