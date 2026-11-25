# Automate static guards and shrink AGENTS.md

Objective: make the file-size/structure gates fail-closed at commit time so correctness no longer depends on an agent remembering them, and prune the now-automated prose from the always-loaded router.
Branch: `chore/engram-config` · Delivery: single PR (#3045) · Checks: `bash scripts/<guard>.sh`, `bash scripts/ci_fast_gate.sh`, `bash scripts/check_agents_doc.sh`

## Specs

S1: "entonces lo vamos a arreglar para no depender de la memoria de los agentes y de paso reducimos las lineas o bytes del AGENTS.md ya que esto se va a automatizar, solo dejamos datos relevantes"

Authorized scope and acceptance criteria derived from S1:

- A1 — `pre-commit` runs the cheap deterministic static guards itself and denies the commit on any violation (fail-closed). No guard may depend on a tool that is absent by default.
- A2 — `AGENTS.md` drops runbook/checklist prose that is now enforced by the hook, keeping source-of-truth tables, unique rules and the pointers to heavy checks that still require a manual/CI run.
- A3 — `AGENTS.md` stays under `scripts/quality-baselines.json` (`agents_doc`: max_lines 650, max_bytes 40960) with margin, verified by `scripts/check_agents_doc.sh`.
- A4 — The hook wiring has a deterministic test that proves both the deny path and the allow path.

## Tasks

- T1 — S1/A1/A4 — inline — **done**, commits `b42c240c` + correction below — static-guard runner wired into `scripts/githooks/pre-commit`; harness and the two affected identity harnesses updated. Six tool-free guards after the independent review removed `check_dependency_direction.sh` (jq).
- T2 — S1/A2/A3 — inline — **done**, commit `9e87ff65` — automated prose removed from `AGENTS.md` (620 lines / 39879 bytes, 1081 bytes under budget).
- T3 — S1/A1-A4 — inline — **done** — guards, harnesses, shellcheck, local gate and real commit probe all observed green.

## Log

L1: "entonces lo vamos a arreglar para no depender de la memoria de los agentes y de paso reducimos las lineas o bytes del AGENTS.md ya que esto se va a automatizar, solo dejamos datos relevantes"

L2 (verification): `scripts/tests/test_static_guards_hook.sh` proves wiring order, allow, deny and missing-guard paths over 7 guards in a hermetic temp repo. `scripts/githooks/static-guards.sh` passes on the live tree. `shellcheck` clean on all four touched shell scripts. `bash scripts/ci_fast_gate.sh` → PASS=46 FAIL=0 SKIP=3, GREEN (code lane). Both real commits ran the new hook and passed. `check_intra_crate_direction.sh` was deliberately excluded: it fails closed when the unbuilt `intra-crate-lint` binary is absent, so wiring it would deny every commit on a fresh checkout.

L3 (independent review, blocker): `gentle-ai-verify` reproduced a fresh-checkout denial — `check_dependency_direction.sh` requires `jq`, not part of the Rust toolchain. A1 was unmet: pre-commit would deny every commit for a non-violation. Fixed by removing that guard from `GUARDS` (it stays in `ci_fast_gate.sh`/CI), tightening the runner header, adding a harness assertion that no wired guard references `jq` outside comments, and correcting the stale two-gate claim in `.agents/skills/webfang-ci-gate/SKILL.md`. The first version of that assertion also flagged a guard whose only `jq` mention is a comment; the check now ignores comment lines.

L4 (recheck of the blocker only): harness OK with 6 guards, `shellcheck` clean, runner OK, `check_agents_doc.sh` OK, `git diff --check` clean. AGENTS.md unchanged at 620 lines / 39879 bytes.

L5 (next): push the new commits to the open PR #3045 only on explicit authorization; the PR title currently names only the first slice and should be widened because 2+ commits make it the changelog line.
