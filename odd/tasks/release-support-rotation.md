# Release support rotation — atomic minor declaration

Goal: make the release-plz Release PR own the support-window declaration for a minor release, removing the post-release rotation chore and the unrelated merge-queue rejection it causes.
Authorization: option A approved by the maintainer; C1 is authorized now, PR delivery waits for the protected `status:approved` label on #2531.

## Specs

S1. "vamos a revisar la issue: #2531 (type:bug) — la de raíz: el release PR debe rotar support.json en el mismo PR, con el modo de fallo silencioso documentado (pull_request verde / merge_group rojo / cola completa expulsada) y las tres ocurrencias (#2052, #2101, #2520) como evidencia de que el detector de #2061 detecta pero no previene."

S2. "A — el Release PR declara la rotación"

S3. "`rotate-stable.sh --pending-publication` declara STABLE/MAINTENANCE/EOL.
`eol-line.sh` no borra la rama en modo automático (queda un ::notice con el comando).
El diff del Release PR muestra la línea EOL → el humano la aprueba al revisar."

S4. "Worktree + C1 ahora, PR cuando haya label"

S5. "línea STABLE declarada: 7.1; minor publicada más reciente: 7.2. La ventana de soporte exige que la STABLE sea la minor vigente (publicar vX.(Y+1).0 rota la anterior a MAINTENANCE)."

S6. "minors publicadas sin declarar en support.json: 7.2  (registralas o marcalas EOL; crearlas es decisión del mantenedor con issue aprobada)."

## Tasks

T1 — C1: add the `pending_rotation` tolerance to `scripts/check_support_drift.sh` with RED/GREEN cases in `scripts/test_support_drift.sh` (S2, S5, S6) — route: inline/test-first — commit: `fix(support): accept a pending minor rotation in the drift gate` — DONE, commit `10be5260`

T2 — C4: keep the branch deletion out of the automatic rotation — `scripts/eol-line.sh` writes the EOL state and leaves a `::notice` with the exact command instead of pushing the deletion; new harness `scripts/test_eol_line.sh`, wired in `ci.yml` (S3) — route: inline/test-first — commit: `fix(support): keep branch deletion out of the automatic rotation`

T3 — C2: add the `rotate-stable.sh --pending-publication` mode, whose premise is the workspace version of the checkout instead of a published tag, keeping the GAP check and the default mode intact; new harness `scripts/test_rotate_stable.sh`, wired in `ci.yml` (S2, S3) — route: inline/test-first — commit: `feat(support): rotate the window before the tag is published`

T4 — C3: route an undeclared minor through `scripts/declare_pending_publication.sh` into the pending rotation, leaving the patch path untouched; harness cases in `scripts/test_declare_pending_publication.sh` (S2) — route: inline/test-first — commit: `feat(release): declare the pending minor rotation in the Release PR`

T5 — Delivery: open the PR against #2531 once that issue carries the protected `status:approved` label (S4) — route: branch-pr — commit: none (delivery)

## Log

L1. "vamos a revisar la issue: #2531 (type:bug) — la de raíz: el release PR debe rotar support.json en el mismo PR, con el modo de fallo silencioso documentado (pull_request verde / merge_group rojo / cola completa expulsada) y las tres ocurrencias (#2052, #2101, #2520) como evidencia de que el detector de #2061 detecta pero no previene."

L2. Audit evidence against `origin/main` at `d81136cf`: `.github/workflows/pr-validation.yml` invokes `scripts/check_support_drift.sh --scope=published` unconditionally for both `pull_request` and `merge_group`; the observed green PR/red queue mismatch came from a stale pre-tag PR run versus a fresh post-tag merge_group run, not event-specific enforcement.

L3. Audit evidence: PR #2507's pull_request validation was green before `v7.2.0` was published; merge_group runs `40946294266` and `40947499715` then failed with S5 and S6. Unrelated PRs #2503, #2509, and #2515 failed on the same drifted base; corrective PR #2521 passed.

L4. Audit evidence: #2052 and #2520 are minor-rotation occurrences; #2101 is patch-`.latest` drift fixed by #2102 and structurally closed by #2144/#2153. `scripts/declare_pending_publication.sh` already owns the atomic patch declaration and explicitly skips an undeclared minor; `scripts/rotate-stable.sh` requires the tag to exist and therefore cannot run before the Release PR merges.

L5. Maintainer decision: "A — el Release PR declara la rotación"

L6. Maintainer decision: "`rotate-stable.sh --pending-publication` declara STABLE/MAINTENANCE/EOL.
`eol-line.sh` no borra la rama en modo automático (queda un ::notice con el comando).
El diff del Release PR muestra la línea EOL → el humano la aprueba al revisar."

L7. Maintainer decision: "Worktree + C1 ahora, PR cuando haya label"

L8. Maintainer instruction: "continua"

L9. Implementation authorization: worktree `/home/xavi/Projects/Rust/webfang-worktrees/fix-support-drift-rotation`, branch `fix/support-drift-rotation`, base `origin/main` at `d81136cf`. T1 starts now; T5 remains blocked on the maintainer's protected `status:approved` label.

L10. T1 evidence — RED: `bash scripts/test_support_drift.sh` reported 64 ok / 5 fail, failing exactly the positive expectations of the new window (gate green, readable note, no `::error`) and the two numeric jumps (2.9 → 2.10, 2.13 → 3.0), while every negative case already passed. GREEN: 69 ok / 0 fail. Also green: `shellcheck scripts/check_support_drift.sh`, `bash scripts/test_guard_coverage.sh` (28 wired guards, mutation on — the first run failed only because the fresh worktree had no `tools/intra-crate-lint` binary; building it locally fixed that), `bash scripts/check_support_drift.sh --scope=both` against this repository, and a differential run of the base script against the new one over five fixtures (consistent, drifted, patch window, gap): byte-identical stdout, stderr and exit code; the only difference is the intended rotation window (base: 3 errors + exit 1; new: one `::notice` + exit 0). `cargo check`, `cargo clippy --all-targets --all-features -D warnings` and `cargo fmt --all -- --check` are green (shell-only diff). Commit: `10be5260`.

L11. Plan change — the units are reordered so the EOL branch deletion leaves the automatic rotation BEFORE the pending rotation mode exists: with `rotate-stable.sh --pending-publication` reachable while `scripts/eol-line.sh` still pushed `git push origin --delete`, the Release PR path could have deleted a remote branch on its own. T2 is now C4, T3 is C2, T4 is C3; no spec changed.

L12. Wiring decision — the new harnesses (`scripts/test_eol_line.sh`, `scripts/test_rotate_stable.sh`) are wired into the `repo-guards` job of `.github/workflows/ci.yml`, next to the other support harnesses. That is a `.github/` change, which the repository gates behind the protected `status:approved` label on #2531 (T5).
