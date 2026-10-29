#!/usr/bin/env bash
set -euo pipefail
#
# ci_queue_metrics.sh — #2284 queue-wait metrics (read-only).
#
# Measures how long work waits BEFORE it starts running, the number the
# queue-tuning knobs (#2284: min_entries_to_merge, wait minutes,
# check_response_timeout, max_entries_to_build) are supposed to move:
#
#   - workflow queue wait: run.startedAt - run.createdAt (one bulk call);
#   - job queue wait: job.started_at - job.created_at (per-run jobs API,
#     bounded to the most recent --jobs-runs completed runs);
#   - macOS lane specifically: job queue wait for jobs whose runs-on lands
#     on a macos-* hosted runner (the lane with the historically longest
#     hosted-queue waits);
#   - merge-queue lane: the ONLY runs whose wait the queue knobs move are
#     the `--event merge_group` runs on `gh-readonly-queue/main/pr-*`
#     branches, which the --branch filter never matches. Two measures:
#     dispatch wait (merge_group startedAt - createdAt) and per-PR residence
#     (the PR timeline's added_to_merge_queue -> merged events), bounded to
#     the most recent --queue-prs PRs seen in those runs;
#   - p50/p95 by nearest-rank on sorted samples, exactly like ci_metrics.sh.
#
# Output: a markdown table block on stdout (meant for $GITHUB_STEP_SUMMARY
# in the ci-health workflow) plus a one-line machine-readable summary.
#
# Fail-closed rules:
#   - every fetched run/jobs/timeline payload that cannot be parsed is
#     COUNTED as unparseable and reported, never silently dropped;
#   - zero valid samples prints `unknown`, never an invented number;
#   - a missing `startedAt` (run not yet started) excludes the run from the
#     queue-wait sample and is reported as in-flight;
#   - a PR without an enqueue event, or without a merge event, is excluded
#     from the residence sample and reported.
#
# Exit codes:
#   0  metrics printed (unavailable pieces -> `unknown`)
#   2  usage error, gh/jq missing, repo identity unreadable
#
# Scope: read-only (gh run list + gh api jobs/timeline). Never mutates.
set -euo pipefail

DAYS=14
WORKFLOW="ci.yml"
BRANCH="main"
JOBS_RUNS=50
QUEUE_PRS=50

usage() {
  sed -n '2,41p' "$0"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --days) DAYS="${2:?missing value for --days}"; shift 2 ;;
    --workflow) WORKFLOW="${2:?missing value for --workflow}"; shift 2 ;;
    --branch) BRANCH="${2:?missing value for --branch}"; shift 2 ;;
    --jobs-runs) JOBS_RUNS="${2:?missing value for --jobs-runs}"; shift 2 ;;
    --queue-prs) QUEUE_PRS="${2:?missing value for --queue-prs}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
done

if ! [[ "$DAYS" =~ ^[0-9]+$ ]] || [[ "$DAYS" -lt 1 ]]; then
  echo "error: --days must be a positive integer (got '$DAYS')" >&2
  exit 2
fi
if ! [[ "$JOBS_RUNS" =~ ^[0-9]+$ ]] || [[ "$JOBS_RUNS" -lt 1 ]]; then
  echo "error: --jobs-runs must be a positive integer (got '$JOBS_RUNS')" >&2
  exit 2
fi
if ! [[ "$QUEUE_PRS" =~ ^[0-9]+$ ]] || [[ "$QUEUE_PRS" -lt 1 ]]; then
  echo "error: --queue-prs must be a positive integer (got '$QUEUE_PRS')" >&2
  exit 2
fi

for tool in gh jq; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "error: $tool is required but not installed" >&2
    exit 2
  }
done

REPO="$(gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>/dev/null || true)"
if [[ -z "$REPO" ]]; then
  echo "error: could not determine owner/repo (gh repo view failed)" >&2
  exit 2
fi

# --- helpers -----------------------------------------------------------------

# nearest-rank percentile over a sorted (ascending, newline-separated) list
# of integer seconds; percentile rank = ceil(p*n), minimum 1 (ci_metrics.sh
# precedent). p is 50 or 95.
percentile() {
  local p="$1"
  # -c (not a heredoc): a heredoc would replace the function's stdin, and the
  # samples arrive exactly through stdin.
  python3 -c '
import math, sys
p = float(sys.argv[1])
vals = [int(x) for x in sys.stdin.read().split() if x.strip()]
if not vals:
    print("unknown")
else:
    vals.sort()
    rank = max(1, math.ceil(p * len(vals) / 100))
    print(vals[rank - 1])
' "$p"
}

