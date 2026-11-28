#!/usr/bin/env bash
#
# test_coverage_floor.sh — semantics harness for scripts/check_coverage_floor.sh
# (#3105, Guard 3).
#
# WHAT IT PINS. The decision table the guard's header documents — 0 above or
# exactly at the floor, 1 below it (naming the measured value, its provenance and
# the floor), 2 cannot tell — against tiny synthetic lcov files, never the real
# lcov.info, which would make the suite depend on a build.
#
# The FIRST row is what keeps a decorative gate out of the tree: while the
# committed floor is still the uncalibrated sentinel, the guard refuses to run.
#
# Usage: bash scripts/tests/test_check_coverage_floor.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="$ROOT/scripts/check_coverage_floor.sh"

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

if [ ! -r "$GUARD" ]; then
  echo "harness target not found: $GUARD" >&2
  exit 1
fi

FIXTURES="$(mktemp -d)"
trap 'rm -rf "$FIXTURES"' EXIT

# write_lcov <path> <lf> <lh> — one source file per record set, so the guard has
# to SUM the records instead of trusting the last one it saw.
write_lcov() {
  local path="$1" lf="$2" lh="$3"
  cat >"$path" <<EOF
TN:
SF:/src/first.rs
LF:$lf
LH:$lh
end_of_record
SF:/src/second.rs
LF:$lf
LH:$lh
end_of_record
EOF
}

OUT=""
RC=0
run_guard() { # args passed verbatim to the guard
  OUT="$(bash "$GUARD" "$@" 2>&1)"
  RC=$?
}
# run_guard_floor <floor> [args...]: the floor is exported to the CHILD only, so
# the ambient environment of whoever runs this suite can never set it.
run_guard_floor() {
  local floor="$1"
  shift
  OUT="$(COVERAGE_FLOOR="$floor" bash "$GUARD" "$@" 2>&1)"
  RC=$?
}

# fixtures: 180 lines total, so each row states a different verdict on the same
# denominator and the numbers are readable in the failure messages.
write_lcov "$FIXTURES/above.lcov" 90 81   # 180 lines, 162 covered = 90.00%
write_lcov "$FIXTURES/at.lcov" 90 80      # 180 lines, 160 covered = 88.88%
write_lcov "$FIXTURES/below.lcov" 90 79   # 180 lines, 158 covered = 87.77%

printf 'TN:\nSF:/src/broken.rs\nLF:not-a-number\nLH:3\nend_of_record\n' >"$FIXTURES/malformed.lcov"
printf 'this file is not lcov at all\n' >"$FIXTURES/empty.lcov"

echo "A: an uncalibrated floor refuses to run (never a pass on a number nobody measured)"
run_guard_floor "" "$FIXTURES/above.lcov"
expect_eq "empty floor override" 2 "$RC"
expect_contains "the refusal says it is not calibrated" "$OUT" "not calibrated"

run_guard_floor "UNSET" "$FIXTURES/above.lcov"
expect_eq "committed sentinel" 2 "$RC"
expect_contains "the sentinel refusal says it is not calibrated" "$OUT" "not calibrated"

run_guard_floor "not-a-percent" "$FIXTURES/above.lcov"
expect_eq "non-numeric floor" 2 "$RC"
expect_contains "a non-numeric floor says it is not calibrated" "$OUT" "not calibrated"

run_guard_floor 101 "$FIXTURES/above.lcov"
expect_eq "floor above 100" 2 "$RC"

echo "B: above the floor -> 0"
run_guard_floor 80 "$FIXTURES/above.lcov"
expect_eq "90.00% against a floor of 80" 0 "$RC"
expect_contains "the measured value is printed" "$OUT" "90.00%"
expect_contains "the floor is printed" "$OUT" "80%"

echo "C: exactly at the floor -> 0"
run_guard_floor 88 "$FIXTURES/at.lcov"
expect_eq "88.88% against a floor of 88" 0 "$RC"
expect_contains "the measured value is printed" "$OUT" "88.88%"

echo "D: one line below the floor -> 1, naming floor and measurement"
run_guard_floor 88 "$FIXTURES/below.lcov"
expect_eq "87.77% against a floor of 88" 1 "$RC"
expect_contains "the measured value is printed" "$OUT" "87.77%"
expect_contains "the floor is printed" "$OUT" "88%"
expect_contains "the failure is annotated for CI" "$OUT" "::error::"
expect_contains "the provenance names the input file" "$OUT" "below.lcov"

echo "E: missing input -> 2 (cannot tell, never a silent pass)"
run_guard "$FIXTURES/does-not-exist.lcov"
expect_eq "missing input" 2 "$RC"
expect_contains "missing input is annotated" "$OUT" "::error::"

echo "F: unparsable input -> 2 (cannot tell, never a silent pass)"
run_guard "$FIXTURES/malformed.lcov"
expect_eq "malformed record" 2 "$RC"
expect_contains "malformed record is annotated" "$OUT" "::error::"

run_guard "$FIXTURES/empty.lcov"
expect_eq "no lcov record at all" 2 "$RC"
expect_contains "empty input is annotated" "$OUT" "::error::"

# summary
if [ "$fail" -ne 0 ]; then
  echo "test_coverage_floor: FAIL ($fail failed, $pass passed)"
  exit 1
fi
echo "test_coverage_floor: PASS ($pass assertions)"
exit 0

# NEGATION: the guard replaced by `exit 0` must turn this harness red — A, D, E
# and F all expect a non-zero verdict the stub can never produce.
# NEGATION: a defaulted floor (0, or any number nobody measured) must turn it
# red — A asserts the refusal in all three uncalibrated shapes.
# NEGATION: accepting the input without summing every record must turn it red —
# each fixture carries two equal SF blocks, so reading only one would report a
# different percentage than D asserts.
# NEGATION: making a below-floor measurement pass must turn it red — D.
# NEGATION: turning a missing or unparsable input into a pass must turn it red
# — E and F (the fail-open shape this guard exists to close).
