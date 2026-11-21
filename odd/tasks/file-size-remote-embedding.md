# ODD — File-size mission: remote_embedding.rs (Ola 2, first full-flow mission)

Objective: split `crates/webfang_core/src/infrastructure/llm/remote_embedding.rs` (1714 lines, grandfathered baseline 1714) by real seams, running the complete policy flow for the first time: `file_size_preflight.sh` (measure & plan) → `--simulate` → seam split → `file_size_done.sh` (close-out). Umbrella #2584, Ola 2 band [1500,2000].
Branch: `refactor/file-size-remote-embedding` (worktree `refactor-file-size-remote-embedding`, base 4bc8b9d9). Delivery: `ask-on-risk` — push/PR/merge are the owner's decision. Runner: `cargo nextest`. Forecast: ~1700 authored changed lines (mostly verbatim moves + test re-parenting).

## Specs

S1 Band [1500,2000], measured 2026-11-21 on main 4bc8b9d9: `cli/commands/connect.rs` 1874 (churn 3), `infrastructure/llm/remote_embedding.rs` 1714 (churn 13), `mcp_server/params.rs` 1552 (declared exception, migration condition recorded in the baseline). Wave rule S4 of `odd/tasks/file-size-ola2.md`: order by churn, not size. User verbatim: "elegís el archivo de Ola 2 (por churn/complejidad, no por líneas — la regla de la ola)".

S2 User verbatim: "la primera misión real de Ola 2 — un archivo de la banda [1500, 2000] con el flujo completo por primera vez: file_size_preflight.sh (medir y planear) → --simulate → split por seams → file_size_done.sh (cerrar)."

S3 Split by real seams, never line arithmetic. Two hats, never mixed: mechanical move (`Movido sin cambios`) vs semantic change (`Corregido / mejorado`), semantic bounded ~20-30% of the diff. Closure is co-equal: every part ≤500 lines + `clippy::too_many_lines`/`cognitive_complexity` clean with no new suppressions + `jscpd` cx (pinned 5.4.0 via `scripts/check_duplication.sh`) not increased. Baseline descent (`--remove`/`--update`) in the SAME PR as the split.

S4 Behavior preserved: `refactor(scope)` commits, tests green between work units, snapshots intact. If the file contains a fetch path, the guard-chain order stays verbatim and contiguous (ValidUrl → rate limit → timeout → redirect → SSRF dial → retry classification → capped body read).

S5 Governance: own issue; PR body carries `Closes part of #2584`; `type:refactor` label; base `main`; sequential delivery; no `CHANGELOG.md`, no `docs/` (except the mandatory test-inventory gate if moved `#[ignore]` paths require it).

S6 This mission is the real case F2 (`split_tests.sh`) validates its multiset proof against — F2's acceptance is defined "after the first real mission".

S7 Push/PR/merge remain the owner's decision; this worktree stops at verified close-out + PR-ready state.

## Tasks

T0 Worktree + bootstrap + isolation checks + indexes + this document — done
T1 `file_size_preflight.sh --format` measurement block — pending
T2 Seam exploration + split plan; validated by `--simulate` before any cut — pending
T3 Split implementation, one work-unit commit per seam, tests green between commits — pending
T4 Triple verification (≤500 lines, function lints, cx) + ratchet `--remove`/`--update` — pending
T5 `file_size_done.sh` close-out (exit 0 expected; 2 parked only with an explicit declared reason) — pending

## Log

L1 (verbatim): "El siguiente paso natural: la primera misión real de Ola 2 — un archivo de la banda [1500, 2000] con el flujo completo por primera vez: file_size_preflight.sh (medir y planear) → --simulate → split por seams → file_size_done.sh (cerrar). … Mi recomendación: elegís el archivo de Ola 2 (por churn/complejidad, no por líneas — la regla de la ola) y arrancamos la misión; F0 lo hago en paralelo si querés. ¿Arrancamos con alguno?"

L2 2026-11-21: Context recovered from Engram (#3196, #3044, #3219). Churn measured: remote_embedding 13 commits, connect 3, params 36 (excluded: declared exception). No open PRs and no mission worktrees → the "rama AI egress activa" exclusion from #3044 no longer applies; both connect.rs and remote_embedding.rs are eligible. Chosen: remote_embedding.rs (S1). Worktree bootstrapped with isolation checks PASS (identity gate, direnv target, both indexes root=head). Seed outcome `cold` (no published seed). Gotcha recorded: the runbook's 5-line `.envrc` template needs explicit `export` on assignments — plain assignments are invisible to direnv's environment diff (direnv 2.38.2), and the identity gate denied with "CARGO_TARGET_DIR is unset" until fixed. Main's live `.envrc` is the working reference.
