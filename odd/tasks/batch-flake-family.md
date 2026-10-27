# Mission: batch_test flake family — diagnosability + de-leak (#2387)

Branch: `test/batch-flake-family` · Worktree: `~/Projects/Rust/webfang-worktrees/test-batch-flake-family`

## Specs
- S1 (issue #2387): "Produce a nextest JUnit report in the test lanes and upload it as a workflow artifact (and/or identify the `profile.ci` knob that surfaces the failed attempt's output for flaky tests — to be verified locally against the installed nextest version)."
- S2 (issue #2387): "When the budget fires, kill the child process and let the worker thread finish instead of abandoning it: removes the LEAK class and puts each measured latency in the panic message."
- S3 (issue #2387, DEFERRED): "Re-evaluate `batch_stdin_processes_urls` once S1 gives us a real TRY 1 panic: today it is the only batch test doing a real fetch without the budget machinery."
- User constraint: "ten cuidado en saturar los recursos del sistema cuando ejecutas comandos pesados" — builds go through mbx (shared cache + RAM scheduler, obs #2306); no `CARGO_BUILD_JOBS` override; focused nextest runs only.

## Tasks
- T1 (S1): fix the JUnit capture placed by #2384 in the wrong job — `test-core` runs ubuntu but the step is named `nextest-junit-windows`; add uploads to `test-crossplatform` (per-OS artifact name), `test-full`, `flake-detection` (`profile.flake-nightly` path).
- T2 (S2): `BudgetChild` in `tests/common/cli_harness.rs` — spawn/register/kill + wait; `run_under_budget` kills the child and joins the worker on timeout; `Overran` carries the killed pid; update the 2 real callers + 4 harness tests; add RED-first test pinning kill+reap (unix-gated).
- T3: verification (check/clippy/fmt + focused nextest) + work-unit commits + PR `Closes #2387`.

## Log
- L1 (2026-10-27): request received. Diagnosis: #2384 (21ea545a) added the JUnit upload to `test-core` (ubuntu) named `nextest-junit-windows` while its own commit message says "the windows lane uploads it" — the lanes with the flake evidence (`test-crossplatform` windows/macos, `test-full`, `flake-detection` nightly) upload nothing. nextest 0.9.131 (the repo pin): no terminal knob surfaces a flaky test's failed-attempt output (`final-status-level = "slow"` already covers the status line); the JUnit records every retry as `<flakyFailure>`/`<rerunFailure>` WITH output (`store-failure-output` defaults true) — the artifact is the diagnosability mechanism. JUnit `path` is relative to `target/nextest/<profile>/`.
- L2 (2026-10-27): S2 design — `assert_spawn_within`'s attempt closure receives a `BudgetChild` handle (`Fn(&BudgetChild) -> Result<(), String>`); `BudgetChild::output(std_cmd)` spawns with piped stdio + null stdin, keeps the `Child` in a shared `Mutex<Option<Child>>` slot (poison-recovered, never `.expect()`), polls `try_wait` and reads pipes via reader threads. On timeout the harness calls `kill()` and joins the worker (bounded: the kill unblocks the wait), then reports `Overran(latencies, killed_pid)`. Non-spawning attempts keep the old abandon semantics (empty slot → no join), so the existing latency property holds.
