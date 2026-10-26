# Mission: spawn-lint-enforcement (#1951)

Branch: `chore/spawn-lint` · Worktree: `~/Projects/Rust/webfang-worktrees/chore-spawn-lint`
Issue: #1951 (`status:approved`, label `type:chore`) — ban `tokio::spawn` via clippy
`disallowed-methods`, per-site justification in production, blanket test exemption.

## Specs

- S1: "adelante pero ten cuidado en saturar los recursos del sistema cuando ejecutas
  comandos pesados" — keep heavy commands (builds/gate) to a minimum; one final
  verification pass on a seeded target.
- S2 (issue, Expected behavior): clippy `--all-targets --all-features -D warnings` green
  with the new entry; a new bare `tokio::spawn` fails the build; no behavior change.
- S3 (issue, Policy): production sites audited one by one — supervised sites carry
  `#[allow(clippy::disallowed_methods)]` naming the supervision mechanism; misrouted
  sites migrate to `spawn_blocking`; tests get `#![cfg_attr(test, allow(...))]` per crate
  root with unit tests + file-top allow per integration test file, each with a one-line
  policy comment. `spawn_blocking` NOT banned.
- S4 (issue, Acceptance criteria): clippy.toml entry w/ reason referencing #1951 and the
  #1941/#1943 class · audit table in PR body · unsupervised sites surfaced, never
  silently allowed · test exemption with policy comment · both spellings fire · audit
  covers src + tests + benches + examples · `ci_fast_gate.sh` green.
- S5 (issue, Known interaction): webfang_core/webfang_mcp crate-root
  `deny(clippy::disallowed_methods)` (ungated) must not be weakened for non-test code —
  attribute-leveling mechanics are part of the implementation.

## Tasks

- T1: Audit all `tokio::spawn` sites: `crates/*/src` (production vs inline `#[cfg(test)]`),
  `crates/*/tests`, `benches/`, `examples/` → classify each.
- T2: Add `tokio::spawn` entry to `disallowed-methods` in `clippy.toml`.
- T3: Crate-root test exemptions (`cfg_attr(test, allow)`) with policy comment, ordered
  after the env-mutation deny where both live; file-top allows in integration test files
  that spawn.
- T4: Per-site `#[allow]` with supervision mechanism for legitimate production sites.
- T5: Migrate or follow-up-issue genuinely unsupervised production sites.
- T6: Verify both spellings fire (probe or pin).
- T7: Verification gate: clippy full command + `ci_fast_gate.sh` (single heavy pass).
- T8: PR with audit table in body, linked to #1951.

## Log

- L1 (2026-10-27): Original request — "vamos a revisar y analizar la Issue #1951 pero ten
  cuidado en saturar los recursos del sistema", then "adelante" (implementation
  authorized, resource-careful). Worktree created at sibling path; indexes ready
  (codedb 1152 files @ 6f8ed0aa, codegraph ok). Seed `cold` (no seed for key) — first
  build pays the graph once.