mins() {
  local secs="$1"
  if [[ "$secs" == "unknown" ]]; then
    echo "unknown"
  else
    python3 -c "print(f'{$secs/60:.1f}')"
  fi
}

epoch_of() {
  # ISO-8601 -> epoch seconds; prints nothing (caller sees empty) on garbage.
  python3 -c "
import sys, datetime
try:
    dt = datetime.datetime.fromisoformat(sys.argv[1].replace('Z', '+00:00'))
    print(int(dt.timestamp()))
except Exception:
    pass
" "$1" 2>/dev/null || true
}

# --- run-level queue wait (bulk) ---------------------------------------------
RUNS_JSON="$(gh run list --workflow "$WORKFLOW" --branch "$BRANCH" \
  --limit 200 --json databaseId,createdAt,startedAt,event,conclusion 2>/dev/null || true)"

CUTOFF="$(python3 -c "import time; print(int(time.time()) - $DAYS*86400)")"

W_UNPARSEABLE=0
W_INFLIGHT=0
W_OUTSIDE=0
W_SAMPLES=""
W_EVENTS='{}'

if [[ -n "$RUNS_JSON" ]] && echo "$RUNS_JSON" | jq -e 'type == "array"' >/dev/null 2>&1; then
  while IFS=$'\x1f' read -r created started event; do
    CE="$(epoch_of "$created")"
    if [[ -z "$CE" ]]; then
      W_UNPARSEABLE=$((W_UNPARSEABLE + 1))
      continue
    fi
    if [[ "$CE" -lt "$CUTOFF" ]]; then
      W_OUTSIDE=$((W_OUTSIDE + 1))
      continue
    fi
    W_EVENTS="$(jq -c --arg e "${event:-unknown}" '.[$e] = ((.[$e] // 0) + 1)' <<<"$W_EVENTS")"
    if [[ -z "$started" || "$started" == "null" ]]; then
      W_INFLIGHT=$((W_INFLIGHT + 1))
      continue
    fi
    SE="$(epoch_of "$started")"
    if [[ -z "$SE" ]]; then
      W_UNPARSEABLE=$((W_UNPARSEABLE + 1))
      continue
    fi
    WAIT=$((SE - CE))
    if [[ "$WAIT" -lt 0 ]]; then
      W_UNPARSEABLE=$((W_UNPARSEABLE + 1))
      continue
    fi
    W_SAMPLES+="$WAIT"$'\n'
  done < <(echo "$RUNS_JSON" | jq -r '.[] | [(.createdAt // ""), (.startedAt // ""), (.event // "")] | join("\u001f")')
else
  W_UNPARSEABLE=$((W_UNPARSEABLE + 1))
fi

# --- job-level queue wait (bounded per-run calls) ----------------------------
JOB_IDS="$(echo "$RUNS_JSON" | jq -r 'type == "array" and length > 0 | "ok"' >/dev/null 2>&1 \
  && echo "$RUNS_JSON" | jq -r '[.[] | select(.conclusion != null and .conclusion != "" and .conclusion != "skipped")] | sort_by(.databaseId) | reverse | .[:'"$JOBS_RUNS"'][] | .databaseId' 2>/dev/null || true)"

J_UNPARSEABLE=0
J_SAMPLES=""
J_MACOS_SAMPLES=""
J_RUNS_FETCHED=0
J_RUNS_FAILED=0

if [[ -n "$JOB_IDS" ]]; then
  while IFS= read -r run_id; do
    [[ -n "$run_id" ]] || continue
    JOBS_JSON="$(gh api "repos/$REPO/actions/runs/$run_id/jobs?per_page=100" 2>/dev/null || true)"
    if [[ -z "$JOBS_JSON" ]] || ! echo "$JOBS_JSON" | jq -e '.jobs | type == "array"' >/dev/null 2>&1; then
      J_RUNS_FAILED=$((J_RUNS_FAILED + 1))
      continue
    fi
    J_RUNS_FETCHED=$((J_RUNS_FETCHED + 1))
    # shellcheck disable=SC2034  # runner_name kept positional for clarity
    while IFS=$'\x1f' read -r created started _runner labels; do
      CE="$(epoch_of "$created")"
      SE="$(epoch_of "$started")"
      if [[ -z "$CE" || -z "$SE" || -z "$started" || "$started" == "null" ]]; then
        J_UNPARSEABLE=$((J_UNPARSEABLE + 1))
        continue
      fi
      WAIT=$((SE - CE))
      if [[ "$WAIT" -lt 0 ]]; then
        J_UNPARSEABLE=$((J_UNPARSEABLE + 1))
        continue
      fi
      J_SAMPLES+="$WAIT"$'\n'
      if grep -qiE 'macos' <<<"$labels"; then
        J_MACOS_SAMPLES+="$WAIT"$'\n'
      fi
    done < <(echo "$JOBS_JSON" | jq -r \
      '.jobs[] | [(.created_at // ""), (.started_at // ""), (.runner_name // ""), ([.labels[]?] | join(","))] | join("\u001f")')
  done <<<"$JOB_IDS"
