#!/usr/bin/env bash
set -euo pipefail
# test_ci_gate_structure.sh — structural CI Gate harness (#2286).
#
# Parses .github/workflows/ci.yml as text (the same shape actionlint already
# validates) and pins the structural contract of the CI Gate:
#
#   1. gate.needs is exactly {change-scope} ∪ FULL_PRODUCERS, where
#      FULL_PRODUCERS/NARROW_PRODUCERS are read from scripts/ci_plan.sh — the
#      planner is the single source of truth, so a producer renamed or added
#      there without gate wiring fails here.
#   2. NARROW ⊆ FULL and every producer entry is a declared job id.
#   3. Every declared job is either in gate.needs or on an explicit advisory
#      allowlist — a new non-advisory job without gate wiring fails.
#   4. No gate producer carries `continue-on-error`.
#   5. The gate's verify step consumes the same partition it gates: its
#      narrow-scope success list ∪ skip list ∪ {change-scope} equals
#      gate.needs, and it fails when change-scope itself failed (a failed or
#      empty plan can never green the gate).
#   6. The fast gate's coverage table (scripts/ci_fast_gate_coverage.tsv)
#      describes EXACTLY gate.needs: no producer missing, no stale row, and no
#      lane token outside the set the gate's dispatcher actually assigns
#      (#2559). The local gate reports its own gaps from that table, so a table
#      that under-describes gate.needs would make the report lie in the same
#      silence it exists to break.
#
#      The lane vocabulary is DERIVED from the dispatcher, never restated: a
#      hand-maintained copy of "the lanes" is the same drift class as a
#      hand-maintained producer list, one level down.
#
# What this harness does NOT prove: that a row's lane column names the lane that
# really runs that job's step. Step names are free text, so there is no static
# mapping to check — this harness pins the job set and the lane vocabulary, and
# nothing else. The report's LOGIC is pinned behaviourally in
# scripts/test_ci_fast_gate.sh, but against its OWN fixture table (one assertion
# per claim: a spaced lane list, `security` partial, `test-core` partial in the
# code lane and covered in the full lane, notes on every line). A wrong lane
# column in the REAL table would therefore pass both harnesses. That residual
# gap is accepted knowingly: the table is 13 hand-written rows that every
# developer reads on every run, so a wrong column is visible, not buried.
#
# Hermetic: no cargo, no network, no fixture repos — it reads the three files
# and nothing else.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CI_YML="${CI_GATE_STRUCTURE_YML:-$SCRIPT_DIR/../.github/workflows/ci.yml}"
PLAN="${CI_PLAN:-$SCRIPT_DIR/ci_plan.sh}"
COVERAGE="${CI_FAST_GATE_COVERAGE:-$SCRIPT_DIR/ci_fast_gate_coverage.tsv}"
FAST_GATE="${CI_FAST_GATE:-$SCRIPT_DIR/ci_fast_gate.sh}"

fail=0
PASSED=0
ok() {
  PASSED=$((PASSED + 1))
  echo "OK: $1"
}
bad() {
  echo "FAIL: $1"
  fail=1
}

[ -f "$CI_YML" ] || { echo "FAIL: ci.yml not found at $CI_YML"; exit 1; }
[ -f "$PLAN" ] || { echo "FAIL: plan script not found at $PLAN"; exit 1; }
[ -f "$COVERAGE" ] || { echo "FAIL: fast-gate coverage table not found at $COVERAGE"; exit 1; }
[ -f "$FAST_GATE" ] || { echo "FAIL: fast gate not found at $FAST_GATE"; exit 1; }

# --- Planner constants (single source of truth) ------------------------------
FULL_PRODUCERS="$(sed -n 's/^FULL_PRODUCERS="\(.*\)"$/\1/p' "$PLAN")"
NARROW_PRODUCERS="$(sed -n 's/^NARROW_PRODUCERS="\(.*\)"$/\1/p' "$PLAN")"

if [[ -n "$FULL_PRODUCERS" ]]; then
  ok "FULL_PRODUCERS is non-empty ($FULL_PRODUCERS)"
else
  bad "FULL_PRODUCERS is empty or missing in ci_plan.sh"
fi
if [[ -n "$NARROW_PRODUCERS" ]]; then
  ok "NARROW_PRODUCERS is non-empty ($NARROW_PRODUCERS)"
else
  bad "NARROW_PRODUCERS is empty or missing in ci_plan.sh"
fi

to_set() { tr ',' '\n' <<<"$1" | tr ' ' '\n' | sed 's/^ *//;s/ *$//' | grep -v '^$' | sort | uniq; }
FULL_SET="$(to_set "$FULL_PRODUCERS")"
NARROW_SET="$(to_set "$NARROW_PRODUCERS")"

