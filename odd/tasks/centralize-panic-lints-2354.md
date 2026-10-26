# Mission: centralize panic lints in workspace lints and cover MCP bins

Issue: #2354 (approved, `type:chore`) — Worktree: `chore-centralize-panic-lints-2354`

## Specs

- S1: "Mover los `deny(clippy::unwrap_used)` / `deny(clippy::expect_used)` / `deny(clippy::disallowed_types)` que hoy viven como `#![cfg_attr(not(test), ...)]` copiados a mano en cada `lib.rs` hacia `[workspace.lints]` (que está vacío), dejando `[lints] workspace = true` por crate."
- S2: "Los bins (`crates/webfang_mcp/src/bin/mcp_server_http.rs`, `mcp_server_stdio.rs`) no heredan el deny del `lib.rs`: agregarles el mismo cfg_attr."
- S3: "`ci_fast_gate` verde; un test de que un `unwrap()` nuevo en un bin falla el gate."

## Design decisions (verified empirically)

- Cargo passes `[lints]` deny as `--deny=` (plain command-line flag); a source-level
  `#![allow]` overrides it (probed with a scratch crate). Attributes > command line.
- Workspace lints cannot express `cfg_attr(not(test), ...)`. To preserve the repo's
  `not(test)` semantics (~1500 unwrap/expect uses in integration tests), the deny goes
  unconditional in `[workspace.lints.clippy]` and each exempt root carries
  `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]`.
- `disallowed_types` stays in `webfang_core` only: `clippy.toml` documents it as opt-in
  per-crate (webfang_cli legitimately uses `anyhow::Error`). Workspace-wide deny would
  be a semantic change beyond the issue.
- Bins inherit package `[lints] workspace = true` (applies to all targets — probed on a
  test target), so S2's cfg_attr is superseded by inheritance; the contract test pins it.

## Tasks

- T1: `[workspace.lints.clippy]` in root Cargo.toml: `unwrap_used = deny`, `expect_used = deny` (+ comment).
- T2: `crates/webfang_core/Cargo.toml`: add `[lints] workspace = true` (missing).
- T3: Strip moved cfg_attr denies from core lib.rs / mcp lib.rs / cli main.rs; add
  `cfg_attr(test, allow)` at those roots (+ the 2 mcp bins, which have cfg(test) mods).
- T4: Harness crates (test_utils 27 uses, benchmark 3) — explicit crate-root allow with comment.
- T5: Add the test exemption line to every integration test target root (tests/*.rs).
- T6: Contract test in `crates/webfang_mcp/tests/`: pins workspace deny + every member
  inheriting `[lints] workspace = true` + bins covered (S3).
- T7: RED evidence: temporary unwrap in a bin fails `cargo clippy -p webfang_mcp --bin`.
- T8: Full gate + work-unit commits + PR.

## Log

- L1: "vamos a revisar y implementar la issue #2354 si no tiene worktree o pr abierto"
  — no PR, no worktree, no branch existed; issue approved. Worktree created from local
  main (7897173c; origin/main is 2 commits behind — local main is ahead).
- L2: Probe results: cargo `[lints]` deny is overridable by source attrs; workspace deny
  applies to test targets and bins.
