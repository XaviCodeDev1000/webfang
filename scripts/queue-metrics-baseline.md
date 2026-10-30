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

## Decision — 2026-10-30: `max_entries_to_build` 1 → 2

Re-measured with the accumulated window (defaults: `bash
scripts/ci_queue_metrics.sh --days 14`, n=49 PRs / 129 merge_group runs;
accounting line in the run log: 0 timeline failures, 1 jobs API failure,
1 PR not merged, 0 negative waits):

| Metric | p50 | p95 | n |
|---|---|---|---|
| Merge-queue residence per PR (enqueue → merged) | 9.3 min | **17.1 min** | 49 |
| Merge_group run duration | 11.1 min | 34.2 min | 129 |
| Runs started while another was still in flight | — | 29% (37/129) | 129 |

Contention is real, not theoretical: ~16 merge_group runs/day, and 29% of
them start before the previous one finishes — the 2026-10-29 burst (4 PRs
in 26 minutes) was not an outlier.

**Applied on 2026-10-30** to ruleset `main-protection-migrated` (id
25720640): `max_entries_to_build: 2`. Kept unchanged:
`min_entries_to_merge: 1`, `min_entries_to_merge_wait_minutes: 0`,
`max_entries_to_merge: 5`, `grouping_strategy: ALLGREEN`,
`check_response_timeout_minutes: 30`.

- **Why this direction is latency-neutral**: with `min_entries_to_merge:
  1` and wait 0, a solo PR merges exactly as before (group of 1); under a
  burst the second entry can be checked concurrently instead of strictly
  serializing behind the first. The side benefit (fewer runner cycles
  under contention) is incidental — on a public repo runner minutes are
  not the currency; residence latency is (#2284).
- **Why `min_entries_to_merge: 2` was rejected**: the wait parameter is
  how long a batch waits to reach the minimum group size — after it
  elapses the minimum is ignored and a smaller group merges. With wait 0
  a minimum of 2 would be ignored immediately (no-op); making it bite
  requires `min_entries_to_merge_wait_minutes > 0`, which taxes every
  solo merge to save runner cycles the issue itself declares irrelevant.
- **Criterion reframe**: #2284's original "raise after p95 drops" has no
  BEFORE side — the pre-#2515 script measured the `main` lane only, where
  the wait is zero by construction. The anchor actually used: the
  absolute p95 (17.1 min) sits far under the 30-minute check timeout, and
  CI itself is ~11 min of that residence.
- **Post-change monitoring**: re-run the script as the window accumulates
  and append a dated row here. If residence p95 degrades materially
  toward the timeout (pair-failure re-queues would show up there first),
  revisit this knob.