# --- Parse ci.yml: declared jobs, gate block, gate verify lists --------------
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

# Declared top-level job ids: two-space indented `id:` lines AFTER the
# `jobs:` key (the `on:` trigger block also has two-space `key:` lines, so
# scanning must start at jobs:).
awk '
  /^jobs:/ { in_jobs = 1; next }
  in_jobs && /^[a-zA-Z_]/ { in_jobs = 0 }
  in_jobs && /^  [A-Za-z0-9_-]+:$/ {
    id = $0; gsub(/^ +|:$/, "", id); print id
  }
' "$CI_YML" > "$T/jobs.txt"

# Gate block: from `  gate:` up to the next top-level job (or EOF).
awk '
  /^  [A-Za-z0-9_-]+:$/ {
    in_gate = ($0 == "  gate:")
  }
  in_gate { print }
' "$CI_YML" > "$T/gate_block.txt"

# needs: [...] inside the gate block (the required-producers list).
GATE_NEEDS_RAW="$(sed -n 's/^    needs: \[\(.*\)\]$/\1/p' "$T/gate_block.txt" | head -1)"
if [[ -n "$GATE_NEEDS_RAW" ]]; then
  ok "gate.needs parsed ($GATE_NEEDS_RAW)"
else
  bad "could not parse gate.needs from ci.yml"
fi
GATE_SET="$(to_set "$GATE_NEEDS_RAW")"

# continue-on-error inside each gate producer's block.
awk '
  /^  [A-Za-z0-9_-]+:$/ { job = $0; gsub(/^ +|:$/, "", job); in_job = 1 }
  in_job && /^[[:space:]]*continue-on-error:/ { print job ": " $0 }
' "$CI_YML" > "$T/coe.txt"

# Gate verify step lists: the two `for job in ...` loops of the narrow branch.
SUCCESS_LIST="$(sed -n 's/^            for job in \(.*\); do$/\1/p' "$T/gate_block.txt" | head -1)"
SKIP_LIST="$(sed -n 's/^            for job in \(.*\); do$/\1/p' "$T/gate_block.txt" | tail -1)"
if [[ -n "$SUCCESS_LIST" && -n "$SKIP_LIST" ]]; then
  ok "gate verify lists parsed"
else
  bad "could not parse the gate verify step's narrow-scope lists"
fi
VERIFY_SET="$(to_set "$SUCCESS_LIST $SKIP_LIST change-scope")"

# --- 1. gate.needs == {change-scope} ∪ FULL ---------------------------------
EXPECTED_GATE="$( { echo change-scope; to_set "$FULL_PRODUCERS"; } | sort -u )"
if [[ "$GATE_SET" == "$EXPECTED_GATE" ]]; then
  ok "gate.needs == {change-scope} ∪ FULL_PRODUCERS"
else
  bad "gate.needs drifted from the planner: extra=[$(comm -13 <(echo "$EXPECTED_GATE") <(echo "$GATE_SET") | tr '\n' ' ')] missing=[$(comm -23 <(echo "$EXPECTED_GATE") <(echo "$GATE_SET") | tr '\n' ' ')]"
fi

# --- 2. NARROW ⊆ FULL, every producer is a declared job ----------------------
NARROW_EXTRA="$(comm -23 <(echo "$NARROW_SET") <(echo "$FULL_SET"))"
if [[ -z "$NARROW_EXTRA" ]]; then
  ok "NARROW ⊆ FULL"
else
  bad "NARROW has entries outside FULL: $(echo "$NARROW_EXTRA" | tr '\n' ' ')"
fi

PRODUCER_UNKNOWN="$(comm -23 <(echo "$FULL_SET") <(sort -u "$T/jobs.txt"))"
if [[ -z "$PRODUCER_UNKNOWN" ]]; then
  ok "every FULL producer is a declared ci.yml job"
else
  bad "planner names jobs that do not exist in ci.yml: $(echo "$PRODUCER_UNKNOWN" | tr '\n' ' ')"
fi

