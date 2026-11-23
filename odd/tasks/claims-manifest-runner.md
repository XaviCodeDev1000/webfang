# ODD — claims-manifest-runner

Objective: finish the in-scope D2 front of issue #2995: verify and fix the remaining stale claim strings, then add one shared claim-manifest runner with regression rows. Branch `docs/claims-manifest-runner` from `main` b80247e1. Delivery strategy: single PR; runner: `bash scripts/ci_fast_gate.sh` plus `bash -n`/shellcheck and the standalone script harnesses.

## Specs

S1 — Re-verify and fix the residual stale strings that the earlier slices missed on current main. Exact strings observed on `main` b80247e1:
- `crates/webfang_core/src/domain/embedding_port.rs`: "Implementations wrap an ONNX inference pool to produce fixed-dimension" / "vectors; both supported models output 384d via Matryoshka truncation."
- `crates/webfang_core/src/infrastructure/error.rs`: "/// Middleware error (from reqwest-middleware, e.g., retry failures)"
Preserve the exact line count of every edited doc-comment: the intra-crate-lint golden records `file / line / use / path`, so a shifted line breaks `the_real_tree_parses_without_errors_and_matches_the_golden_set`. Do not regenerate the golden.

S2 — Add one shared runner `scripts/check_claim_manifest.sh` with `scripts/claims-manifest.tsv` (path | forbidden-string | source-of-truth | rationale). Issue #2995 verbatim: "a shared claim-manifest runner vs extending each of the eight guards. Recommended: shared runner after slice 1 proves the pattern." Populate regression rows only for claims already absent on main. Do NOT add rows for the two `error.rs` strings fixed in open PR #3001 until that PR merges. Reuse the existing retired-claim loop shape from `scripts/check_docs_claims.sh` and the marker parser/reporting from `scripts/check_data_flow_doc.sh`; do not reimplement their parsing. Hard scope: no `.github/`, no `deny.toml`, no `docs/`, no `CHANGELOG.md`.

S3 — Tests: `scripts/test_claim_manifest.sh` and a wiring harness. Shell-harness contract: fixtures in `mktemp -d` with `trap 'rm -rf "$TMP"' EXIT`, assert exit codes and observable report output only, inject one stale string as the RED repro, then restore. Wire the runner locally in `scripts/ci_fast_gate.sh` at the docs and guards lanes; harness discovery has no auto-discovery, so add the trigger+`run_step` entry by hand. Run `bash -n`, shellcheck with the gate's exclusion list, and both harnesses by hand — the gate lanes do not cover them.

S4 — Close with evidence: `cargo check`, `ci_fast_gate.sh` GREEN with the coverage report read, the intra-crate-lint golden test green, full pre-commit gate, and a verified per-spec verdict. Risk: item 5 (gate wiring changes local verification behavior) — one independent verifier after the change's own checks. No push, PR or merge without a fresh owner decision.

## Tasks

- T1 · S1 — route inline — residual stale strings, line counts preserved — status done — commit pending
- T2 · S2-S3 — route inline — runner + manifest + harnesses + gate wiring — status pending — commit pending
- T3 · S4 — route inline — verification, independent verifier, mirror, ODD close — status pending — commit pending

## Log

L1 — "estas autorizado continua"

L2 — Immediately preceding scope statement: "El issue #2995 marca `deny.toml` (D3) y `.github/` (D4) como **fuera de alcance**, así que no los toco. El frente en alcance que queda es D2: el runner compartido de manifiestos de claims."

L3 — Mapping handoff (gentle-ai-explore, obs 3299): eight guards are per-document silos; only `check_ai_providers_doc.sh` and `check_data_flow_doc.sh` have an in-doc `<!-- claims:begin/end -->` protocol. No shared manifest lib exists. Reuse `check_data_flow_doc.sh` marker parser and `check_docs_claims.sh` retired-claim loop. Harnesses live in `scripts/test_*.sh` and `scripts/tests/test_*.sh`, enumerated by trigger+`run_step` in `ci_fast_gate.sh` (docs block ~:697-704), mirrored in CI `repo-guards` — the latter is out of scope.

L4 — Parent spot check confirmed the map's anomaly: `embedding_port.rs` trait doc still says ONNX/384d/Matryoshka at the top (lines 39-41), although slice 2 corrected the `embed_batch` doc and the module header. `infrastructure/error.rs` still attributes `InfraError::Middleware` to reqwest-middleware. Both are genuine residuals of #2995, not stale memory. The two `error.rs` strings are fixed only in open PR #3001, so they are not yet safe as manifest rows on main.

L5 — Worktree bootstrap verified: `/home/xavi/Projects/Rust/webfang-worktrees/docs-claims-manifest-runner`, branch `docs/claims-manifest-runner`, target `$HOME/.cache/cargo-target/docs-claims-manifest-runner`, both indexes at b80247e1, identity gate OK, name proxy OK.

L6 — T1 evidence: RED observed as the three strings present on main; GREEN after replacing the `EmbeddingPort` trait doc with the remote-endpoint contract and the `InfraError::Middleware` attribution with the HTTP stack. `git diff --numstat` is 3/3 and 1/1 — line counts preserved, so the intra-crate-lint golden test stays green (`2 passed`, `cargo test --manifest-path tools/intra-crate-lint/Cargo.toml --locked --test real_tree`). `cargo check -q` exit 0.
