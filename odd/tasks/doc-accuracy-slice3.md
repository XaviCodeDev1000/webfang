# Feature: doc-accuracy-slice3
Branch: docs/doc-accuracy-slice3 · issue: #2995 (umbrella) · delivery: one commit · runner: `cargo check` + `ci_fast_gate.sh` + targeted nextest.

## Specs

- **S1** — Fix two stale doc-comments in `crates/webfang_core/src/error.rs`: the `ScraperError::Middleware` comment *"from reqwest-middleware, e.g., retry failures"* (no `reqwest-middleware` crate exists in either lockfile; `reqwest` itself enters only through optional `chromiumoxide`), and the `SemanticError::ChunkTooLarge` comment *"the same budget has to mean the same thing for a local ONNX model and a remote embedding endpoint"* (ADR-0004 step 6 deleted local inference, executed in #2072). Remove the false crate attribution and the ONNX reference; keep the contract statements that remain true.
- **S2** — Method: RED check of the two stale strings before editing, GREEN after; `cargo check` after the `.rs` edit; no behavior, API, dependency or CI change.
- **S3** — Out of scope: `deny.toml` LGPL comment (D3, needs its own owner), `.github/` (D4), shared claim-manifest runner (D2), frozen docs, and the files corrected in slices 1–2.

## Tasks

- T1 · S1-S2 — route inline — RED check + two doc-comments — status done — commit eb55e667
- T2 · S1-S3 — route inline — verification, mirror, ODD close — status done — commit pending

Route declaration: inline. Risk: none — doc-comments only; no behavior change. Rust skill `rust-best-practices` loaded before touching `.rs`.

## Log

L1 — merge quee aplicado monitoriza el merge y luego continua

L2 — Authorized continuation against the umbrella plan: slice 2 was merged as PR #2997 (c2963c68) and the next recorded front is the `error.rs` doc-comments found in slice 2's L8. Push/PR/merge of this slice remain separate owner decisions.

L3 — Verification before edit: `Cargo.lock` has zero `name = "reqwest-middleware"` entries; `cargo tree -i reqwest -p webfang_core --all-features` shows `reqwest v0.12.28` only under `chromiumoxide v0.7.0` (optional `chromium` feature), never as the project's HTTP client. `codedb word 'ScraperError::Middleware'` and `word 'Middleware('` return no hits — no construction sites. `reqwest-middleware` attribution is therefore stale, not merely unpinned.

L4 — Slice 2 post-merge cleanup done from main before this worktree: verified MERGED, ff-only f0b524c→c2963c68, worktree/branch/target removed. A foreign unstaged deletion (`.pi-lens.json`) from another agent was present in main and deliberately left untouched.

L5 — T1 evidence: RED 2/2 stale strings present. `ScraperError::Middleware` lost the false `reqwest-middleware` attribution; `SemanticError::ChunkTooLarge` lost the local-ONNX reference. First attempt shortened the comment from three lines to two, which shifted line numbers in the intra-crate-lint golden (it records `file / line / use / path`) and failed `the_real_tree_parses_without_errors_and_matches_the_golden_set`; the comment was rewritten to the SAME three-line shape so the golden matches without touching the golden file. Commit eb55e667 (amended, unpushed).

L6 — Verification: GREEN 2/2; `cargo check` exit 0; RDD assess `risk: medium` (`executable_change` on error.rs), `review_due: false`, `under_budget`. `ci_fast_gate.sh` GREEN (PASS=46 FAIL=0 SKIP=3) after forcing a real-cargo rebuild. Full `cargo nextest run -p webfang_core --lib` exit 0, 3121/3121 PASS.

L7 — Environment lesson: a throwaway worktree at the base commit was created to prove the golden failure was mine, and its CARGO_TARGET_DIR pointed at this slice's target. mbx then served the compiled `real_tree` test binary with the throwaway worktree's absolute path baked in; after deleting the throwaway worktree the gate failed with "the real tree is missing: .../tmp-golden-base/...". Fix: remove `debug/deps/real_tree-*` and rerun the gate with the mbx shim removed from PATH so cargo recompiles locally. Never share a target dir with a throwaway worktree whose compiled artifacts embed absolute paths.
