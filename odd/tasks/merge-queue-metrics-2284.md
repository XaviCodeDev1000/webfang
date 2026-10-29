# Mission: merge-queue-metrics-2284

Branch: `chore/merge-queue-metrics-2284` · Issue: #2284 (status:approved, REWRITTEN by the maintainer) · PR base: `main`

## Specs

- S1: "#2284 remitente — el tuning necesita medir 10–20 PRs antes/después; es naturalmente lento, arrancalo temprano pero cierra último." (Fase 4 — Chores)
- S2: Rewritten issue #2284 remaining scope: (1) extend ci_queue_metrics.sh to the merge_group lane (createdAt→startedAt, per-PR enqueue→merge from the PR timeline; --branch must stop excluding gh-readonly-queue/*), (2) durable in-repo baseline table with window and n, (3) decide max_entries_to_build: 2 on that data with the before/after table in the PR body. The knob half is DONE in the ruleset (2026-10-28) — DO NOT re-apply.

## Tasks

- T1: ci_queue_metrics.sh merge_group lane — DONE (dispatch wait + per-PR residence from added_to_merge_queue→merged timeline events, --queue-prs bound, fail-closed accounting for no-enqueue/not-merged/failed timelines/negative waits).
- T2: queue_metrics_fake_gh.sh + test_ci_queue_metrics.sh — DONE (fake dispatches on --event and serves timelines; 2 new fixture cases; 20/20 assertions green).
- T3: Durable baseline — DONE (scripts/queue-metrics-baseline.md, measured 2026-10-29: residence p50=10.8m p95=12.5m n=25; dispatch ≈ 0; main-lane rows match the issue's own table).
- T4: max_entries_to_build decision — DEFERRED by the issue's own sequencing: needs the window to accumulate post-ruleset data; the baseline file records how to decide.

## Log

- L1: Fase 4 assignment. Baseline collection started early: v1 collector measured the WRONG lane (pull_request) — everything 0s; v2 measured merge_group runs (ready→test_start→merged) — findings fed the issue rewrite the maintainer made.
- L2: Timeline probe: GitHub exposes `added_to_merge_queue` / `merged` / `removed_from_merge_queue` events — the durable enqueue/merge record the issue asked for.
- L3: GOTCHA: merge_group runs execute on gh-readonly-queue/main/pr-* branches; gh run list --branch main never matches them; the correct filter is --event merge_group.