# --- 3. every declared job is gated or explicitly advisory -------------------
# Advisory allowlist (path:line-free, by job id). A job lands here ONLY with
# a written reason; adding a new non-advisory job without gate wiring fails
# assertion 1 of this harness.
ALLOWLIST="gate
test-crossplatform|planner-gated advisory lane (#1608), promoted to required is a follow-up
tokio-console-smoke|main/nightly advisory smoke
notify|scheduled-run reporting, not a producer
feature-matrix|advisory compile matrix
mcp|planner-gated advisory handshake
release|push-to-main artifact build
miri|advisory FFI lane (continue-on-error, reported by notify)
miri-infra-fast|advisory FFI lane
miri-infra-slow|advisory FFI lane
coverage|report-only upload, never gates
flake-detection|nightly retries-0 flake report, job-level continue-on-error (#2286)
cfg-windows-lint|planner-gated cfg(windows) cross-lint (#2658 class found by #2732/#2733); deterministic lint, promoted to required after 20 consecutive green runs"

while IFS= read -r job; do
  [[ -n "$job" ]] || continue
  if grep -qx "$job" <<<"$GATE_SET"; then
    :
  elif cut -d'|' -f1 <<<"$ALLOWLIST" | grep -qx "$job"; then
    :
  else
    bad "job '$job' is neither in gate.needs nor on the advisory allowlist"
  fi
done < "$T/jobs.txt"
ok "every declared job is gated or on the advisory allowlist"

# --- 4. no continue-on-error on gate producers -------------------------------
COE_IN_GATE="$(while IFS= read -r line; do
  j="${line%%: *}"
  if grep -qx "$j" <<<"$GATE_SET"; then
    echo "$line"
  fi
done < "$T/coe.txt")"
if [[ -z "$COE_IN_GATE" ]]; then
  ok "no gate producer carries continue-on-error"
else
  bad "continue-on-error inside a gate producer: $(echo "$COE_IN_GATE" | tr '\n' ';')"
fi

# --- 5. the gate verify step consumes the same partition --------------------
if [[ "$VERIFY_SET" == "$GATE_SET" ]]; then
  ok "gate verify narrow lists + change-scope == gate.needs"
else
  bad "gate verify step drifted from gate.needs: extra=[$(comm -13 <(echo "$GATE_SET") <(echo "$VERIFY_SET") | tr '\n' ' ')] missing=[$(comm -23 <(echo "$GATE_SET") <(echo "$VERIFY_SET") | tr '\n' ' ')]"
fi

if grep -q '"change-scope".result' "$T/gate_block.txt"; then
  ok "gate verify fails when change-scope (the plan) failed"
else
  bad "gate verify does not check change-scope's result — a failed/empty plan could green the gate"
fi

# --- 6. the fast gate's coverage table describes EXACTLY gate.needs ----------
# `scripts/ci_fast_gate.sh` prints, at the end of every run, the `gate.needs`
# jobs its lane did not verify (#2559). That report is only worth reading if
# it is complete: a producer added to `gate.needs` and never declared here
# would be missing from the report — the very silence the report exists to
# break. So the two sets must match in BOTH directions, and every lane token
# must name a lane the gate actually dispatches.
COVERAGE_JOBS="$(awk -F'\t' '$1 !~ /^#/ && $1 != "" { print $1 }' "$COVERAGE" | sort -u)"
COVERAGE_LANES="$(awk -F'\t' '$1 !~ /^#/ && $1 != "" { print $2; print $3 }' "$COVERAGE" \
  | tr ',' '\n' | sed 's/^ *//;s/ *$//' | grep -v '^$' | grep -vx -- '-' | sort -u)"

# The lane vocabulary, read from the dispatcher's own assignments. The gate sets
# FAST_GATE_LANES in exactly the branches that reach the coverage report, so the
# set of values assigned there IS the set of lanes a row may name. Restating it
# by hand here would recreate, one level down, the drift this harness exists to
# catch.
GATE_LANES="$(sed -n 's/^ *FAST_GATE_LANES="\(.*\)"$/\1/p' "$FAST_GATE" \
  | tr ' ' '\n' | sed 's/^ *//;s/ *$//' | grep -v '^$' | sort -u)"

if [[ -n "$GATE_LANES" ]]; then
  ok "the fast gate's lanes were derived from its dispatcher ($(echo "$GATE_LANES" | tr '\n' ' '))"
else
  bad "could not derive any lane from $FAST_GATE (no FAST_GATE_LANES assignment found)"
fi

if [[ -n "$COVERAGE_JOBS" ]]; then
  ok "fast-gate coverage table parsed ($(wc -l <<<"$COVERAGE_JOBS" | tr -d ' ') jobs)"
else
  bad "could not parse any job from $COVERAGE"
fi

COVERAGE_MISSING="$(comm -23 <(echo "$GATE_SET") <(echo "$COVERAGE_JOBS"))"
if [[ -z "$COVERAGE_MISSING" ]]; then
  ok "every gate.needs job is declared in the fast-gate coverage table"
else
  bad "gate.needs jobs absent from the fast-gate coverage table: $(echo "$COVERAGE_MISSING" | tr '\n' ' ')"
fi

COVERAGE_EXTRA="$(comm -13 <(echo "$GATE_SET") <(echo "$COVERAGE_JOBS"))"
if [[ -z "$COVERAGE_EXTRA" ]]; then
  ok "the fast-gate coverage table declares no job outside gate.needs"
else
  bad "the fast-gate coverage table names jobs that are not gate.needs: $(echo "$COVERAGE_EXTRA" | tr '\n' ' ')"
fi

COVERAGE_BAD_LANE="$(comm -23 <(echo "$COVERAGE_LANES") <(echo "$GATE_LANES"))"
if [[ -z "$COVERAGE_BAD_LANE" ]]; then
  ok "every lane named in the coverage table is a lane the dispatcher assigns ($(echo "$GATE_LANES" | tr '\n' ' '))"
else
  bad "lane(s) in the coverage table that the dispatcher never assigns: $(echo "$COVERAGE_BAD_LANE" | tr '\n' ' ')"
fi

echo
if [[ "$fail" -ne 0 ]]; then
  echo "ci_gate_structure: $PASSED passed, at least one assertion FAILED."
  exit 1
fi

# --- Self-test: every guard must be able to FAIL -----------------------------
# A structural guard that cannot red is indistinguishable from no guard at
# all. Each case mutates a THROWAWAY copy (never the repository) and asserts
# the harness exits non-zero.
expect_fail() {
  local case_name="$1"
  if bash "$0" > /dev/null 2>&1; then
    bad "self-test '$case_name': the harness stayed GREEN on a broken input"
  else
    ok "self-test '$case_name': the harness fails closed"
  fi
}

SELF="$T/self"
mkdir -p "$SELF"

# Case 1: a producer removed from gate.needs.
sed 's/, test-release-provenance//' "$CI_YML" > "$SELF/ci.yml"
export CI_GATE_STRUCTURE_YML="$SELF/ci.yml" CI_PLAN="$PLAN"
expect_fail "producer removed from gate.needs"

# Case 2: a new non-advisory job with no gate wiring.
{ cat "$CI_YML"; printf '\n  new-ungated-job:\n    name: New ungated job\n    runs-on: ubuntu-latest\n'; } > "$SELF/ci.yml"
expect_fail "ungated new job"

# Case 3: continue-on-error inside a gate producer.
awk '
  /^  fmt:/ { in_fmt = 1 }
  in_fmt && /^    runs-on:/ { print; print "    continue-on-error: true"; in_fmt = 0; next }
  { print }
' "$CI_YML" > "$SELF/ci.yml"
expect_fail "continue-on-error on a gate producer"

# Case 4: the planner emits an empty producer list.
sed 's/^FULL_PRODUCERS=.*/FULL_PRODUCERS=""/' "$PLAN" > "$SELF/ci_plan.sh"
export CI_GATE_STRUCTURE_YML="$CI_YML" CI_PLAN="$SELF/ci_plan.sh"
expect_fail "empty FULL_PRODUCERS in the planner"

# Case 5: a producer missing from the fast gate's coverage table — the exact
# silence the report exists to break.
export CI_PLAN="$PLAN"
awk -F'\t' '$1 != "fmt"' "$COVERAGE" > "$SELF/coverage-missing.tsv"
export CI_FAST_GATE_COVERAGE="$SELF/coverage-missing.tsv"
expect_fail "gate.needs job missing from the coverage table"

# Case 6: the coverage table names a job that is not gate.needs — a stale row
# from a retired producer.
{ cat "$COVERAGE"; printf 'retired-job\tcode\t-\tno longer a producer\t-\n'; } > "$SELF/coverage-extra.tsv"
export CI_FAST_GATE_COVERAGE="$SELF/coverage-extra.tsv"
expect_fail "coverage table row outside gate.needs"

# Case 7: the table names a lane the dispatcher no longer assigns. The lane
# vocabulary is derived, so retiring a lane must red this harness rather than
# leaving a row that certifies coverage nobody provides.
export CI_FAST_GATE_COVERAGE="$COVERAGE"
sed 's/^  FAST_GATE_LANES="code"$/  FAST_GATE_LANES=""/' "$FAST_GATE" > "$SELF/ci_fast_gate.sh"
export CI_FAST_GATE="$SELF/ci_fast_gate.sh"
expect_fail "coverage table lane the dispatcher no longer assigns"

unset CI_GATE_STRUCTURE_YML CI_PLAN CI_FAST_GATE_COVERAGE CI_FAST_GATE

echo "All ci_gate_structure checks passed ($PASSED assertions, 7 self-tests)."
exit 0
