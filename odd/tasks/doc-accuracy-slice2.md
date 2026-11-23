# Feature: doc-accuracy-slice2
Branch: docs/doc-accuracy-slice2 · issue: #2995 (umbrella) · delivery: one work-unit commit per surface · runner: `cargo check` for the Rust surface + `ci_fast_gate.sh` + full pre-commit gate.

## Specs

- **S1** — Fix `COMPATIBILITY-MATRIX.md`: *"`CrawlCheckpoint` (JSON+CRC32, `checkpoint_interval=100`) is engine-internal, not wired to `--resume`"* is false. Verified wiring: `restore_checkpoint_state` is called from `run` in `application/crawler/engine/lifecycle.rs`, and `load_checkpoint_state` from `with_checkpoint` in `engine/checkpoint.rs`; the corrected AGENTS.md audit (routerization S9) lists the claim as false. Rewrite to the verified split: engine-internal scheduling state restored by `run()` on `--resume`, with per-URL export progress tracked by `StateStore` / `ExportState` (JSON).
- **S2** — Fix three doc-comments in `crates/webfang_core/src/domain/embedding_port.rs`: *"Returns a fixed-dimension vector (384d for both Granite models)."*, *"Implementations may override this to use batched ONNX inference for better throughput."*, and *"Both supported models (Granite-97M and Granite-311M) return 384."* ADR-0004 step 6 removed local inference (`6a, full deletion`, executed in #2072); the port is remote/BYO and `model_id` already says *"or the provider-declared model name for remote endpoints"*. Replace with adapter-declared dimension / backend batching, with no model names or numbers.
- **S3** — Method: RED check of the four stale strings before editing, GREEN after; `cargo check` after the Rust doc-comment edit; no dependency, CI or feature change. Newly found stale `.rs` doc-comments in `crates/webfang_core/src/error.rs` (a `reqwest-middleware` attribution and a *"local ONNX model"* reference) are recorded for a later slice, not silently edited here.
- **S4** — Out of scope: `deny.toml` LGPL comment (D3), `.github/` (D4), frozen `evidence/**` / `CHANGELOG.md`, `README.md` / `NOTICE` (slice 1, already merged).

## Tasks

- T1 · S1 — route inline — COMPATIBILITY-MATRIX correction — status pending — commit pending
- T2 · S2 — route inline — EmbeddingPort doc-comments — status pending — commit pending
- T3 · S1-S4 — route inline — verification, mirror, ODD close — status pending — commit pending

Route declaration: all inline. Risk: none — prose and doc-comments only; no behavior, API, dependency or CI change. Rust skill `rust-best-practices` loaded before touching `.rs`.

## Log

L1 — ya esta mergeado, continua

L2 — Authorized plan (verbatim from the preceding answer): "Cuando se mergee, el siguiente frente del paraguas es `COMPATIBILITY-MATRIX.md` + los tres doc-comments de `EmbeddingPort`." Umbrella #2995 remains the linked issue; slice 1 merged as PR #2996 (merge commit f0b524c).

L3 — Verification before edit: `codedb callers load_checkpoint_state` → `engine/checkpoint.rs::with_checkpoint`; `codedb callers restore_checkpoint_state` → `engine/lifecycle.rs::run`; `StateStore::load_or_default` and `ExportState` confirmed in `infrastructure/export/state_store.rs` / `domain/entities/export.rs`. The COMPATIBILITY-MATRIX sentence contradicts this and the corrected AGENTS.md audit.

L4 — Post-merge cleanup of slice 1 done from main before this worktree: ff-only to f0b524c, worktree/branch/target removed. A foreign untracked file (`odd/tasks/docs-consolidation.md`, another agent's planning doc) was present in main and deliberately left untouched.
