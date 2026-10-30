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

T2 — C4: keep the branch deletion out of the automatic rotation — `scripts/eol-line.sh` writes the EOL state and leaves a `::notice` with the exact command instead of pushing the deletion; new harness `scripts/test_eol_line.sh`, wired in `ci.yml` (S3) — route: inline/test-first — commit: `fix(support): keep branch deletion out of the automatic rotation` — DONE, commit `ad68339e`

T3 — C2: add the `rotate-stable.sh --pending-publication` mode, whose premise is the workspace version of the checkout instead of a published tag, keeping the GAP check and the default mode intact; new harness `scripts/test_rotate_stable.sh`, wired in `ci.yml` (S2, S3) — route: inline/test-first — commit: `feat(support): rotate the window before the tag is published` — DONE, commit `8c52cf23`

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

L13. T2 evidence — RED: `bash scripts/test_eol_line.sh` reported 10 ok / 3 fail: the remote branch was deleted, the local branch was deleted, and there was no `::notice` with the runbook command. GREEN: 13 ok / 0 fail — the state lands (EOL, `branch` null, `eol_at`, `eol_reason`, `accepts` removed), both branches SURVIVE, the notice names `git push origin --delete support/2.1`, and an undeclared line still fails closed without mutating the declaration. Also green: `shellcheck`, `scripts/test_guard_coverage.sh` (28 wired guards, mutation on), `scripts/check_test_inventory.sh`, `scripts/check_regression_naming.sh`, `scripts/test_ci_gate_structure.sh` and the `ci.yml` YAML parse. Commit: `ad68339e`.

L14. Delivery strategy — `exception-ok`: ONE PR with four reviewable commits, over the ~400-line slice budget. The budget is respected per commit (T1 277, T2 266 authored lines), and the PR exceeds it for two reasons that are not negotiable: (a) the unit is atomic — C1 alone would widen the acceptance domain of a REQUIRED check with no producer that fills it (the maintainer's decision was explicit: C1 does not merge alone), and splitting the producer from its tolerance would leave the Release PR carrying a rotation the gate rejects; (b) this repository has no stacked PRs — sequential delivery would need the same protected label several times for one issue. The slice boundaries are the commits: `10be5260` (C1, the gate tolerance), `ad68339e` (C4, the EOL deletion out of the rotation), the C2 commit (the pending rotation mode) and the C3 commit (the Release PR routing), with the last one closing the feature.

L15. T3 evidence — RED: `bash scripts/test_rotate_stable.sh` reported 11 ok / 10 fail: the pending mode refused with `v2.2.0 no está publicado` and nothing was rotated. GREEN: 21 ok / 0 fail — the pending mode rotates without the tag (new line STABLE with `latest` X.Y.0 and `accepts` bugfix+security, the previous STABLE demoted to MAINTENANCE with `accepts` security, the third line EOL with `eol_at`), the `::notice` announces the branch deletion and the remote branch survives, `SUPPORT.md` matches the fresh render of the rotated declaration, the default mode still refuses without the published tag and still rotates with it, the workspace version is the premise (a checkout preparing 2.3.0 may not declare 2.2), the GAP check still refuses a published-but-undeclared minor in between, and a second pending run writes nothing. Differential against the previous commit (`ad68339e`): the default mode is byte-identical (exit code, stdout, stderr and the timestamp-normalized `support.json`) with the tag missing, with the tag present and with an invalid line; the only difference is the line number bash prints inside the unchanged no-argument usage error. Commit: `8c52cf23`.

L16. T4 evidence — RED: the new `scripts/test_declare_pending_publication.sh` against the previous commit's script (`8c52cf23`) reported 33 ok / 10 fail — every minor-rotation assertion fails (the new line is not STABLE, the previous one is not demoted, the rotation is not named, no commit lands on the release branch, the projection does not travel). GREEN: 45 ok / 0 fail — the writer rotates the window of a pending minor (new line STABLE with `latest` X.Y.0, the previous STABLE demoted to MAINTENANCE with `accepts` security), the rendered projection travels with the declaration, a rotation that CANNOT be done (a published minor that was never declared) fails the writer with a named error and leaves the declaration byte-identical, the patch path is untouched (an undeclared line with a patch version still warns and skips, and a declared line still changes only its `latest`), and the orchestrator pushes the ROTATED window to the Release PR branch with the bot identity. Differential against `8c52cf23`: a declared line with a patch version is byte-identical (exit code, stdout, stderr, declared lines); an undeclared line with a patch version behaves identically and only the WORDING of the warning changes (it now says "declaring a new line is a governance decision" instead of "rotating the support lines is a governance decision", because the rotation is now what the script does for a minor); an undeclared line with a minor version differs as intended (rotation instead of skip).

L17. Latent defect found and fixed while wiring the pending mode — `scripts/rotate-stable.sh` died SILENTLY, with no message, on a repository without tags: the `grep` of an empty tag stream exits 1, `pipefail` turns the assignment into a failure and `set -e` kills the script. The default mode never reaches that line (the tag premise comes first and exits with a message), so only the pending mode could hit it. Fixed with an explicit `|| true` and a comment that says why: no tags means no published minors, which is an answer (the gap is empty), not a crash.

L18. Fixture hygiene — the support harnesses pin `tag.gpgsign false` on purpose: this workstation signs tags globally, and a signed tag opens an editor to compose the message, so a harness that tags without pinning the config HANGS instead of failing. The new case that publishes a tag (the GAP fixture) had to pin it as well.
