---
name: webfang-ci-gate
description: "Trigger: ci_fast_gate, local gate, coverage report, pre-commit, identity gate, gate.needs, lane. Run the webfang local CI gate and read its coverage; complement Engram #2980/#2309."
license: Apache-2.0
metadata:
  author: webfang
  version: "1.1"
---

## Activation Contract

Activate before pushing or opening a PR, when running `scripts/ci_fast_gate.sh`, interpreting
its coverage report, replicating the pre-commit gate, or deciding which CI job to run by hand.
Skip for read-only exploration.

## Hard Rules

- `--all-features` is mandatory for clippy/check: `chromium`-gated code otherwise looks dead and
  `clippy --fix` would delete it. Never run `clippy --fix` against a narrower feature set.
- A bare `cargo fmt` is a fixer (rewrites the tree, exits 0 regardless); green from it is not
  evidence. Verify with `cargo fmt --all -- --check`.
- Read the coverage report before pushing: for every `NOT covered` job you touched, run that
  job's check by hand. The local gate mirrors `gate.needs`, not the whole workflow.
- Pre-commit replicates CI exactly: `cargo check` + `cargo clippy --all-targets --all-features
  -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` + `cargo fmt --all
  -- --check` + `env RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps`.
- `pre-commit` has two gates: `identity-gate.sh` runs first and is **fail-closed** (missing
  `.worktree-identity`, inherited `CARGO_TARGET_DIR` or detached HEAD deny the commit);
  `review-gate.sh` stays fail-open for review verdicts. A green review gate is not a green commit.
- Shell harnesses in `scripts/tests/` are not wired into `ci_fast_gate.sh` lanes. When scripts or
  hooks change, run `bash -n`, `shellcheck` and the relevant harnesses by hand.
- Deliberate local exceptions (toolchain, artifact-guards, full security, test-full depth) are
  partial/never-covered by class; do not treat a local GREEN as full CI equivalence.

## Decision Gates

| Situation | Action |
| --- | --- |
| Docs-only change | docs lane; still read the coverage report |
| Code change | code lane runs nextest for selected crates only (`test-core` partial); run affected module tests |
| `--all-features` workspace run needed | full lane (`test-full`) |
| Coverage names a job you touched as NOT covered | run that job's check by hand before push |
| Identity gate denies the commit | write the manifest with `scripts/worktree_identity.sh` or load direnv; never bypass the hook |
| Scripts or hooks changed | run the `scripts/tests/` harnesses by hand, alongside the normal gate |

## Execution Steps

1. Run `bash scripts/ci_fast_gate.sh`; let the classifier pick the lane.
2. Read the ending coverage report (covered / partial / never, per `gate.needs`).
3. Run every NOT-covered job you touched by hand; run affected-module nextest.
4. Run the full pre-commit gate (four commands above) before committing.
5. Re-read `git diff --stat main...HEAD`: only expected symbols/files changed.

## Output Contract

Return: lane run, gate verdict (GREEN/RED), the coverage report's covered/partial/never breakdown
for jobs you touched, and the hand-run checks performed for NOT-covered jobs.

## References

- `references/sources.md` — Engram #2980/#2309 queries + local sources of truth.
- `../../../scripts/ci_fast_gate.sh` — lane-aware local gate.
- `../../../scripts/ci_fast_gate_coverage.tsv` — coverage declaration mirrored to `gate.needs`.
- `../../../scripts/test_ci_gate_structure.sh` — drift guard (TSV ↔ `gate.needs`).
- `../../../scripts/githooks/identity-gate.sh` — fail-closed first gate in `pre-commit`.