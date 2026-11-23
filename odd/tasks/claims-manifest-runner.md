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
- T2 · S2-S3 — route inline — runner + manifest + harnesses + gate wiring — status done — commit pending
- T3 · S4 — route inline — verification, independent verifier, mirror, ODD close — status done — commit pending

## Log

L1 — "estas autorizado continua"

L2 — Immediately preceding scope statement: "El issue #2995 marca `deny.toml` (D3) y `.github/` (D4) como **fuera de alcance**, así que no los toco. El frente en alcance que queda es D2: el runner compartido de manifiestos de claims."

L3 — Mapping handoff (gentle-ai-explore, obs 3299): eight guards are per-document silos; only `check_ai_providers_doc.sh` and `check_data_flow_doc.sh` have an in-doc `<!-- claims:begin/end -->` protocol. No shared manifest lib exists. Reuse `check_data_flow_doc.sh` marker parser and `check_docs_claims.sh` retired-claim loop. Harnesses live in `scripts/test_*.sh` and `scripts/tests/test_*.sh`, enumerated by trigger+`run_step` in `ci_fast_gate.sh` (docs block ~:697-704), mirrored in CI `repo-guards` — the latter is out of scope.

L4 — Parent spot check confirmed the map's anomaly: `embedding_port.rs` trait doc still says ONNX/384d/Matryoshka at the top (lines 39-41), although slice 2 corrected the `embed_batch` doc and the module header. `infrastructure/error.rs` still attributes `InfraError::Middleware` to reqwest-middleware. Both are genuine residuals of #2995, not stale memory. The two `error.rs` strings are fixed only in open PR #3001, so they are not yet safe as manifest rows on main.

L5 — Worktree bootstrap verified: `/home/xavi/Projects/Rust/webfang-worktrees/docs-claims-manifest-runner`, branch `docs/claims-manifest-runner`, target `$HOME/.cache/cargo-target/docs-claims-manifest-runner`, both indexes at b80247e1, identity gate OK, name proxy OK.

L6 — T1 evidence: RED observed as the three strings present on main; GREEN after replacing the `EmbeddingPort` trait doc with the remote-endpoint contract and the `InfraError::Middleware` attribution with the HTTP stack. `git diff --numstat` is 3/3 and 1/1 — line counts preserved, so the intra-crate-lint golden test stays green (`2 passed`, `cargo test --manifest-path tools/intra-crate-lint/Cargo.toml --locked --test real_tree`). `cargo check -q` exit 0.

L7 — T2 evidence: `scripts/claims-manifest.tsv` has 10 active path-scoped rows (seven already-fixed NOTICE/README/COMPATIBILITY rows plus the three residuals fixed in T1; the wreq-util self-flag is excluded by scope). The two `error.rs` strings remain commented as pending activation until PR #3001 merges — they were still present on this branch's base. Reused the retired-claim loop of `check_docs_claims.sh` and the fail-closed reporting of `check_data_flow_doc.sh`; added out-of-scope and empty-manifest guards. Harness RED repro: inject `selectors 0.35.0` into a mktemp fixture → exit 1; restore → exit 0. `bash -n` clean; shellcheck with the gate exclusion list exit 0; semantics harness 8/8 and wiring harness 9/9; `ci_fast_gate.sh` GREEN (PASS=49 FAIL=0 SKIP=3). `.github/`, `deny.toml`, `docs/` and `CHANGELOG.md` untouched.

L8 — Native RDD review (global switch on, risk escalated as item 5): assess `risk=high`, `review_due=true`; consent granted by the owner for this candidate; lenses risk/readability/reliability; `review-d59a78986e9ffd6d` approved and acknowledged (`gentle-ai.review-acknowledged/v1`, authority burned). No blocker. One readability WARNING claimed the commented pending rows point to a nonexistent path — parent check showed BOTH `crates/webfang_core/src/error.rs` and `crates/webfang_core/src/infrastructure/error.rs` exist, so the finding is a false positive and no change was made. Remaining review notes are informational.

L9 — Independent verifier (obs #3307) found one real S2 blocker: a leading `./` bypassed the out-of-scope guard (`./deny.toml`, `./CHANGELOG.md`, `./.github/workflows/ci.yml` all exited 0). One correction batch: strip repeated leading `./` before the scope/traversal checks, harden the traversal case with trailing `..`, and add the seven verifier boundary probes to `scripts/test_claim_manifest.sh`. Recheck limited to the blocker: semantics 15/15, wiring 9/9, `bash -n` and shellcheck clean, runner 10 rows verified, `ci_fast_gate.sh` GREEN (PASS=49 FAIL=0 SKIP=3). Scratch `/tmp/webfang-verify-wjCXEN` removed by parent after the verifier returned. S4 gate was unverified in the verifier's no-`.git` scratch copy; the parent's real-worktree run above is the evidence.

L10 — Verdicts: S1 MET, S2 MET after the single correction, S3 MET, S4 MET. Delivery still needs a fresh owner decision for push/PR; no merge was performed.

L12 — CI follow-up after PR #3004 opened: `repo-guards` failed because the B52 fixture in `scripts/test_ci_fast_gate.sh` has a hand-maintained no-op stub list for every `lane_ci` harness; the two new harnesses were missing. Added both to that list (the same F1a pattern already documented in the harness). `bash scripts/test_ci_fast_gate.sh` → 98 ok, 0 fail, 0 skip; `ci_fast_gate.sh` → GREEN PASS=49 FAIL=0 SKIP=3.

L13 — Second CI follow-up: guard-coverage meta-guard requires every `check_*.sh` and its harness to be reachable from a workflow, `justfile` or `mise.toml`. Added a `check-docs-truth` justfile task that runs both the guard and its semantics harness, plus the required `# NEGATION:` marker in `scripts/test_claim_manifest.sh`. `test_guard_coverage.sh --no-mutation` → 33 wired guards covered; `test_ci_fast_gate.sh` → 98 ok; `ci_fast_gate.sh` → GREEN PASS=87 FAIL=0 SKIP=4. The full mutation run is not executable in this worktree: `scripts/test_intra_crate_direction.sh` preflight cannot find the extractor at its expected local path (environmental; this harness passed in CI on the earlier run), so `--no-mutation` plus CI is the evidence.
