# ODD — Ola 2 close: `connect.rs` split by seams

Objective: split `crates/webfang_core/src/cli/commands/connect.rs` (1874 lines) into a hub
and children ≤ 500 lines, behavior preserved. Last grandfathered file of the Ola 2 band
`[1500, 2000]` (umbrella #2584); `params.rs` is a declared cohesive exception (Ola 2 L10).
Branch: `refactor/ola2-connect` · worktree `~/Projects/Rust/webfang-worktrees/refactor-ola2-connect`
· base `origin/main@94de2c51` · issue #2952 (`status:approved`, owner authorization in L1).
Delivery: `ask-on-risk`; runner: `cargo nextest` (same runner before and after every measurement).

## Specs

S1 — Scope (policy § Olas, quoted): "Ola 2 = [1500, 2000]". The baseline is the source of
truth: `jq '.file_size_rust.grandfathered["crates/webfang_core/src/cli/commands/connect.rs"]'`
`scripts/quality-baselines.json` → 1874. No open PR or branch touches the file.

S2 — Method (policy § Método, quoted): "Split por seams, un commit de unidad por extracción,
tests verdes entre unidades." Mechanical movement first; two hats, never mixed. The PR declares
`Movido sin cambios` and `Corregido / mejorado` (the second may be "sin hallazgos").

S3 — Test extraction (policy §4.3 step 5, quoted): "split_tests.sh [F2] will extract mod tests
with a multiset proof; until then the verbatim move is proven by hand." F2 (#2951) is unmerged:
the extraction is done by hand and the command + output are pasted in the mission log. The test
inventory invariant is the proof: same leaf-name set and same total count before/after; every
route rename is declared.

S4 — Production seams: 833 production lines must end ≤ 500 in the hub or in parts. Candidate
seams named at open: TOML provider merge/upsert (`upsert_provider` … `merge_capabilities`),
temp-file + atomic write (`temp_path` … `write_temp`), provider render (`display_for_id` /
`write_provider`), command flow + endpoint probe (`run` / `register` / `validate_endpoint`).
The exact plan is validated with `file_size_preflight.sh --simulate` (exit 0; a defect exits 4).

S5 — Close (policy §4.6, quoted): "file_size_done.sh <path> [F1b]: exit 0 = closed (parts ≤
hard_limit, or the file has an exceptions entry already present in the mission's base commit,
--remove done, functions under the threshold, no new allow in the diff, jscpd cx not raised,
golden refreshed, the same test set per the declared runner's <runner> list)". The ratchet
descent (`--remove` / descending `--update`) travels in the SAME commit that produces it; the
golden `tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt` is refreshed post-fmt in that
commit.

S6 — Verification (policy § Compuertas, no cuts): `cargo check --all-targets --all-features` ·
strict clippy (`-D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`) ·
`cargo fmt --all -- --check` · `RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features
--no-deps` · full nextest suite · `--list` before/after with the same runner · `check_file_size.sh`
· `check_test_inventory.sh` · `check_duplication.sh` · `check_dead_tests.sh` · intra-crate
direction · `bash scripts/ci_fast_gate.sh` GREEN with its coverage report read.

S7 — Governance: own issue (#2952) + PR with `Closes #2952` and `Closes part of #2584`, exactly
one `type:*` label (`type:refactor`), conventional branch, base `main`, sequential delivery.
No `CHANGELOG.md`, no `docs/` changes. Push, PR and merge remain the owner's decision.

S8 — High risk: `connect` writes the operator's config through a read-modify-write with an
optimistic concurrency check and an atomic rename. The split must keep the bytes and the call
order identical (`write_provider` → `upsert_provider` → `commit_splice` → `ensure_config_unchanged`
→ `write_atomically`). No SSRF/TLS policy change. Four lenses at close.

S9 — Parallel-safety record (owner question, L0): the F2 mission's surface is
`scripts/split_tests.sh`, `scripts/test_split_tests.sh`, `odd/tasks/file-size-split-tests.md`
and the gate registration (`scripts/ci_fast_gate_coverage.tsv`, `.github/workflows/ci.yml`,
`scripts/ci_fast_gate.sh`, `scripts/test_ci_gate_structure.sh`). This mission's surface is
`crates/webfang_core/src/cli/commands/connect.rs` + `connect/*`,
`scripts/quality-baselines.json`, the golden and `docs/test-inventory.json`. Intersection: empty.
Caveat: `fix/2859-body-cap-cause` also touches the golden and `docs/test-inventory.json`
(regenerated files); coordinate those two if that branch is still alive.

## Tasks

| ID | Task | Specs | Route | Status | Evidence |
|----|------|-------|-------|--------|----------|
| C0 | Worktree + bootstrap + indexes + this doc + issue #2952 | S1,S7,S9 | inline | done | proxy OK; direnv target + both indexes at 94de2c51 |
| C1 | Measure: preflight `--format`, `--list` baseline, seam census, `--simulate` plan | S1,S3,S4 | inline | done | 1874 = 833 prod + 1041 tests; churn 3; 0 allow; 35 tests baseline |
| C2 | Test extraction by theme into `connect/tests_*.rs`; `--list` invariant; commit | S2,S3 | inline | done | 11 / 6 / 18 tests; 35/35 pass; leaf set identical |
| C3 | Production seam extraction(s); tests green; commit per seam | S2,S4 | inline | done | `toml_merge` 284 + `atomic_write` 193; hub 373; check green |
| C4 | Ratchet `--remove`/`--update` + golden + inventory in the same commit | S5 | inline | done | entry removed; golden +9 sites post-fmt; inventory current |
| C5 | Full verification + four lenses + `file_size_done.sh` | S5,S6,S8 | inline | verification done | check/clippy/fmt/doc/nextest/gates green; `file_size_done` exit 0 |
| C6 | PR-ready state; push/PR owner decision | S7 | inline | pending | — |

## Log

L0 (verbatim, 2026-11-22): "cuales la siguiente mision de splits ? ya tengo un agente trabajando
en worktree feat-file-size-split-tests, que podemos hacer en paralelo sin conflictos?"

L1 (verbatim, 2026-11-22): "arranca y anade label status:approved tienes mi permiso" — owner
authorization for the mission start and for applying the protected `status:approved` label to
issue #2952 (recorded here as the binding approval; the label is not self-approved by the agent).

L2 (2026-11-22, C0): worktree `refactor-ola2-connect` ↔ branch `refactor/ola2-connect`, base
`origin/main@94de2c51`; `.envrc` written from the tree's own name (5 lines), `direnv allow`,
`.env` copied, identity manifest written; isolation verified (`CARGO_TARGET_DIR` =
`~/.cache/cargo-target/refactor-ola2-connect`); codegraph + codedb indexed with root = worktree
and head = 94de2c51. Issue #2952 created with `status:approved`. Seed attempt: `cold`
(no compatible seed for key `v1-ebe3b78b66ad5728`).

L3 (2026-11-22, C2-C4): extraction executed in one mechanical pass (tests and production)
after the pre-validated seam plan. Test items are located by their `#[test]` marker and carry
their contiguous attributes and doc comments; the first pass orphaned a `#[cfg(unix)]` across a
file boundary, the v2 pass fixed it (script `/tmp/opencode/extract_connect_v2.py`, outside the
worktree). Children: `tests_merge.rs` 456 lines (11 tests), `tests_flow.rs` 175 (6),
`tests_atomic.rs` 414 (18), `toml_merge.rs` 284, `atomic_write.rs` 193; hub `connect.rs` 373.
Cross-child helpers are `pub(super)` (`upsert_provider`, `temp_path`, `create_temp`,
`gc_connect_temps`, `write_atomically`, `write_temp`, `TEMP_CREATE_ATTEMPTS`).
Evidence: `cargo check --all-targets --all-features` green; `cargo nextest run -p webfang_core
--lib -E 'test(/cli::commands::connect::/)'` → 35 tests run, 35 passed; the `--list` leaf set is
identical to the 35-name baseline (`/tmp/opencode/connect-tests-before.txt`). Ratchet:
`check_file_size.sh --remove crates/webfang_core/src/cli/commands/connect.rs` removed the 1874
entry; `refresh_real_tree_golden.sh` added 9 sites (test-child inline `ConfigDefaults::load_explicit`
calls + the `toml_merge` import), refreshed post-fmt; `check_test_inventory.sh` reports current.

L4 (2026-11-22, C5 verification): full gate battery green in the worktree with `direnv export`:
`cargo check --all-targets --all-features`; strict clippy (`-D warnings -W clippy::cognitive_complexity
-W clippy::too_many_lines`); `cargo fmt --all -- --check`; `RUSTDOCFLAGS=-D warnings cargo doc
--workspace --all-features --no-deps`; `cargo nextest run --workspace --all-features` → 5741 run,
5741 passed, 8 skipped; `cargo test --doc --workspace --all-features` → 58 passed; the 35 connect
tests pass with the leaf set unchanged; `check_file_size.sh` ratchet holds; `check_test_inventory.sh`
current; `check_duplication.sh` at/below baseline (8173 vs 8219); `check_dead_tests.sh` clean;
`check_dependency_direction.sh` exit 0; intra-crate-lint 23/23 (real-tree golden matches).
`bash scripts/ci_fast_gate.sh` → PASS=47 FAIL=0 SKIP=2, GREEN; its coverage report was read —
`test-full` and `cargo test --doc` are locally uncovered, both run by hand above.
`file_size_done.sh` exit 0 for the target + six parts: hub 373, fixtures 119, atomic_write 193,
toml_merge 284, tests_merge 454, tests_flow 173, tests_atomic 414 — all ≤ 500. Four lenses: pending
native review; PR/push/merge remain the owner's decision (C6).
