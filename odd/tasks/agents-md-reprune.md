# Feature: AGENTS.md re-prune to the owner-approved budget (Slice 1 of 2)

**Issue:** #2832 — requires the owner's `status:approved` before the PR is opened.
**Labels:** `type:docs`
**Branch:** `docs/agents-reprune` (worktree `~/Projects/Rust/webfang-worktrees/docs-agents-reprune`), base `origin/main`
**Delivery route:** sequential slices — Slice 1 (this PR) prunes; Slice 2 (`chore/agents-doc-guard`) adds the fail-closed size/structure guard after Slice 1 merges. No stacked PRs.
**Scope:** one repo file (`AGENTS.md`) + this mission record. No production code change, no `docs/` change, no `CHANGELOG.md` change.

## Objective

`AGENTS.md` regrew from 740 lines (post-#2208 router) to 862 lines / 52,832 bytes by agents
appending operational runbook prose on every commit. The owner approved (AskUserQuestion,
2026-11-16):

1. **Budget:** moderate — ≤650 lines / ≤40 KiB (40,960 bytes).
2. **Linter scope (Slice 2):** size + structure — line/byte budget, paired fences, zero
   line-number citations.

This slice restores the router: keep ALL rules (including the worktree-policy rules and the
Intelligence Stack, per S4 of `odd/tasks/agents-md-routerization.md`), condense the operational
runbook to verified Engram pointers.

## What moved where (S2/S5: nothing pruned without a verified destination)

| Block | Destination |
|---|---|
| `ci_fast_gate` coverage-report detail + deliberate local exceptions | **obs #2980** (new, query `ci_fast_gate coverage report gate.needs lane partial`) + `scripts/ci_fast_gate_coverage.tsv`, `scripts/test_ci_gate_structure.sh` |
| Worktree bootstrap heredoc, post-merge 7-step runbook, shared-resources table, namespace detail | **obs #2307** (holds it verbatim) |
| Changelog/squash measured examples, watcher-fallback incident narrative, breaking-surface CI-job operational detail | **obs #2309 / #2316** + `scripts/breaking_surface_lib.sh`, `.github/merge-queue.json` |
| Error-funnel diagram, fetch-chain "why" prose | condensed in place; the normative tables and all rules stay |

The `Consulting Engram` table gains the #2980 row. The four project skills merged in #2829
(`webfang-worktree-ops`, `webfang-ci-gate`, `webfang-testing`, `webfang-fetch-path`) are the
trigger-loaded actionable layer complementing these pointers; they are registered in § Skills
and CLIs.

## Verification (obs #2313 lesson: a rewrite is not a reordering — every claim re-checked)

- Size: **647 lines / 40,957 bytes** (≤650 / ≤40,960). Fences: 14 (even). Zero line-number
  citations (regex over file-extension anchors). All 14 H2 render.
- Every cited repo path exists; two stale citations corrected against the tree:
  `domain/entities.rs` → `domain/entities/` (now a directory module) and `read_body_capped`
  attributed to `wreq_downloader/wreq_body.rs` (extracted sibling module), via codedb.
- Every cited symbol verified with codedb (`BehavioralTest`, `webfang_path`,
  `redact_nondeterministic`, `ValidUrl`/`try_from_url`, `fetch_inner`, `redirect_policy`,
  `ssrf_guard`, `read_body_capped`, `DEFAULT_MAX_PAGE_BYTES` = 50 MiB, `log_scrape_error`,
  `CorrelationId`, `CpuBridge`, `until_ready_or_cancel_observed`, `build_scrape_rate_limiter`,
  `RateLimiterConfig`, `FileTraceLayer`, `EXIT_CONFIG` = 78, `scrape_urls`, `MockClock`,
  `WEBFANG_EMBEDDING_PROVIDER`, `BodyTooLarge`, `infrastructure/stream`).
- All 14 Engram table queries resolve (`engram search … --project webfang`), including the new
  #2980 row.
- `bash scripts/ci_fast_gate.sh` GREEN (docs lane); coverage report read.

## Log

- **L1 (2026-11-16)** — Owner decisions captured (budget moderate ~650/~40 KiB; linter = size +
  structure). Skills track (#2829) delivered first; this slice resumes from the merged main
  (`c12d1ac8`) — merge conflict in § Skills and CLIs resolved keeping the pruned prose + the
  four new skill names.
- **L2 (2026-11-16)** — Prune executed in three passes: runbook blocks → Engram pointers,
  deduplication (pocket rule, integration-test recipe bullet, ✅/❌ example block inlined),
  prose compression. No rule removed; the obs #370 bullet was briefly dropped by a bad edit
  and restored immediately.