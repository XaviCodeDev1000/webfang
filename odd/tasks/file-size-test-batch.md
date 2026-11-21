# Mission: file-size-test-batch

Objective: extract the inline `#[cfg(test)] mod tests` block of four grandfathered files into sibling
`<stem>_tests.rs` files, following the byte-level pattern of #2931, and close their ratchet entries.
Branch: `refactor/file-size-test-batch` (worktree `refactor-file-size-test-batch`; base `3492178a`).
Delivery: ONE PR, work-unit commits (4 split commits + 1 ratchet/golden commit); runner: `cargo nextest run -p webfang_core --lib`, `bash scripts/check_file_size.sh`, `bash scripts/test_refresh_real_tree_golden.sh`, `bash scripts/ci_fast_gate.sh` (full lane).

## Specs

S1 (the accepted batch — owner's exact acceptance): "arrancamos con con ese batch". The batch is the
one proposed immediately before, verbatim:

| archivo | total → prod | tests que se mueven |
|---|---|---|
| `domain/config.rs` | 637 → 451 | 186 |
| `domain/persistence.rs` | 735 → 458 | 277 |
| `crawler/robots_utils.rs` | 684 → 477 | 207 |
| `adaptive_engine/real.rs` | 755 → 474 | 281 |

For each of the four source files the only production change is replacing its inline
`#[cfg(test)] mod tests { ... }` block with:

    #[cfg(test)]
    #[path = "<stem>_tests.rs"]
    mod tests;

The moved body is de-indented by 4 spaces; that de-indentation is the ONLY byte change to the moved
lines (the #2931 proof: `diff --ignore-all-space` empty, module path and all full test names
preserved, `nextest list` counts equal). No other production code changes.

S2 (original request, verbatim): "vamos a continuar con el plan de reducir y revisar el codigo de los
archivos grandes". Governing plan: `odd/tasks/file-size-reduction.md` (OLAS, two hats, four-lens peer
review); split policy: skill `webfang-file-splits`.

S3 (ratchet + golden, same commit): after the four moves each source is <=500 and leaves
`file_size_rust.grandfathered`; `scripts/quality-baselines.json` loses exactly those four entries.
`tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt` is regenerated with
`scripts/refresh_real_tree_golden.sh` in the SAME commit that changes the tree (its same-commit
contract).

S4 (verification triple): per touched file — the line guard (`check_file_size.sh`), function lints
(`clippy::too_many_lines` 100, `clippy::cognitive_complexity` 25, no new suppressions), and
`jscpd --complexity` `cx` not increased (pinned 5.4.0 via `scripts/check_duplication.sh`). Full
pre-commit gate green.

S5 (review + governance): each split reviewed under the four lenses (logic, efficiency, security,
maintainability); an explicit "no findings" is valid. PR body declares `Movido sin cambios` and
`Corregido / mejorado`, carries `Closes part of #2584`, label `type:refactor`, base `main`. Push, PR
creation and merge stay owner decisions.

## Tasks

- T1 — S1,S5 — delegated (parallel writers: 4 disjoint units, 186-281 lines each) — config.rs split (637→454) — COMPLETE (19/19 tests; byte proof + token proof empty)
- T2 — S1,S5 — delegated (same trigger) — persistence.rs split (735→461) — COMPLETE (21/21 tests; token proof empty)
- T3 — S1,S5 — delegated (same trigger) — robots_utils.rs split (684→483, TWO sibling files) — COMPLETE (13/13 tests; token proofs empty)
- T4 — S1,S5 — delegated (same trigger) — real.rs split (755→477) — COMPLETE (7/7 tests with --features adaptive-selectors; token proof empty)
- T5 — S3 — inline (shared files, parent owns git) — ratchet remove x4 + golden fixture refresh — COMPLETE in `ed9c42ef` (85 → 81 entries; golden +16 sites; harness 8/8)
- T6 — S4,S5 — inline — verification triple + four-lens review + gate — COMPLETE in `ed9c42ef` (gate 4/4 green; nextest 60/60; fast gate PASS=46 FAIL=0; duplication down)

## Log

L1 (2026-11-20): "vamos a continuar con el plan de reducir y revisar el codigo de los archivos grandes"

L2 (2026-11-20): "arrancamos con con ese batch"

