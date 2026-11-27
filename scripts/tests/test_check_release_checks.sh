#!/usr/bin/env bash
#
# test_release_checks.sh — semantics harness for scripts/check_release_checks.sh
# (#3105, Guard 1).
#
# WHAT IT PINS. The release pipeline must consult the CI result of the commit it
# is about to publish, not only its own provenance. The API read itself cannot
# run here — no network, no token — so the harness drives the fixture mode the
# guard documents in its header and asserts the DECISION TABLE:
#
#   0  every required context is present and its conclusion is `success`
#   1  at least one required context is missing, failed or still running
#      (decided red) — and the ::error:: line NAMES every offending context
#   2  cannot tell (the answer is unavailable or unparsable) — never a silent pass
#
# The extra failing context in the green fixture proves the decision is scoped
# to REQUIRED_CONTEXTS: a context nobody requires must not block a release.
#
# WHY THE FIXTURES AND NOT THE WORKFLOW. The wiring assertion (release.yml still
# invokes this guard from `preflight`, before `build`) belongs to the commit that
# wires the step — asserting it here, before the step exists, would be a harness
# that is red on purpose. See NEGATION below for what the case table is proven
# sensitive to today.
#
# Exit code: 0 when every assertion holds, 1 otherwise.
#
# Usage: bash scripts/tests/test_release_checks.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="$ROOT/scripts/check_release_checks.sh"

pass=0
fail=0
ok() { echo "  ok   $*"; pass=$((pass + 1)); }
bad() {
  echo "  FAIL $*" >&2
  fail=$((fail + 1))
}

expect_eq() { # label expected actual
  if [ "$2" = "$3" ]; then ok "$1 (exit $3)"; else bad "$1: expected exit '$2', got '$3'"; fi
}
expect_contains() { # label haystack needle
  case "$2" in
  *"$3"*) ok "$1 contains '$3'" ;;
  *)
    bad "$1: '$3' not found in output"
    printf '%s\n' "$2" >&2
    ;;
  esac
}
expect_not_contains() { # label haystack needle
  case "$2" in
  *"$3"*) bad "$1: '$3' unexpectedly present" ;;
  *) ok "$1 without '$3'" ;;
  esac
}

if [ ! -r "$GUARD" ]; then
  echo "harness target not found: $GUARD" >&2
  exit 1
fi

FIXTURES="$(mktemp -d)"
trap 'rm -rf "$FIXTURES"' EXIT

# run_case <fixture|-> : drive the guard's documented fixture mode.
# `-` passes no fixture at all, which is the cannot-tell shape where the answer
# file does not exist.
OUT=""
RC=0
run_case() {
  local fixture="$1"
  local -a cmd=(bash "$GUARD" checks)
  if [ "$fixture" != "-" ]; then
    OUT="$(RELEASE_CHECKS_FIXTURE="$fixture" GITHUB_SHA=0000000000000000000000000000000000000000 "${cmd[@]}" 2>&1)"
  else
    OUT="$(RELEASE_CHECKS_FIXTURE="$FIXTURES/does-not-exist.json" GITHUB_SHA=0000000000000000000000000000000000000000 "${cmd[@]}" 2>&1)"
  fi
  RC=$?
}

# --- fixtures ---------------------------------------------------------------
# One JSON document per decision-table row, in the shape of the check-runs API
# answer the guard normalizes (`check_runs[]` with name/status/conclusion).
cat >"$FIXTURES/green.json" <<'JSON'
{
  "total_count": 5,
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "success"},
    {"name": "cargo-mutants (PR diff)", "status": "completed", "conclusion": "success"},
    {"name": "Validate PR metadata", "status": "completed", "conclusion": "success"},
    {"name": "breaking-surface", "status": "completed", "conclusion": "success"},
    {"name": "an unrelated advisory job", "status": "completed", "conclusion": "failure"}
  ]
}
JSON

cat >"$FIXTURES/failed.json" <<'JSON'
{
  "total_count": 4,
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "failure"},
    {"name": "cargo-mutants (PR diff)", "status": "completed", "conclusion": "success"},
    {"name": "Validate PR metadata", "status": "completed", "conclusion": "success"},
    {"name": "breaking-surface", "status": "completed", "conclusion": "success"}
  ]
}
JSON

cat >"$FIXTURES/missing.json" <<'JSON'
{
  "total_count": 3,
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "success"},
    {"name": "cargo-mutants (PR diff)", "status": "completed", "conclusion": "success"},
    {"name": "Validate PR metadata", "status": "completed", "conclusion": "success"}
  ]
}
JSON