fi

# --- merge-queue lane (#2284) -------------------------------------------------
# The knobs move the merge_group runs' wait, which `--branch` never matches:
# those runs execute on `gh-readonly-queue/main/pr-*` branches. Two measures:
#   - dispatch wait: merge_group run startedAt − createdAt (enqueue → CI
#     start of the group the PR was batched into);
#   - residence per PR: the PR timeline's `added_to_merge_queue` → `merged`
#     events, the only durable record of when a PR entered the queue.
MQ_RUNS_JSON="$(gh run list --workflow "$WORKFLOW" --event merge_group \
  --limit 200 --json databaseId,createdAt,startedAt,headBranch,conclusion 2>/dev/null || true)"

MQ_UNPARSEABLE=0
MQ_INFLIGHT=0
MQ_OUTSIDE=0
MQ_SAMPLES=""
MQ_HEADS=""

if [[ -n "$MQ_RUNS_JSON" ]] && echo "$MQ_RUNS_JSON" | jq -e 'type == "array"' >/dev/null 2>&1; then
  while IFS=$'\x1f' read -r created started head_branch; do
    CE="$(epoch_of "$created")"
    if [[ -z "$CE" ]]; then
      MQ_UNPARSEABLE=$((MQ_UNPARSEABLE + 1))
      continue
    fi
    if [[ "$CE" -lt "$CUTOFF" ]]; then
      MQ_OUTSIDE=$((MQ_OUTSIDE + 1))
      continue
    fi
    if [[ -z "$started" || "$started" == "null" ]]; then
      MQ_INFLIGHT=$((MQ_INFLIGHT + 1))
      continue
    fi
    SE="$(epoch_of "$started")"
    if [[ -z "$SE" ]]; then
      MQ_UNPARSEABLE=$((MQ_UNPARSEABLE + 1))
      continue
    fi
    WAIT=$((SE - CE))
    if [[ "$WAIT" -lt 0 ]]; then
      MQ_UNPARSEABLE=$((MQ_UNPARSEABLE + 1))
      continue
    fi
    MQ_SAMPLES+="$WAIT"$'\n'
    MQ_HEADS+="$head_branch"$'\n'
  done < <(echo "$MQ_RUNS_JSON" | jq -r '.[] | [(.createdAt // ""), (.startedAt // ""), (.headBranch // "")] | join("\u001f")')
else
  MQ_UNPARSEABLE=$((MQ_UNPARSEABLE + 1))
fi

# One PR can carry several merge_group runs (one per workflow); dedupe.
QUEUE_PR_IDS="$(grep -oE 'pr-[0-9]+' <<<"$MQ_HEADS" 2>/dev/null | sed 's/^pr-//' | sort -un | head -n "$QUEUE_PRS" || true)"

R_UNPARSEABLE=0
R_SAMPLES=""
R_FETCHED=0
R_FAILED=0
R_NO_ENQUEUE=0
R_NOT_MERGED=0
R_NEGATIVE=0

