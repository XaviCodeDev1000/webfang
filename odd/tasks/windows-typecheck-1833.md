# Mission: windows-typecheck-1833

Branch: `chore/windows-typecheck-1833` · Issue: #1833 (status:approved) · PR base: `main`

## Specs

- S1: "#1833 — type-check de cfg(windows) en local. Toca gates de CI/scripts: espera a que #2475 (en vuelo, también toca guards) esté mergeada para no rebasear." (Fase 4 — Chores). #2475 CLOSED / PR #2477 merged 2026-10-29 17:10 — desbloqueado.
- S2: Issue #1833 body: scripts/check_windows_platform.sh (scratch crate under $TMPDIR, path-mirrored shim for crate::application::crawler::ports::CTRL_C_EVENT_NAME, mirrored workspace lint table, cargo check --target x86_64-pc-windows-msvc, fail closed naming `rustup target add`, no MSVC toolchain, no linking) + wire into ci_fast_gate.sh or document in AGENTS.md.

## Tasks

- T1: scripts/check_windows_platform.sh — DONE (path-mirrored shim instead of sed rewrite; deps extracted live from the workspace root so nothing rots; lints mirrored verbatim from [workspace.lints]).
- T2: scripts/tests/test_check_windows_platform.sh — DONE (4 cases: missing-target loud refusal via PATH-stubbed rustup; real-module GREEN; injected cfg(windows) type error RED; bare unsafe outside the module's scoped allows RED). --offline mode for the gate's stub-cargo fixtures.
- T3: ci_fast_gate.sh wiring — DONE (lane_ci semantics harness --offline; code lane + full lane scoped to infrastructure/platform/ or the guard files).
- T4: AGENTS.md — DONE (Commands + project rule).
- T5: commit + PR.

## Log

- L1: Fase 4 assignment; unblocked by #2477 merge.
- L2: GOTCHAS paid for: (1) the module also uses `futures` (qualified path, not a `use`) — dep extraction list needed it; (2) mbx mishandles MSVC std resolution for a non-host target (E0463) — the script pins (cargo, rustc) to the SAME toolchain bin dir via `rustup which` + RUSTC override, because cargo invokes `rustc` from PATH where shims may resolve another toolchain (1.91.0 lacks the MSVC std; 1.91.1, the repo pin, has it); (3) an mbx-then-real-cargo mix poisons the shared scratch target dir — cleaned once, and the script now always bypasses mbx so it cannot recur.
- L4: rustup target add x86_64-pc-windows-msvc installed on 1.91.1 locally (std only).
- L5: gate harness `62→64 ok, 5 fail` — the 5 (B51×2, A15×3) are PRE-EXISTING on main (shellcheck step in the fixture), verified by running the same harness on main@aa19f2e9; zero regressions.