L3: Worktree `refactor-file-size-test-batch` created off `origin/main` (`3492178a`, the #2931 head).
Identity proxy matches (`refactor-file-size-test-batch` == `refactor/file-size-test-batch`). `.envrc`
written from the tree's own name (5 lines), `direnv allow`, `.env` copied, `codegraph init` (829 files)
and `codedb reindex` (1537 files, head `3492178acefe`) run inside the worktree. Isolation verified:
`direnv export bash` → `CARGO_TARGET_DIR=/home/xavi/.cache/cargo-target/refactor-file-size-test-batch`;
CodeGraph `Project:` names the worktree; codedb head == git HEAD. `seed_target.sh` → `seed: cold
reason=no-seed key=v1-62438d8bb2aff6a9` (normal, not a build failure). Skill registry refreshed in-tree
(33 skills) for delegation.

L4: Pattern source studied from #2931 (`3492178a`): `exit_codes.rs` kept 206 lines, new
`exit_codes_tests.rs` 411 lines, wiring `#[cfg(test)] #[path = "exit_codes_tests.rs"] mod tests;`,
`use super::{...}` lines kept de-indented at the new file head, 18 sites added to the intra-crate
golden fixture, one baseline entry removed — all in the same commit. This batch replicates that
shape four times.

L5 (parent decision on T3, no human decision needed): `robots_utils.rs` has TWO top-level test
modules (`tests`, 169 body lines; `memory_probe_tests`, 32 body lines) plus a `#[cfg(test)]
fn init_cache_cell` helper that stays in source. Extracting only `mod tests` would leave ~515 lines
(>500) and break S3; folding both into one file would rename the memory-probe test and break S1.
Adopted the writer's option A: widened T3's edit surface to two sibling files, each with its own
`#[cfg(test)] #[path = "..."] mod ...;`. All 13 full test names preserved, including
`infrastructure::crawler::robots_utils::memory_probe_tests::probe_robots_cache_growth_5k_hosts`.
Source lands at 483 lines (6 above the nominal 477: two 3-line wirings instead of one) — still
under the 500 guardrail. This is an architecture-neutral mechanical extension of the authorized
objective, not a scope change.

L6 (parent decision on the de-indent/rustfmt conflict): pure 4-space de-indent makes five wrapped
calls fit under `max_width = 100`, so `cargo fmt --all -- --check` was red on `persistence_tests.rs`
(4 sites) and `real_tests.rs` (1 site). Both a strict `diff --ignore-all-space` and fmt-green cannot
hold simultaneously. Decision: **authorize rustfmt reflow** and keep the stronger semantic proof —
`/tmp/opencode/verify_extractions.py` derives each original body from `git show HEAD:<source>`,
de-indents 4 spaces, and compares whitespace-stripped AND trailing-comma-normalized token streams
against the working-tree file. Result: 5/5 extracted modules `token_stream_equal=True`. Rationale:
the invariant is "no test content changed", not "de-indent is the only byte diff"; trailing commas
and line wrapping are rustfmt-normalized and semantically inert. `cargo fmt --all` applied;
`cargo fmt --all -- --check` now green. The accepted batch's table (S1) is unchanged.

L7 (observed line counts vs proposal): the proposal's "production" column subtracted the whole
inline block (186/277/207/281 lines) including its `#[cfg(test)]`/`mod tests {`/closing brace, so
the post-extraction source is production + 3 (or +6 for the two-module T3). Observed: config 454,
persistence 461, robots_utils 483, real 477 — all <=500. No defect; recorded so the descent is
reproducible.

L8 (closure evidence, commit `ed9c42ef`): single work-unit commit — the golden fixture's same-commit
contract binds all four tree changes together, so the batch ships as one reviewable slice (12 files,
+1057/-950). Verification triple: line guard green (81 grandfathered, down from 85); clippy full
(`--all-targets --all-features -D warnings -W cognitive_complexity -W too_many_lines`) green;
duplication DOWN (jscpd 5.4.0 base 8204 lines/56138 tokens → 8197/56088, `newClones: 0`, gate
baseline 8219). Pre-commit 4/4 green (check, clippy, fmt --check, doc with RUSTDOCFLAGS=-D warnings).
Combined nextest of the four modules with `--features adaptive-selectors`: 60/60 pass (19+21+13+7).
Golden harness `test_refresh_real_tree_golden.sh`: 8/8. Local gate (`ci_fast_gate.sh`, code lane):
PASS=46 FAIL=0 SKIP=2; coverage confirms the touched jobs (file-size ratchet/preflight, intra-crate
lint tool tests, duplication ratchet) ran in this lane; the 2 SKIPs (pinned lint, windows cross-target)
do not apply — no platform module or lint config changed.

L9 (four-lens review — explicit result): logic — no findings (token-equivalent move, 60/60 tests,
all full names preserved); efficiency — no findings (no production code changed); security — no
findings (no new inputs, SSRF-guard test moved verbatim); maintainability — no findings, with two
declared deviations already recorded in L5 (two sibling files for the two-module file) and L6
(rustfmt reflow with token-stream proof). Advisory-only notice from the fast gate: 30 moved test
functions have no issue/PR number in their names — pre-existing names, unchanged by this batch;
disclose in the PR body, do not rename (renaming would break S1's full-name invariant).

L10: delivery state — committed locally on `refactor/file-size-test-batch` at `ed9c42ef`. Push, PR
creation and merge remain owner decisions under ordinary repository policy. PR must carry
`Closes part of #2584`, label `type:refactor`, base `main`, and the two declaration lists
(`Movido sin cambios` = all five extracted modules; `Corregido / mejorado` = none).
