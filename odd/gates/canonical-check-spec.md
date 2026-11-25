# Canonical check gate specification

This file is the single source of truth for the local copies of the CI verification gate.
CI (`.github/workflows/ci.yml`) is authoritative: when CI changes, update this file first,
then mirror the exact commands into `justfile` and `.pre-commit-config.yaml`.

## Commands

Run from the workspace root, in this order:

1. `cargo check`
2. `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`
3. `cargo fmt --all -- --check`
4. `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps`

`--all-features` is mandatory: `chromium`-gated code otherwise looks dead to clippy and
`clippy --fix` would delete its imports. `cargo fmt` without `--check` is a fixer and its
green exit is not evidence; always verify with `--check`.

## Language

All comments, errors, logs and user-facing strings in repository artifacts are English.

## Mirrors

- `justfile`: recipes `check` and `test-ci` run the four commands above; `test-ci` also runs the focused test suite.
- `.pre-commit-config.yaml`: hooks `rustfmt-check` and `clippy` use the commands above.
- `.github/workflows/ci.yml`: authoritative source; the `repo-guards` job also runs
  `bash scripts/tests/test_canonical_gate_alignment.sh` to check that the four commands above stay aligned across the mirrors.

## Verification

- `just check`
- `pre-commit run --all-files`
- Full pre-commit gate: the four commands listed above.
- `bash scripts/tests/test_canonical_gate_alignment.sh` (hermetic string/args comparison; no cargo, just, pre-commit or rustdoc execution).
