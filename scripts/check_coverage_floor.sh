#!/usr/bin/env bash
#
# Coverage floor — the coverage gate must be a NUMBER, not a comparison.
# Codecov's `target: auto` compares against the previous upload and the job that
# produces coverage is documented as non-gating, so nothing can fail on a drop.
#
# Usage: check_coverage_floor.sh [LCOV_FILE]
#
#   LCOV_FILE        first positional argument, else $LCOV_FILE, else lcov.info.
#   COVERAGE_FLOOR   integer percent; the environment overrides the committed
#                    value below and goes through the SAME validation.
#
# Exit 0 = measured line coverage >= floor. Exit 1 = below the floor (the message
# prints the measured value, its provenance and the floor). Exit 2 = cannot tell:
# the floor is not calibrated, the input is missing/unreadable, or its LF/LH
# records are absent or malformed — never a silent pass.
#
# HOW THE MEASUREMENT IS READ
#   SUM(LH) / SUM(LF) over every record set, in integer basis points (truncated),
#   so the comparison never depends on floating point. `--fail-under-lines N` on
#   `cargo llvm-cov` stays the CI enforcement point; this guard makes the number
#   visible and independently checkable from the produced lcov.
set -euo pipefail

# THE FLOOR — one definition. A MEASURED integer percent; never 0, and never a
# number someone lowered by hand to get a PR through.
#
# Measured at commit 99471ded on 2026-11-27T22:04:09Z with the tooling CI pins
# (cargo-llvm-cov 0.9.1, cargo-nextest 0.9.131, rustc 1.100.0), using the same
# command the Coverage job runs:
#
#   cargo llvm-cov nextest --locked --all-features --workspace --lcov \
#     --output-path <tmp>.lcov --profile coverage -E 'not test(test_mcp)'
#
# LH=60_918 / LF=70_252 = 86.7135% line coverage. The floor is that figure
# ROUNDED DOWN, and the rounding IS its margin: 0.71 pp, about 500 lines. That
# absorbs ordinary churn without hiding a real regression, while two slices that
# add code without tests still reach it. A tighter margin would fail sooner for
# noise; a looser one (85 = ~1200 lines) would take several slices to bite, and
# the next person would lower it by hand instead of investigating.
#
# It only ratchets UP. Raising it is a deliberate, measured edit; lowering it is
# a regression, not a fix. The sentinel path below is kept on purpose: if the
# number is ever removed, the guard REFUSES to run instead of comparing against
# a default that passes everything.
COVERAGE_FLOOR="${COVERAGE_FLOOR-86}"  # `-` not `:-`: unset takes the measured default, explicit-empty refuses

LCOV_FILE="${1:-${LCOV_FILE:-lcov.info}}"

fail_cannot_tell() { # message
  echo "::error::Coverage floor: $1" >&2
  exit 2
}

case "$COVERAGE_FLOOR" in
  '' | UNSET | unset)
    fail_cannot_tell "floor not calibrated yet: COVERAGE_FLOOR must be a measured integer percent (set COVERAGE_FLOOR at the top of $0), refusing to declare a coverage verdict against an unmeasured floor"
    ;;
esac
[[ "$COVERAGE_FLOOR" =~ ^[0-9]{1,3}$ ]] ||
  fail_cannot_tell "floor not calibrated yet: COVERAGE_FLOOR='$COVERAGE_FLOOR' is not a measured integer percent"
((COVERAGE_FLOOR <= 100)) ||
  fail_cannot_tell "COVERAGE_FLOOR='$COVERAGE_FLOOR' is not a percent (0-100)"

[[ -e "$LCOV_FILE" ]] || fail_cannot_tell "cannot tell — lcov input '$LCOV_FILE' does not exist"
[[ -r "$LCOV_FILE" ]] || fail_cannot_tell "cannot tell — lcov input '$LCOV_FILE' is not readable"

# A record whose value is not `[0-9]+` is counted as bad, never coerced to 0 — a
# silent 0 would turn a schema change into a pass.
parsed="$(awk '
  /^LF:/ {
    if ($0 ~ /^LF:[0-9]+[[:space:]]*$/) { lf += substr($0, 4) + 0; lf_records++ } else { bad++ }
    next
  }
  /^LH:/ {
    if ($0 ~ /^LH:[0-9]+[[:space:]]*$/) { lh += substr($0, 4) + 0; lh_records++ } else { bad++ }
    next
  }
  END { printf "%d %d %d %d %d\n", lf + 0, lh + 0, lf_records + 0, lh_records + 0, bad + 0 }
' "$LCOV_FILE")" || fail_cannot_tell "cannot tell — '$LCOV_FILE' could not be read"

read -r lf lh lf_records lh_records bad_records <<<"$parsed"

((bad_records == 0)) ||
  fail_cannot_tell "cannot tell — '$LCOV_FILE' carries $bad_records malformed LF/LH record(s)"
((lf_records > 0 && lh_records > 0)) ||
  fail_cannot_tell "cannot tell — '$LCOV_FILE' carries no parseable LF/LH records"
((lf_records == lh_records)) ||
  fail_cannot_tell "cannot tell — '$LCOV_FILE' has $lf_records LF record(s) but $lh_records LH record(s)"
((lf > 0)) || fail_cannot_tell "cannot tell — '$LCOV_FILE' measures 0 lines"
((lh <= lf)) ||
  fail_cannot_tell "cannot tell — '$LCOV_FILE' reports more covered lines ($lh) than measured lines ($lf)"

# Basis points (hundredths of a percent), truncated: integer arithmetic only.
measured_bp=$((lh * 10000 / lf))
floor_bp=$((COVERAGE_FLOOR * 100))
measured="$(printf '%d.%02d%%' $((measured_bp / 100)) $((measured_bp % 100)))"
provenance="measured from '$LCOV_FILE' ($lh of $lf lines covered)"

if ((measured_bp < floor_bp)); then
  shortfall_bp=$((floor_bp - measured_bp))
  gap="$(printf '%d.%02d%%' $((shortfall_bp / 100)) $((shortfall_bp % 100)))"
  echo "::error::Coverage floor: line coverage $measured is BELOW the floor ${COVERAGE_FLOOR}% (short by $gap) — $provenance" >&2
  exit 1
fi

echo "Coverage floor: line coverage $measured >= floor ${COVERAGE_FLOOR}% — $provenance"
exit 0
