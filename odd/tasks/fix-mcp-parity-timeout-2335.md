# fix-mcp-parity-timeout-2335

Objective: make the nightly parity tests (`cli_vs_mcp_sitemap_run_record_shape_parity`, `cli_vs_mcp_export_jsonl_record_parity`) stop burning the 180s slow-timeout ceiling in the `flake-detection` lane, by fixing the root cause already diagnosed as finding T5 of #2331 — not by wrapping the symptom in a test-side timeout.
Branch: `fix/mcp-parity-timeout-2335` · Worktree: `~/Projects/Rust/webfang-worktrees/fix-mcp-parity-timeout-2335` · Issue: #2335 (`status:approved`, `type:bug`) · Delivery: single PR (small scope) · Runner: `cargo nextest`.

## Specs

- **S1** — «El cuelgue reproducido y la causa raíz explicada (¿espera sin timeout en una de las dos corridas? ¿I/O del registro? ¿sustrato compartido con #2331?).» Root cause candidate (from #2331 T5, to be reproduced and confirmed here): in `--all-features` lanes the core helper builds the binary with five forwarded features while the MCP helper builds default; both write `<target>/debug/webfang`, so concurrent test processes alternate feature sets and invalidate each other's link.
- **S2** — «El test determinista bajo `--retries 0` en corridas consecutivas de la lane nocturna.» A symptom wrapper (`tokio::time::timeout` around the MCP half) does NOT satisfy this: under retries-0 the lane stays red, only faster.
- **S3** — «Si la causa es la infraestructura de paridad compartida, el fix cubre también a #2331 (y viceversa).» The #2331 side already landed (#2336, `WEBFANG_TEST_PREBUILT_BIN`); this mission closes the open half (T5).
- **S4** — «sin override per-test, siguiendo el criterio de #2313/#2331»: no `.config/nextest.toml` override for these tests, no `#[ignore]`, no test deletion.
- **S5** — Design constraint carried from T5: NOT `WEBFANG_TEST_PREBUILT_BIN` on `test-full`/nightly — the lane pre-build uses `--all-features` (chromium) and the core helper deliberately does not forward `chromium`. The fix is one authoritative pre-build + contract, or per-feature target dirs.

## Tasks

- **T1** — Reproduce the collision locally and pick the design. **done** (see E1–E6, D1; the T5 mechanism is refined, not confirmed: there is NO steady-state invalidation).
- **T2** — Pre-build both helper specs (`default` + the five forwarded features) in `test-full` and `flake-detection` BEFORE nextest, without declaring `WEBFANG_TEST_PREBUILT_BIN`. **done** (CI scope confirmed by user prompt — unanswered, proceeded with the recommended both-lanes option; actionlint clean; +27 lines, no other surface touched).
- **T3** — Verify: consecutive local `--retries 0` runs of both parity tests green, then the nightly lane observation post-merge. **local done** (E7); the consecutive-nightly observation stays open post-merge.

## Design

- **D1 (chosen)** — Pre-build the two exact specs the helpers build, in the runner, before nextest; keep the on-demand builds (no contract env). After materialization every helper build is a ~0.3s no-op that still verifies the fingerprint, so the YAML↔helper feature-list sync is enforced by cargo itself: drift degrades to one visible slow build, never a silent wrong binary (#302 hermeticity preserved — blind trust via the contract would let helpers run whichever spec's file sits at `target/debug/webfang`).
- **Rejected — per-feature target dirs**: duplicated dep graphs, does not remove the cold materialization cost, doubles CI compile.
- **Rejected — unify the helpers' feature sets**: changes the binary surface under test (a behavioral decision), and still leaves one cold build inside a test.
- **Rejected — declare `WEBFANG_TEST_PREBUILT_BIN` in all-features lanes**: T5/S5 — the lane pre-build (`--all-features`, chromium) does not match what the helpers build; the contract disables the helpers' feature-set self-check.

## Evidence

- **E1** — Cold spec A (`cargo build -p webfang_cli --bin webfang`, the MCP helper's spec): **42.31s**. Matches #2331's E1 (42.238s).
- **E2** — First spec B (`--features adaptive-selectors,mcp,persistence,console,dev-tracing`, the core helper's spec under `--all-features`): **42.15s**, **116 units recompiled** — the features drag half the graph (mcp→rmcp chain, persistence→sqlite, …), not just a final relink. Binary 126.7MB (A) → 139.3MB (B).
- **E3** — Spec A again: **0.263s, 0 units**. Cargo keeps both feature variants side by side (distinct `-C metadata` hashes): **there is no A↔B invalidation ping-pong once both specs are materialized**. The T5 mechanism ("concurrent processes invalidate each other's link") is wrong in steady state.
- **E4** — Steady-state alternation: B again 0.387s / A again 0.252s, 0 units both.
- **E5** — Refined root cause: with no pre-build and nextest's process-per-test, the **first materialization of EACH spec** (two ~42s builds locally; tens of seconds in-lane) is paid INSIDE test processes, serialized under one cargo build lock. A test queuing behind both first-builds burns past the 180s slow-timeout with retries=0 (nightly), or hangs-then-passes → `flaky-result=fail` reds the PR lane (`test-full`, retries=2). The parity tests are the visible casualties because they run both flows (largest surface among the webfang_mcp tests).
- **E6** — `test-full`'s current pre-build (`--all-features`, chromium) materializes a spec NO helper builds; #2331 deliberately left the contract unset there, so both helper specs are still cold when the first tests run.
- **E7** — Local verification with both specs materialized (the lane's post-pre-build state): `cargo nextest run --profile flake-nightly -p webfang_mcp --test cli_mcp_export_jsonl_parity_test --test sitemap_crawl_run_staleness_test` — 3 tests run, 3 passed, **0.732s** (run 1) and **0.643s** (run 2), zero retries. Both parity tests green and fast in consecutive runs.

## Log

- **L1** — Original request: issue #2335 (nightly parity TIMEOUT, 180s ceiling, run 40116186564). 2026-10-25: first attempt in this worktree was reviewed and discarded by user decision («lo eliminamos por completo y vamos a empezar desde cero»): it had zero commits, bundled #2331/#2332/#2333 changes into this branch, duplicated #2336 hunks already in main, and its 90s `tokio::time::timeout` wrapper was a palliative that left S2 unsatisfied.
- **L2** — Fresh bootstrap from `main@735addc8` (includes #2336 and #2339): worktree, `.envrc` (5 lines), `.env`, identity proxy OK, indexes.
- **L3** — Implementation + delivery: commit `f183f765` (ci.yml +27 in `test-full`/`flake-detection`, mission record), `ci_fast_gate` GREEN (12/0/13 lane-aware), actionlint clean, PR **#2345** (`type:bug`, base main). Merge waits on the maintainer (`scripts/merge-when-green.sh 2345 --queue`); S2's consecutive-nightly confirmation is a post-merge observation.
