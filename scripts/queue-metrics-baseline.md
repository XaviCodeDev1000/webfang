# Merge-queue wait baseline (#2284)

Durable record of the measurement the merge-queue knob decision
(`max_entries_to_build`) is made against. Regenerate with:

```bash
bash scripts/ci_queue_metrics.sh --days 14 --queue-prs 25
```

The daily `queue-metrics` job in `ci-health.yml` publishes the same table to
its step summary; this file exists so the baseline survives workflow runs
(the step summary is ephemeral, and the pre-#2284 lane numbers were zero by
construction — pushes to `main` do not queue).

## Baseline — 2026-10-29

- Window: last 14 days (`--days 14`), measured 2026-10-29 from the repo at
  commit `0920e254`.
- Ruleset state during the window: `main-protection-migrated` moved
  `min_entries_to_merge` 3 → 1 and `min_entries_to_merge_wait_minutes` 5 → 0
  on **2026-10-28** — the window is therefore mostly post-change; the
  residence numbers below are the AFTER side of the knob change. A BEFORE
  side with this methodology does not exist: the old `ci_queue_metrics.sh`
  measured the `--branch main` lane only, where the wait is zero by
  construction.

| Metric | p50 | p95 | n |
|---|---|---|---|
| Workflow queue wait (main lane, startedAt − createdAt) | 0.0 min | 0.0 min | 200 |
| Job queue wait (main lane) | 0.0 min | 2.2 min | 1278 |
| macOS job queue wait | 0.1 min | 5.1 min | 28 |
| Merge-queue dispatch wait (merge_group startedAt − createdAt) | 0.0 min | 0.0 min | 112 |
| **Merge-queue residence per PR (added_to_merge_queue → merged)** | **10.8 min** | **12.5 min** | 25 |

## How to read it

- **Dispatch wait ≈ 0** with n=112: GitHub creates and starts merge_group
  runs immediately. The queue's cost is NOT dispatch latency.
- **Residence (10.8/12.5 min)** is enqueue → merge for a PR: it contains the
  group's CI testing time plus any batching wait. This is the number the
  `max_entries_to_build` knob moves: raising it to 2 lets a PR ride an
  in-flight group instead of waiting for the next build slot, at the cost of
  occasionally rebuilding both entries when the first fails.
- p50/p95 are nearest-rank over integer seconds, computed by
  `scripts/ci_queue_metrics.sh`; `n` is the sample count, and the script's
  accounting line is the source of truth for what was included.
- Decision pending on this data: whether to raise `max_entries_to_build`
  from 1 to 2 in the ruleset. Do not raise it on intuition — re-measure
  after the window accumulates, and put the before/after table in the PR
  that changes the ruleset.