if [[ -n "$QUEUE_PR_IDS" ]]; then
  while IFS= read -r pr; do
    [[ -n "$pr" ]] || continue
    TIMELINE_JSON="$(gh api "repos/$REPO/issues/$pr/timeline?per_page=100" 2>/dev/null || true)"
    if [[ -z "$TIMELINE_JSON" ]] || ! echo "$TIMELINE_JSON" | jq -e 'type == "array"' >/dev/null 2>&1; then
      R_FAILED=$((R_FAILED + 1))
      continue
    fi
    R_FETCHED=$((R_FETCHED + 1))
    # Enqueue = the LAST added_to_merge_queue event (a PR may be re-enqueued
    # after a failed group); merge = the merged event. Negative or absent
    # pieces are accounted for, never silently dropped.
    read -r enqueued merged unparseable <<<"$(echo "$TIMELINE_JSON" | jq -r \
      '([.[] | select(.event == "added_to_merge_queue") | .created_at] | last // "none") + " " +
       ([.[] | select(.event == "merged") | .created_at] | last // "none") + " " +
       ([.[] | select(.created_at == null)] | length | tostring)' 2>/dev/null || echo "none none 1")"
    ENQ="$(epoch_of "$enqueued")"
    MRG="$(epoch_of "$merged")"
    if [[ -z "$ENQ" || "$enqueued" == "none" ]]; then
      R_NO_ENQUEUE=$((R_NO_ENQUEUE + 1))
      continue
    fi
    if [[ -z "$MRG" || "$merged" == "none" ]]; then
      R_NOT_MERGED=$((R_NOT_MERGED + 1))
      continue
    fi
    if [[ -n "$unparseable" && "$unparseable" != "0" ]]; then
      R_UNPARSEABLE=$((R_UNPARSEABLE + unparseable))
    fi
    RESIDENCE=$((MRG - ENQ))
    if [[ "$RESIDENCE" -lt 0 ]]; then
      R_NEGATIVE=$((R_NEGATIVE + 1))
      continue
    fi
    R_SAMPLES+="$RESIDENCE"$'\n'
  done <<<"$QUEUE_PR_IDS"
fi

MQ_P50="$(percentile 50 <<<"$MQ_SAMPLES")"
MQ_P95="$(percentile 95 <<<"$MQ_SAMPLES")"
R_P50="$(percentile 50 <<<"$R_SAMPLES")"
R_P95="$(percentile 95 <<<"$R_SAMPLES")"
MQ_N="$(grep -c . <<<"$MQ_SAMPLES" 2>/dev/null || true)"
R_N="$(grep -c . <<<"$R_SAMPLES" 2>/dev/null || true)"

# --- report ------------------------------------------------------------------
W_P50="$(percentile 50 <<<"$W_SAMPLES")"
W_P95="$(percentile 95 <<<"$W_SAMPLES")"
J_P50="$(percentile 50 <<<"$J_SAMPLES")"
J_P95="$(percentile 95 <<<"$J_SAMPLES")"
M_P50="$(percentile 50 <<<"$J_MACOS_SAMPLES")"
M_P95="$(percentile 95 <<<"$J_MACOS_SAMPLES")"

W_N="$(grep -c . <<<"$W_SAMPLES" 2>/dev/null || true)"
J_N="$(grep -c . <<<"$J_SAMPLES" 2>/dev/null || true)"
M_N="$(grep -c . <<<"$J_MACOS_SAMPLES" 2>/dev/null || true)"

cat <<EOF
## Queue-wait metrics — \`$WORKFLOW\` on \`$BRANCH\` + merge_group lane (last $DAYS days)

| Metric | p50 | p95 | n |
|---|---|---|---|
| Workflow queue wait (startedAt − createdAt) | $(mins "$W_P50") min | $(mins "$W_P95") min | $W_N |
| Job queue wait (started_at − created_at) | $(mins "$J_P50") min | $(mins "$J_P95") min | $J_N |
| macOS job queue wait | $(mins "$M_P50") min | $(mins "$M_P95") min | $M_N |
| Merge-queue dispatch wait (merge_group startedAt − createdAt) | $(mins "$MQ_P50") min | $(mins "$MQ_P95") min | $MQ_N |
| Merge-queue residence per PR (added_to_merge_queue → merged) | $(mins "$R_P50") min | $(mins "$R_P95") min | $R_N |

- Sample accounting: runs outside window $W_OUTSIDE, in-flight (no startedAt) $W_INFLIGHT, unparseable run rows $W_UNPARSEABLE; jobs fetched from $J_RUNS_FETCHED/$JOBS_RUNS completed runs, jobs API failures $J_RUNS_FAILED, unparseable job rows $J_UNPARSEABLE; merge_group runs outside window $MQ_OUTSIDE, in-flight $MQ_INFLIGHT, unparseable $MQ_UNPARSEABLE; PR timelines fetched $R_FETCHED/$QUEUE_PRS queue runs, timeline failures $R_FAILED, PRs without an enqueue event $R_NO_ENQUEUE, PRs not merged $R_NOT_MERGED, negative waits $R_NEGATIVE.
- Events seen: $W_EVENTS
- p50/p95 use nearest-rank on integer seconds; "unknown" means zero valid samples (never invented).
- The main-lane rows measure \`--branch $BRANCH\`; the merge-queue rows measure \`--event merge_group\` runs (head branch \`gh-readonly-queue/main/pr-*\`), which the branch filter alone never matches (#2284: the only runs whose wait the queue knobs move).
EOF

echo "queue-metrics: workflow p50=$(mins "$W_P50")m p95=$(mins "$W_P95")m (n=$W_N); job p50=$(mins "$J_P50")m p95=$(mins "$J_P95")m (n=$J_N); macos p50=$(mins "$M_P50")m p95=$(mins "$M_P95")m (n=$M_N); merge-dispatch p50=$(mins "$MQ_P50")m p95=$(mins "$MQ_P95")m (n=$MQ_N); merge-residence p50=$(mins "$R_P50")m p95=$(mins "$R_P95")m (n=$R_N)"
