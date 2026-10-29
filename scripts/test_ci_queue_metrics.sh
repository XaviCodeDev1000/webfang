#!/usr/bin/env bash
set -euo pipefail
#
# test_ci_queue_metrics.sh — semantics harness for scripts/ci_queue_metrics.sh
# (#2284).
#
# Pins the measurement contract by OBSERVABLE BEHAVIOUR only:
#   - nearest-rank p50/p95 math on known fixture waits (exact numbers, no
#     tolerance: ceil(0.5*n) and ceil(0.95*n) with minimum rank 1);
#   - the macOS lane filter (jobs whose labels contain macos are measured
#     separately, without leaving the overall sample);
#   - sample accounting: in-flight runs (no startedAt), unparseable payloads,
#     negative waits and jobs-API failures are COUNTED and reported, never
#     silently dropped (fail-closed accounting);
#   - zero valid samples prints `unknown`, never an invented number.
#
# Every `gh` call is served by scripts/tests/queue_metrics_fake_gh.sh from
# mktemp fixtures; this harness never touches the network and never runs
# cargo. Both scripts are invoked through `bash` on purpose: fixture trees
# (test_ci_fast_gate.sh B52) may copy scripts without preserving exec bits.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
METRICS="${QUEUE_METRICS:-$SCRIPT_DIR/scripts/ci_queue_metrics.sh}"
FAKE_GH="$SCRIPT_DIR/scripts/tests/queue_metrics_fake_gh.sh"

fail=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); echo "OK: $1"; }
bad() { echo "FAIL: $1"; fail=1; }

[ -f "$METRICS" ] || { echo "FAIL: metrics script not found at $METRICS"; exit 1; }
[ -f "$FAKE_GH" ] || { echo "FAIL: fake gh not found at $FAKE_GH"; exit 1; }

FIXROOT="$(mktemp -d)"
trap 'rm -rf "$FIXROOT"' EXIT

# Fixed timestamps: --days 36500 keeps everything inside the window.
T0="2026-10-01T10:00:00Z"

mk_fixture() { # $1 = fixture dir name
  mkdir -p "$FIXROOT/$1"
  printf '{"nameWithOwner":"acme/webfang"}\n' > "$FIXROOT/$1/repo.json"
  echo "$FIXROOT/$1"
}

run_metrics() { # $1 = fixture dir; rest: extra args
  local dir="$1"; shift
  FAKE_GH_DIR="$dir" PATH="$FIXROOT/bin:$PATH" \
    bash "$METRICS" --days 36500 "$@" 2>/dev/null
}

mkdir -p "$FIXROOT/bin"
cat > "$FIXROOT/bin/gh" <<EOF
#!/usr/bin/env bash
exec bash "$FAKE_GH" "\$@"
EOF
chmod +x "$FIXROOT/bin/gh"

