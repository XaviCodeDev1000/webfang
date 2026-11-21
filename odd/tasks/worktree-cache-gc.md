# ODD — Worktree cache GC + identity gate
Objective: build the agreed orphan-target sweep and fail-closed identity gate, confirmed against the real FS. Branch: `feat/worktree-cache-gc` (worktree `feat-worktree-cache-gc`). Delivery: `ask-on-risk`; runner: `bash scripts/tests/test_*.sh`, `bash scripts/ci_fast_gate.sh`, cargo pre-commit gate.

## Specs

S1. "lo armamos nosotros  en un worktree con documento ODD." — Authorized scope: implement here, in a sibling worktree with this document and its Engram mirror. Acceptance: worktree isolated (`.envrc`, indexes, identity proxy), document + mirror present, work units committed on the feature branch. Checks: `git branch --show-current`, proxy, `codedb status`, isolation guard.

S2. "Vamos a analizar las recomendaciones de A && B luego investigamos las mejores opciones para nuestro proyecto y si es necesario migrar o actualizar o eliminar cosas que tenemos implementadas lo hacemos si el beneficio es real." — Only changes with verified real benefit: orphan-target sweep + identity gate. No migration to kache/sccache/shared target/mbx managed targets (benefit measured as zero). Acceptance: no `Cargo.toml`, feature-flag, profile, `.envrc` cache-wrapper or mbx-setting changes. Checks: `git diff --stat main...HEAD`.

S3. "### IMPORTANTE*: A & B no tienen acceso a nuestro repositorio pero tu si. nunca des por valida su recomendacion sin confirmar antes." — Every A/B design element must be checked against this repo before adoption. Acceptance: audit findings recorded in the Log; sweep uses `.envrc` declarations as the live registry (not worktree root paths), protects all four reserved dirs by canonical identity, uses newest-file mtime, keeps lsof guard and confirmation token; hook combines main path+branch anchor, rebase detection, manifest anchor and non-interactive remedy. Checks: test scripts in `scripts/tests/` covering each case.

S4. "de momento no implementamos nada, ahora vamos a auditar las dos recomendaciones solamente, y si necesitas mas detalles o informacion me lo dices en tu respuesta y se le paso tus dudas a A && B y ellos lo investigan por nosotros" — The audit phase is complete and read-only; this document opens the implementation phase only after the explicit "lo armamos nosotros" instruction.

S5. Assumption (from the agreed salvage design, not a user string): the sweep is dry-run by default and never deletes in this feature; the identity gate is fail-closed and wired before the existing fail-open `review-gate.sh`; the second anchor is a `.worktree-identity` manifest written at bootstrap. Acceptance: no deletion command executed against `~/.cache/cargo-target/`; hook denies missing manifest; existing worktrees need their owner to write the manifest before the hook is enabled there. Checks: dry-run test with fake fixtures, hook tests, documented migration note.

## Tasks

- T1 — S1,S4 — route inline — bootstrap worktree + ODD doc + mirror — status: done — commit: pending
- T2 — S2,S3,S5 — route inline — shared canonicalization helper + orphan sweep + tests — status: pending — commit: pending
- T3 — S1,S3,S5 — route inline — identity gate + pre-commit wiring + manifest helper + tests — status: pending — commit: pending
- T4 — S2,S3 — route inline — AGENTS.md alignment (gitignore, namespace, mbx vs sccache, hook) — status: pending — commit: pending
- T5 — S1,S5 — route inline — full verification + ODD close + mirror — status: pending — commit: pending

## Log

L1 (verbatim user request): "lo armamos nosotros  en un worktree con documento ODD."

L2 (verbatim user constraint): "### IMPORTANTE*: A & B no tienen acceso a nuestro repositorio pero tu si. nunca des por valida su recomendacion sin confirmar antes."

L3 (verbatim earlier scope): "Vamos a analizar las recomendaciones de A && B luego investigamos las mejores opciones para nuestro proyecto y si es necesario migrar o actualizar o eliminar cosas que tenemos implementadas lo hacemos si el beneficio es real."

L4 — Evidence collected before implementation (all confirmed on this FS, 2026-11-21):
- `mbx gc --dry-run` would reclaim only ~9 GiB learned incremental + 81.8 MiB abandoned target; `gc.auto=true` already set. Manual `CARGO_TARGET_DIR` is outside mbx managed targets (vendor docs). No tool migration.
- Both A/B sweeps built the live set from worktree ROOT paths and compared to TARGET DIR paths — namespaces never match, so live targets would be misclassified as orphans. Correct registry: `CARGO_TARGET_DIR` declarations read from live worktrees' `.envrc` (model of `check_target_isolation.sh`).
- B's reserved check covered only descendants of `seeds`/`quarantine`; `seeds-webfang` (7.8G) and `quarantine-webfang` (4.9G) are siblings with `CACHEDIR.TAG` + `.rustc_info.json` and would be deleted. All four are excluded by documented precedent in `odd/tasks/merge-prs-to-main.md`.
- B used directory mtime; measured `main`: dir mtime 15 Nov, newest file 20 Nov — directory mtime would mark the active target idle. Use newest-file mtime.
- B regressed on `--porcelain -z` (works here), `|| true` on git failure (fail-open), and `--execute` confirmation token; A's design keeps those correct.
- Hook: A's main exception lacked branch check; B lacked rebase/cherry-pick detection and his `.worktree-exception` was dead code. Neither shipped the non-interactive remedy; in this shell `CARGO_TARGET_DIR` is set to main's target (silent inheritance the hook must catch).
- No `.worktree-identity` / `.worktree-exception` exists in any worktree: the hook's manifest is a migration dependency, not yet satisfiable by existing trees.
- Live worktree set changes between checks (`chore-reanchor-baseline-v921` replaced `fix-baseline-anchor-durability` mid-audit): sweep must read state at execution time.

L5 — Route declaration: everything inline; no writer-delegation trigger (no parallel independent units, context within budget). High-risk item: T3 touches the shared `pre-commit` hook (delivery/environment, item 5); it gets its own commit and an independent verifier after its own checks. RDD mode is on (global).
