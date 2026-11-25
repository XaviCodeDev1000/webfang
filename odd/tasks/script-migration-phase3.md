# Script migration — Fase 3

Branch: `refactor/script-migration-phase3` · Base: `origin/main@17ef82c7` · Worktree: `/home/xavi/Projects/Rust/webfang-worktrees/refactor-script-migration-phase3` · Delivery: work-unit commits on this branch; push/PR/merge remain human decisions. Runner: `bash scripts/tests/<harness>.sh` + `cargo nextest run` when Rust code is added.

## Specs

S1. "solo crea el worktree y deja el plan bien guardado y claro para ejecutar la migracion en otra sesion". This mission creates the worktree and the ODD record only; no script or Rust source is modified in this session.

S2. "Empezar por `analyze-trace.sh` → luego `ci_plan.sh`". Verified mapping (read-only, Engram decision #3518): `scripts/analyze-trace.sh` (149 lines, 13 `jq` commands, no CI invocation, deterministic, low risk) goes first; `scripts/ci_plan.sh` (193 lines, writes 16 `key=value` pairs to `GITHUB_OUTPUT`, feeds `gate.needs`, high risk) goes second.

S3. Verified correction to the incoming plan: `ci_plan.sh` does **not** call GitHub APIs. Inputs are `GITHUB_*`/`PR_*`/`MERGE_GROUP_*`/`PUSH_*` env, `git rev-parse`, `mktemp` and two bash detectors (`scripts/ci_plan.sh:29-63,102-174`). `test_ci_gate_structure.sh:47,74,79,344-345` reads `FULL_PRODUCERS`/`NARROW_PRODUCERS` literally from the script text; any port must keep or adapt that contract.

S4. Verified correction to the incoming plan: `cargo -Zscript` is **not viable** here. `mise.toml:6` and `rust-toolchain.toml:6-7` pin stable 1.91.1; CI uses `dtolnay/rust-toolchain` with the repo pin (`ci.yml:128`) and a toolchain-drift guard (`ci.yml:465-483`). Migrate to a **Rust binary in the workspace**, not a `.rs` script requiring nightly.

S5. "Golden tests por cada migración (comparar stdout/stderr/exit code contra el Bash)". For `analyze-trace.sh`, reuse the hermetic fixture shape in `scripts/test_artifact_analyze_trace.sh:54-93` and the existing Rust parser + goldens in `crates/webfang_benchmark/src/aggregate.rs` / `tests/goldens/`. Compare all 13 commands, missing file, missing args and injected ids; normalize jq error text across versions.

S6. "Mantener los .sh originales hasta que los .rs estén probados"; "xtask solo si aparece necesidad de APIs internas o tests unitarios de la automatización"; "cargo -Zscript para scripts nuevos con crates"; "Mantener Bash para glue scripts y file-size policy"; "No migrar check_dependency_direction.sh"; "No migrar en bloque". File-size policy scripts, semantics harnesses and glue stay in Bash.

## Tasks

T1 | S2,S4,S5,S6 | inline writer + differential harness | completed | commit: `d5ddf951` — `analyze_trace` binary in `webfang_benchmark` reuses `aggregate::TraceLine` (extended with top-level `span`/`trace_id`); `scripts/test_artifact_analyze_trace_differential.sh` compares 13 commands + goldens + edge cases byte-for-byte (stdout, normalized stderr, exit code) against the Bash; 58/58 PASS; `.sh` kept.
T2 | S2,S3,S4,S5,S6 | delegated writer + golden test | pending | commit: pending — migrate `ci_plan.sh` only after T1; preserve `GITHUB_OUTPUT` contract and adapt `test_ci_gate_structure.sh` literals; golden test from `scripts/test_ci_plan.sh:35-74` (PR/merge_group/push, bad SHA, classifier failure, crossplatform tri-state).
T3 | S5,S6 | inline verification + cleanup | pending | commit: pending — run all golden tests, full gate, then remove originals only after green; update `justfile`/docs only if the new binary needs a recipe.

## Log

L1 (verbatim user request): "solo crea el worktree y deja el plan bien guardado y claro para ejecutar la migracion en otra sesion".

L2 (verbatim plan fragments supplied by the user): "Empezar por `analyze-trace.sh` → luego `ci_plan.sh`"; "Golden tests por cada migración (comparar stdout/stderr/exit code contra el Bash)"; "xtask solo si aparece necesidad de APIs internas o tests unitarios de la automatización"; "cargo -Zscript para scripts nuevos con crates (manteniendo el .sh hasta probar el .rs)"; "Mantener Bash para glue scripts y file-size policy"; "No migrar check_dependency_direction.sh"; "No migrar en bloque".

L3 (read-only mapping, no edits): `scripts/ci_plan.sh:29-63,102-174` uses env/git/bash detectors, no `gh`/`curl`; `ci.yml:196-211` feeds `gate.needs`; `ci.yml:640-647` records a real silent-skip incident. `scripts/analyze-trace.sh:6,29-46,48-149` has 13 commands, `jq` gate and anti-injection `jq --arg`; no CI wiring. `crates/webfang_benchmark/src/aggregate.rs:1-7,19-42` already parses the same JSONL contract with goldens. Stable pin: `mise.toml:6`, `rust-toolchain.toml:6-7`, `ci.yml:128,465-483`. Verdict: start with `analyze-trace.sh`, workspace binary (not `-Zscript`); mapping persisted as Engram decision #3518.

L4 (bootstrap, 2026-11-25): sibling worktree created from `origin/main@17ef82c7`; 5-line `.envrc` written from this tree name and activated; `.env` copied; identity manifest written; CodeGraph (963 files) and CodeDB (1714 files) indexed here; `direnv export bash` reports `CARGO_TARGET_DIR=.../refactor-script-migration-phase3`; both indexes name this tree and match HEAD `17ef82c7`; identity-gate exits 0; branch ↔ dir proxy OK.

L5 (resume recipe for the next session): work only at the absolute worktree path above; `eval "$(direnv export bash)"` before any command; read this file until `## Log`, then `mem_search` project `webfang` topic `phase3/script-migration-plan` (decision #3518) and `odd/script-migration-phase3/tasks` (this mirror); verify current HEAD and git status before editing; T1 is the first task. No implementation was authorized in this session.

L6 (T1 completed, 2026-11-26): implementation authorized by the user's "continua" against this plan. Worked inline (one understood unit, no Writer reason). Added `[[bin]] analyze_trace` plus `src/bin/analyze_trace.rs` (475 lines; dispatch, 9 row commands, GNU sort/uniq/jq scalar helpers) and `src/bin/analyze_trace/whole.rs` (285 lines; `jq -s` commands + jq pretty printer) — split to pass the 500-line ratchet. Extended `aggregate::TraceLine` with `span`/`trace_id` (serde default; backward compatible). New `scripts/test_artifact_analyze_trace_differential.sh` (LC_ALL=C; jq error lines normalized; stdout/exit byte-exact) passes 58/58: 13 commands, 3 goldens × 12 commands, missing file/args/id, injected ids, empty file. Legacy `scripts/test_artifact_analyze_trace.sh` still 5/5; `cargo nextest run -p webfang_benchmark` green (77 tests); `cargo check`, clippy (`-D warnings`, cognitive/too_many_lines), `cargo fmt --check`, `check_file_size.sh` and `RUSTDOCFLAGS=-D warnings cargo doc` green; `ci_fast_gate.sh` PASS=47 FAIL=0 SKIP=2. Notable: jq 1.8.1 preserves key order/lexemes, `uniq -c` uses a 7-wide count, `sort -rn` reverses the last-resort tie comparison, and `jq -c` compacts whitespace — all pinned by the differential. No `.sh` removed yet (T3). Work-unit commit `d5ddf951` (6 files, +927/−1; static-guards 6/6, review-gate informational). Mirror resynced at T1 close.