# ---------------------------------------------------------------------------
# Case 1: exact nearest-rank math + macOS filter + accounting
#   runs: R1 wait 60s, R2 wait 120s, R3 wait 300s, R4 in-flight (no start)
#   -> p50 = rank ceil(0.5*3)=2 -> 120 (2.0 min); p95 = rank 3 -> 300 (5.0)
#   jobs across one completed run: waits 0,10,20,30,40,50 (macos on two)
#   -> overall p50 = rank ceil(0.5*6)=3 -> 20; p95 = rank ceil(0.95*6)=6 -> 50
#   -> macos (30,50): p50 = 30; p95 = 50
# ---------------------------------------------------------------------------
D1="$(mk_fixture case1)"
jq -n --arg t0 "$T0" '[
  {databaseId: 1, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:01:00")), event: "push", conclusion: "success"},
  {databaseId: 2, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:02:00")), event: "pull_request", conclusion: "success"},
  {databaseId: 3, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:05:00")), event: "push", conclusion: "success"},
  {databaseId: 4, createdAt: $t0, startedAt: null, event: "push", conclusion: null}
]' > "$D1/runs.json"
jq -n '{
  total_count: 6,
  jobs: [
    {name: "a", created_at: "2026-10-01T10:00:00Z", started_at: "2026-10-01T10:00:00Z", labels: ["ubuntu-latest"]},
    {name: "b", created_at: "2026-10-01T10:00:00Z", started_at: "2026-10-01T10:00:10Z", labels: ["ubuntu-latest"]},
    {name: "c", created_at: "2026-10-01T10:00:00Z", started_at: "2026-10-01T10:00:20Z", labels: ["ubuntu-latest"]},
    {name: "d", created_at: "2026-10-01T10:00:00Z", started_at: "2026-10-01T10:00:30Z", labels: ["ubuntu-latest"]},
    {name: "e", created_at: "2026-10-01T10:00:00Z", started_at: "2026-10-01T10:00:40Z", labels: ["macos-15"]},
    {name: "f", created_at: "2026-10-01T10:00:00Z", started_at: "2026-10-01T10:00:50Z", labels: ["macos-15"]}
  ]
}' > "$D1/jobs-2.json"

OUT1="$(run_metrics "$D1" --jobs-runs 50 || true)"

if echo "$OUT1" | grep -q '| 2.0 min | 5.0 min | 3 |'; then
  ok "workflow p50/p95 exact (nearest-rank over 3 samples)"
else
  bad "workflow p50/p95 wrong: $(echo "$OUT1" | sed -n 's/.*Workflow queue wait.*//p')"
fi
if echo "$OUT1" | grep -q '| 0.3 min | 0.8 min | 6 |'; then
  ok "job p50=0.3m p95=0.8m over 6 jobs"
else
  bad "job percentiles wrong"
fi
if echo "$OUT1" | grep -q '| 0.7 min | 0.8 min | 2 |'; then
  ok "macOS lane measured separately (0.7/0.8 over 2)"
else
  bad "macOS percentiles wrong"
fi
if echo "$OUT1" | grep -q 'in-flight (no startedAt) 1'; then
  ok "in-flight run counted, excluded from samples"
else
  bad "in-flight accounting wrong"
fi

# ---------------------------------------------------------------------------
# Case 2: unparseable payload is COUNTED, not dropped; script still exits 0
# ---------------------------------------------------------------------------
D2="$(mk_fixture case2)"
printf 'not-json-at-all' > "$D2/runs.json"
OUT2="$(run_metrics "$D2")"; RC2=$?
if [ "$RC2" -eq 0 ]; then
  ok "unparseable runs payload still exits 0"
else
  bad "exit $RC2 on unparseable payload"
fi
if echo "$OUT2" | grep -q 'unparseable run rows 1'; then
  ok "unparseable run row reported"
else
  bad "unparseable run row not reported"
fi
# ---------------------------------------------------------------------------
# Case 3: jobs API failure counted as a failure, other runs still measured
# ---------------------------------------------------------------------------
D3="$(mk_fixture case3)"
jq -n --arg t0 "$T0" '[
  {databaseId: 7, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:04:00")), event: "push", conclusion: "success"},
  {databaseId: 8, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:01:00")), event: "push", conclusion: "success"}
]' > "$D3/runs.json"
jq -n '{total_count: 1, jobs: [{name: "j", created_at: "2026-10-01T10:00:00Z", started_at: "2026-10-01T10:01:00Z", labels: ["ubuntu-latest"]}]}' > "$D3/jobs-7.json"
# run 8 has NO fixture -> fake gh errors -> the script must count the failure
OUT3="$(run_metrics "$D3" --jobs-runs 50)"; RC3=$?
if [ "$RC3" -eq 0 ]; then
  ok "jobs API failure still exits 0"
else
  bad "exit $RC3 on jobs API failure"
fi
if echo "$OUT3" | grep -q 'jobs API failures 1'; then
  ok "jobs API failure counted"
else
  bad "jobs API failure not counted"
fi
if echo "$OUT3" | grep -q 'jobs fetched from 1/50'; then
  ok "successful jobs fetch still counted (1/50)"
else
  bad "jobs fetch count wrong"
fi
# ---------------------------------------------------------------------------
# Case 4: zero valid samples -> unknown, never invented
# ---------------------------------------------------------------------------
D4="$(mk_fixture case4)"
jq -n '[]' > "$D4/runs.json"
OUT4="$(run_metrics "$D4")"; RC4=$?
if [ "$RC4" -eq 0 ]; then
  ok "empty corpus exits 0"
else
  bad "exit $RC4 on empty corpus"
fi
if echo "$OUT4" | grep -qE '\| unknown min \| unknown min \| 0 \|'; then
  ok "empty corpus prints unknown (never invented)"
else
  bad "empty corpus did not print unknown"
fi
# ---------------------------------------------------------------------------
# Case 5: negative wait (clock skew) is unparseable accounting, not a sample
# ---------------------------------------------------------------------------
D5="$(mk_fixture case5)"
jq -n --arg t0 "$T0" '[
  {databaseId: 9, createdAt: "2026-10-01T10:05:00Z", startedAt: "2026-10-01T10:04:00Z", event: "push", conclusion: "success"}
]' > "$D5/runs.json"
OUT5="$(run_metrics "$D5")"
if echo "$OUT5" | grep -q 'unparseable run rows 1'; then
  ok "negative wait (skewed clock) counted as unparseable"
else
  bad "negative wait leaked into samples"
fi
# ---------------------------------------------------------------------------
# Case 6: merge-queue lane exact math (#2284)
#   merge_group runs: two workflows for PR 100, both wait 30s in dispatch
#     -> dispatch p50=p95=30s (0.5 min), n=2 (both runs are samples even
#        though they belong to one PR)
#   timeline-100: added_to_merge_queue @10:00:00, merged @10:30:00
#     -> residence 1800s (30.0 min), n=1
# ---------------------------------------------------------------------------
D6="$(mk_fixture case6)"
jq -n --arg t0 "$T0" '[
  {databaseId: 21, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:00:30")), event: "merge_group", conclusion: "success", headBranch: "gh-readonly-queue/main/pr-100-abc123"},
  {databaseId: 22, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:00:30")), event: "merge_group", conclusion: "success", headBranch: "gh-readonly-queue/main/pr-100-abc123"}
]' > "$D6/merge_runs.json"
jq -n '[
  {event: "labeled", created_at: "2026-10-01T09:00:00Z"},
  {event: "added_to_merge_queue", created_at: "2026-10-01T10:00:00Z"},
  {event: "merged", created_at: "2026-10-01T10:30:00Z"},
  {event: "removed_from_merge_queue", created_at: "2026-10-01T10:30:00Z"}
]' > "$D6/timeline-100.json"
OUT6="$(run_metrics "$D6" --queue-prs 50)"
if echo "$OUT6" | grep -q '| 0.5 min | 0.5 min | 2 |'; then
  ok "merge-queue dispatch wait exact (0.5/0.5 over 2 runs)"
else
  bad "merge-queue dispatch wait wrong"
fi
if echo "$OUT6" | grep -q '| 30.0 min | 30.0 min | 1 |'; then
  ok "merge-queue residence exact (30.0 min from the PR timeline)"
else
  bad "merge-queue residence wrong"
fi
if echo "$OUT6" | grep -q 'PR timelines fetched 1/50'; then
  ok "PR timeline fetch counted (1/50)"
else
  bad "PR timeline fetch accounting wrong"
fi
# ---------------------------------------------------------------------------
# Case 7: residence accounting is fail-closed, never silently dropped
#   PR 200: merged WITHOUT an enqueue event  -> no-enqueue accounting
#   PR 201: enqueued but never merged        -> not-merged accounting
#   PR 202: no timeline fixture              -> timeline failure accounting
#   -> residence has ZERO samples (unknown), dispatch still exact (3 runs)
# ---------------------------------------------------------------------------
D7="$(mk_fixture case7)"
jq -n --arg t0 "$T0" '[
  {databaseId: 31, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:01:00")), event: "merge_group", conclusion: "success", headBranch: "gh-readonly-queue/main/pr-200-def456"},
  {databaseId: 32, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:00:10")), event: "merge_group", conclusion: "success", headBranch: "gh-readonly-queue/main/pr-201-789abc"},
  {databaseId: 33, createdAt: $t0, startedAt: ($t0 | sub("10:00:00";"10:02:00")), event: "merge_group", conclusion: "success", headBranch: "gh-readonly-queue/main/pr-202-012def"}
]' > "$D7/merge_runs.json"
jq -n '[
  {event: "merged", created_at: "2026-10-01T11:00:00Z"}
]' > "$D7/timeline-200.json"
jq -n '[
  {event: "added_to_merge_queue", created_at: "2026-10-01T10:00:00Z"}
]' > "$D7/timeline-201.json"
OUT7="$(run_metrics "$D7" --queue-prs 50)"
if echo "$OUT7" | grep -q '| Merge-queue residence per PR (added_to_merge_queue → merged) | unknown min | unknown min | 0 |'; then
  ok "residence with zero valid samples prints unknown"
else
  bad "residence unknown row wrong"
fi
if echo "$OUT7" | grep -q 'PRs without an enqueue event 1'; then
  ok "merged-without-enqueue PR accounted"
else
  bad "no-enqueue accounting wrong"
fi
if echo "$OUT7" | grep -q 'PRs not merged 1'; then
  ok "enqueued-but-not-merged PR accounted"
else
  bad "not-merged accounting wrong"
fi
if echo "$OUT7" | grep -q 'timeline failures 1'; then
  ok "timeline API failure accounted"
else
  bad "timeline failure accounting wrong"
fi
if echo "$OUT7" | grep -q '| 1.0 min | 2.0 min | 3 |'; then
  ok "dispatch wait unaffected by residence accounting (1.0/2.0 over 3)"
else
  bad "dispatch wait percentiles wrong"
fi
echo
if [[ "$fail" -eq 0 ]]; then
  echo "All ci_queue_metrics checks passed ($PASSED assertions)."
  exit 0
else
  echo "ci_queue_metrics: $PASSED passed, at least one assertion FAILED."
  exit 1
fi
