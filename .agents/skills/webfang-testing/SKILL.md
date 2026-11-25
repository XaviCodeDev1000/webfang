---
name: webfang-testing
description: "Trigger: integration test, shell harness, insta, snapshot, webfang_path, BehavioralTest, six-node. Write webfang integration tests and scripts/tests/ harnesses with deterministic fixtures."
license: Apache-2.0
metadata:
  author: webfang
  version: "1.1"
---

## Activation Contract

Activate when writing or modifying an integration test in `crates/webfang_core/tests/`, a shell
harness in `scripts/tests/`, resolving the `webfang` binary, producing Markdown/JSON/stderr
assertions, or reviewing a snapshot diff. Skip for pure unit tests inside a module.

## Hard Rules

- Resolve the binary with `webfang_path()`, NEVER `assert_cmd::cargo_bin(...)`: this is a virtual
  workspace and the `webfang` binary is built by sibling `webfang_cli`, so `CARGO_BIN_EXE_*` is
  not set for `webfang_core` tests.
- Every test producing Markdown/JSON/stderr uses `insta` snapshots, not `assert!(output.contains)`.
  Cycle: nextest (`.snap.new`) → `cargo insta review` (review every diff) → nextest green. Never
  commit pending `.snap.new` (gitignored).
- `redact_nondeterministic()` is mandatory (TempDir → `[TEMP_PATH]`, timestamps → `[TIMESTAMP]`,
  ports → `[PORT]`, ANSI → `[ANSI]`); extra fields via `insta::Settings::clone_current()` +
  `add_filter(...)` + `.bind(...)` (see `crates/webfang_core/tests/behavioral/main.rs`).
- No real network: wiremock for HTTP, TempDir for the filesystem; injected time and randomness.
- Six-node diagnostic: observable behavior (public ports), ephemeral adapters, semantic
  assertions, effort in stable domain logic, Arrange ≤5 lines, absolute determinism. An Arrange
  longer than 5 lines is a production design defect — fix the design, not the test.
- Integration tests auto-discover (one file per target); add a `[[test]]` entry only for
  subdirectories or special features. Import the harness with `#[path = "common/mod.rs"] mod common;`.

## Shell harnesses (`scripts/tests/`)

Scripts and hooks are verified by standalone bash harnesses, not insta snapshots:

- Build fixtures in `mktemp -d` with a `trap 'rm -rf "$TMP"' EXIT`; never touch the real
  `~/.cache/cargo-target/` or a session worktree. A fake git repo and fake cache are enough.
- Assert observable behavior only: report JSON fields, exit codes and filesystem effects. No
  `contains` on incidental text; no dependency on clock or process IDs.
- For destructive scripts, test the dry-run default and the missing-token failure; never run
  `--execute` against real data from a harness.
- Run `bash -n`, `shellcheck` and the harness itself; these are not covered by `ci_fast_gate.sh`
  lanes, so run them by hand when the corresponding script changes.

## Decision Gates

| Situation | Action |
| --- | --- |
| New integration test | File in `crates/webfang_core/tests/`, harness import, `webfang_path()`, snapshots |
| Output assertion | Snapshot + `redact_nondeterministic()`, never `contains` |
| Snapshot diff | `cargo insta review` every diff; never auto-accept blind |
| Arrange >5 lines | Refactor the production design, not the test |
| Script or hook changed | Add/update a `scripts/tests/` harness; run `bash -n` + shellcheck + harness |

## Execution Steps

1. Create the test file; import the harness; build the command with `webfang_path()`.
2. Arrange with wiremock + TempDir (≤5 lines); inject time/randomness.
3. Assert via snapshots sanitized with `redact_nondeterministic()`.
4. Run `cargo nextest run --test <name>`; review every `.snap.new` with `cargo insta review`; re-run green.

## Output Contract

Return: test file path, the harness/`webfang_path()` wiring, snapshot names, sanitization applied,
and the nextest + insta-review verdict (green, no pending snapshots).

## References

- `references/sources.md` — local sources of truth (harness symbols, AGENTS.md § Testing).
- `../../../crates/webfang_core/tests/common/cli_harness.rs` — `BehavioralTest`, `scraper_cmd()`,
  `find_files()`, `read_md_content()`, `webfang_path()`, `redact_nondeterministic()`.
- `../../../AGENTS.md` — § Testing (normative rules + six-node diagnostic).
- `../../../scripts/tests/` — shell harnesses for `sweep_orphan_targets.sh` and the identity gate.