cat >"$FIXTURES/running.json" <<'JSON'
{
  "total_count": 4,
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "success"},
    {"name": "cargo-mutants (PR diff)", "status": "in_progress", "conclusion": null},
    {"name": "Validate PR metadata", "status": "completed", "conclusion": "success"},
    {"name": "breaking-surface", "status": "completed", "conclusion": "success"}
  ]
}
JSON

cat >"$FIXTURES/malformed.json" <<'JSON'
{
  "total_count": 1,
  "message": "Resource not accessible by integration"
}
JSON

cat >"$FIXTURES/two-offenders.json" <<'JSON'
{
  "total_count": 4,
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "cancelled"},
    {"name": "cargo-mutants (PR diff)", "status": "queued", "conclusion": null},
    {"name": "Validate PR metadata", "status": "completed", "conclusion": "success"},
    {"name": "breaking-surface", "status": "completed", "conclusion": "success"}
  ]
}
JSON

# --- decision table --------------------------------------------------------
echo "A: usage — no subcommand never passes silently"
OUT="$(bash "$GUARD" 2>&1)"
RC=$?
if [ "$RC" -ne 0 ]; then
  ok "no-argument invocation is non-zero (exit $RC)"
else
  bad "no-argument invocation exited 0 — a silent pass"
fi
expect_contains "usage line" "$OUT" "usage: check_release_checks.sh"
expect_contains "usage names the subcommand" "$OUT" "{checks}"

echo "B: all required contexts green -> 0"
run_case "$FIXTURES/green.json"
expect_eq "green fixture" 0 "$RC"
expect_not_contains "green fixture prints no error annotation" "$OUT" "::error::"

echo "C: one failed required context -> 1 and the message NAMES it"
run_case "$FIXTURES/failed.json"
expect_eq "failed fixture" 1 "$RC"
expect_contains "failed context is named" "$OUT" "CI Gate"
expect_contains "failure annotation" "$OUT" "::error::"

echo "D: one missing required context -> 1 and the message NAMES it"
run_case "$FIXTURES/missing.json"
expect_eq "missing fixture" 1 "$RC"
expect_contains "missing context is named" "$OUT" "breaking-surface"
expect_contains "missing is reported as missing" "$OUT" "missing"

echo "E: one still-running required context -> 1 and the message NAMES it"
run_case "$FIXTURES/running.json"
expect_eq "running fixture" 1 "$RC"
expect_contains "running context is named" "$OUT" "cargo-mutants (PR diff)"
expect_contains "observed status is reported" "$OUT" "in_progress"

echo "F: every offender is named in ONE blocking message"
run_case "$FIXTURES/two-offenders.json"
expect_eq "two-offender fixture" 1 "$RC"
expect_contains "first offender is named" "$OUT" "CI Gate"
expect_contains "second offender is named" "$OUT" "cargo-mutants (PR diff)"
expect_contains "second offender status is named" "$OUT" "queued"
OFFENDER_LINES="$(printf '%s\n' "$OUT" | grep -c '::error::' || true)"
if [ "${OFFENDER_LINES:-0}" -ge 1 ]; then
  ok "offenders are reported in one blocking error line ($OFFENDER_LINES)"
else
  bad "no ::error:: line was printed for a blocked release"
fi

echo "G: cannot tell -> 2 (never a silent pass)"
run_case -
expect_eq "unreadable answer" 2 "$RC"
expect_contains "cannot-tell annotation" "$OUT" "::error::"

run_case "$FIXTURES/malformed.json"
expect_eq "unparsable answer" 2 "$RC"
expect_contains "unparsable annotation" "$OUT" "::error::"

# --- summary ---------------------------------------------------------------
if [ "$fail" -ne 0 ]; then
  echo "test_release_checks: FAIL ($fail failed, $pass passed)"
  exit 1
fi
echo "test_release_checks: PASS ($pass assertions)"
exit 0

# NEGATION: the guard replaced by `exit 0` must turn this harness red (the case
# table would report a silent pass on every row).
# NEGATION: dropping any ONE context from REQUIRED_CONTEXTS must turn it red —
# each of the four is the sole offender of a row above (CI Gate in C, breaking-
# surface in D, cargo-mutants (PR diff) in E, Validate PR metadata in D's
# green companions), so a context removed from the asserted set stops being
# checked and the row that named it expects exit 1 while the guard returns 0.
# NEGATION: making a non-success conclusion pass must turn it red — C and F.
# NEGATION: turning `cannot tell` into a pass must turn it red — G.
