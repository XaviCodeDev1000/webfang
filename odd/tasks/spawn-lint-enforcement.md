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
- L2 (2026-10-27): Worktree misplacement fixed — `git -C webfang worktree add
  <relative>` created an in-repo worktree; removed and recreated at the sibling path
  with absolute paths.
- L3 (2026-10-27): Audit strategy — added the clippy.toml entry FIRST and let clippy
  enumerate violations (DefId-authoritative) instead of text search (130 files mention
  "spawn"; too noisy). Two warm clippy passes with `-j 8`, no full cold rebuild.
- L4 (2026-10-27): Authoritative census (clippy, --all-targets --all-features):
  8 production sites (webfang_core lib) — ALL supervised, zero migrations needed;
  ~33 inline cfg(test) sites in webfang_core + 3 in webfang_mcp (lib test);
  13 integration test files (9 core, 4 mcp); ZERO sites in webfang_cli,
  webfang_test_utils, webfang_benchmark, benches/, examples/.
- L5 (2026-10-27): Production audit table (each verified by reading the lifecycle):
  - bounded_sink.rs (with_backlog_bytes): handle stored in Self.writer, joined in
    finish() with JoinError → BoundedSinkError::Writer.
  - collector.rs (new): handle stored in ResultsCollector.handle, joined in collect().
  - engine.rs (with_autoscale): stored in self.autoscale_handle, aborted in shutdown()
    (#1941 lifecycle).
  - engine.rs (spawn_signal_handler): returns JoinHandle; caller stores + aborts at
    shutdown (#1941 lifecycle).
  - shutdown.rs (ShutdownGuard::install): guard owns handle, aborts on Drop.
  - chromiumoxide_downloader.rs: HandlerGuard aborts the CDP pump on every non-join
    exit (#1129).
  - axtree/mod.rs (fetch_raw_axtree): deterministic finally calls handler_job.abort(),
    even on timeout.
  - bridge.rs (dispatch_resource): handle intentionally not stored; the oneshot
    receiver is the supervision surface — task death closes tx, receiver observes.
- L6 (2026-10-27): Attribute-leveling mechanics: webfang_core's crate-root deny is
  followed by `#![warn(clippy::style)]` (implied member re-warn) — the test allow must
  be placed AFTER the group warns, and a group mention downgrades the deny to warn
  (CI -D warnings re-escalates). Documented for the PR.
- L7 (2026-10-27): Spelling probe: temporary test using `tokio::task::spawn` failed
  clippy -D warnings (exit 101) → both spellings fire. Probe deleted after verification.
- L8 (2026-10-27): Two work-unit commits: 89f7ef36 (lint entry + crate roots + 8
  production allows), 20bb322f (13 test-file exemptions). ci_fast_gate launched.
