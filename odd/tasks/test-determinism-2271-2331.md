# test-determinism-2271-2331

Objective: one PR closing #2271 + #2331 — same root class: tests that measure
the environment's clock/load instead of asserting a product property.
Branch: `test/test-determinism-2271-2331` · Worktree:
`~/Projects/Rust/webfang-worktrees/test-test-determinism-2271-2331` · Base:
`aa19f2e9` (includes #2492; files verified disjoint) · Delivery: single PR.

## Specs

- **S1** (user, verbatim): «Fase 3 — Determinismo de tests (un solo batch):
  #2271 + #2331 en un PR cerrando ambas: misma raíz (tests que miden
  reloj/carga del entorno en vez de la propiedad del producto), N issues nunca
  justifican N patches.»
- **S2** (user, verbatim): «Ojo: #2491 (en vuelo por otro agente) toca core —
  revisá el diff antes de abrir para evitar colisión en
  crates/webfang_core/tests/.»
- **S3** (user, verbatim): «ten cuidado al hacer builds ejecutando comandos
  pesados, porque hay otros agentes trabajando»
- **S4** (#2271): the test asserts a product property (an observable event),
  not an absolute wall-clock threshold; a hang guard stays, generous and
  load-independent.
- **S5** (#2331): the lane's tests stop converting retry passes into reds
  (`flaky-result = "fail"`); if a structural Windows cause exists it gets
  documented and the test adjusted — never a per-test flaky override (#2313
  retires those).

## Mechanism map (verified against `aa19f2e9`)

| Test | Anchor | Mechanism today | Fix |
|---|---|---|---|
| `batch_file_timeout_does_not_hang` (#2271) | `tests/behavioral/cli/batch_test.rs` | `elapsed < 25s` wall assert over a run that is healthy at 1.7s; measured 26.2s under load (issue evidence) → red with no product defect. The tokio 30s guard also orphans the child instead of killing it. | Assert the observable contract only: exit 69 + stderr names the URL + timeout keyword. Hang guard = `assert_spawn_within`/`BudgetChild` (kills + reaps). Wall asserts deleted. |
| `crawl_js_strategy_respects_timeout_secs` (#2331) | `tests/behavioral/cli/crawl_test.rs` | `elapsed < 10s` wall assert over a run that measured 7.1s on TRY 2 (run 40407265924) — 2.9s headroom under load. Same orphaning tokio guard. | Assert `!status.success()` only (the 10s mock delay flips any ignored-timeout regression to success → exit code still catches it; the engine-level contract is pinned by `engine_js_strategy_timeout_test.rs`). Budget machinery for the hang guard. |
| `batch_empty_stdin_exits_64` (#2331) | `tests/behavioral/cli/batch_test.rs` + `SPAWN_LATENCY_BUDGET` (`tests/common/cli_harness.rs`) | Shape is already correct (budget + kill + reap), but the 2×20s internal window is narrower than the observed load spikes — both internal attempts overran on run 40407265924. | Raise the shared `SPAWN_LATENCY_BUDGET` 20s → 60s: 2×60s stays under the `default` profile ceiling (180s) and the `ci` ceiling (240s); real regressions still fail fast through the assertion, never through the budget. |
| `cli_vs_mcp_export_jsonl_record_parity` (#2331, third) | — | Resolved by #2336/#2345 (build-lock class); maintainer's scope note asks to confirm separately. Not reproduced on any run since 2026-10-26 (windows lane green 2026-10-29, run 40910682204). | No code change; PR body records the evidence. |

## Tasks

- **T1** — `SPAWN_LATENCY_BUDGET` 20s → 60s with the new evidence in its doc
  comment; update the two empty-batch doc comments that cite ~2x20 s. **done**
  (`2a63e5a7`).
- **T2** (#2271) — convert `batch_file_timeout_does_not_hang` to the observable
  contract under `assert_spawn_within` (spawn_blocking keeps wiremock's
  current-thread runtime free). **done** (`60042559`); its sibling
  `batch_file_timeout_reports_failures` converted in the same slice — same
  mechanism (orphaning 30s tokio clock, 3.8s from the measured load spike),
  same file, same class.
- **T3** (#2331) — convert `crawl_js_strategy_respects_timeout_secs` the same
  way; drop the elapsed assert. **done** (`f52789b5`).
- **T4** — verification + PR `Closes #2271` + `Closes #2331`. **done**
  (evidence below; PR opened after the #2492 diff check).

## Log

- **L1** (2026-10-29) — original request: S1–S3. Prior art located: #2336
  (`WEBFANG_TEST_PREBUILT_BIN`), #2345 (#2335 parity pre-builds), #2408
  (kill+reap + JUnit per-attempt), `odd/tasks/1697-spawn-flake-sibling.md`,
  `odd/tasks/batch-flake-family.md`. The residual #2331 scope is the
  maintainer's reopening comment: run 40407265924 (2026-10-26, AFTER
  #2336/#2345) — `batch_empty_stdin_exits_64` FLKY-FL 2/3 (pass 1.0s) and
  `crawl_js_strategy_respects_timeout_secs` FLKY-FL 2/3 (pass 7.1s); the third
  original test did not reproduce.
- **L2** — S2 resolved by observation: #2491 merged as PR #2492 (`aa19f2e9`)
  before this branch was cut; its diff (persistence.rs, state_store.rs,
  ADR-0013, intra-crate fixture) is disjoint from every file this mission
  touches.
- **L3** — bootstrap: worktree, `.envrc` (5 lines, own tree name), `.env` copied,
  codedb index `1194 files @ aa19f2e9`, codegraph indexing, seed `cold`
  (`no-seed` — normal, first build pays; mbx manages load, no
  `CARGO_BUILD_JOBS` override, focused runs only per S3).
- **L4** — implementation, 3 work-unit commits (`2a63e5a7`, `60042559`,
  `f52789b5`). Two compile iterations fixed forward inside the slice before
  any commit of the converted tests: `JoinError::try_into_panic` consumes
  `self` (match with `Err` rebinding), and the `Fn` attempt closure cannot
  consume a captured `Command` (the command is built inside the closure from
  owned captures, the `batch_empty_file_exits_64` idiom).
- **L5** — scope note: the third #2331 test (`cli_vs_mcp_export_jsonl_record_parity`)
  gets no code change per the maintainer's reopening comment and the
  post-#2336/#2345 evidence; recorded in the PR body.

## Evidence

- **E1** — targeted nextest, 5 affected tests × 3 consecutive runs:
  **15/15 PASS**, durations stable (empty-batch 0.68–1.75 s, timeout-batch
  1.67–2.91 s, crawl 5.27–5.64 s). Zero retries consumed, `flaky-result=fail`
  active (local `default` profile inherits the enforcement).
- **E2** — RED audit (guard ≠ lid): injected `!= Some(70)` into
  `batch_file_timeout_does_not_hang` → TRY 1/2/3 FAIL at 1.32–1.65 s THROUGH
  the assertion ("expected exit code 70 … got Some(69)" + the product's
  timeout stderr), the budget untouched. Reverted; green again at 1.34 s.
  Same audit precedent as the #1697 sibling mission.
- **E3** — stderr contract cross-check: the product's fetch error path emits
  `operation timed out … status: None` for the 1 s/10 s scenario, so the
  timeout-keyword assert reads real product output (also pinned by the
  pre-existing `batch_file_timeout_reports_failures` contract).
- **E4** — gate: `cargo check` ✓ (57.6 s cold), `cargo clippy --all-targets
  --all-features -D warnings -W cognitive_complexity -W too_many_lines`
  exit 0, `cargo fmt --all -- --check` ✓. `cargo doc` not applicable: only
  test files changed (rustdoc does not document test targets).
- **E5** — S3 load discipline: every build went through mbx (cache hits on
  every invocation; 356-compilation reuse on clippy), builds strictly
  sequential, focused nextest filters only, seed `cold/no-seed` paid once.
