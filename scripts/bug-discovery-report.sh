#!/usr/bin/env bash
# Reporter for the Production Bug Discovery jobs (#2666).
#
# Both jobs report their findings with THIS code, because a copy per job is how
# they drifted apart in the first place: the copy grepped for a line that
# STARTS with "FAIL", and nextest 0.9.131 never prints one.
#
# nextest 0.9.131 renders a failed test as
#   "  TRY 3 FAIL [   0.482s] (1/2) <binary> <test_path>"
# a terminated one as
#   "   TRY 1 TMT [ 180.046s] (───) <binary> <test_path>"
# an enforced flaky (flaky-result = "fail") as
#   " FLKY-FL 2/3 [  79.107s] (5239/5241) <binary> <test_path>"
# and in CI the line is colorized (CARGO_TERM_COLOR=always), so the escape
# codes sit BETWEEN the status token and the duration. A matcher that expects
# "FAIL" at the start of the line, or that expects "FAIL" to be followed by
# the bracket, sees NOTHING: for 33 days the summary said "All tests hermetic"
# / "No findings: all ignored tests passed." on runs that were RED with real
# findings. The reporter strips the ANSI first and then matches the shapes.
#
# It also says so when nextest never produced a summary at all: it died
# before or during the run (#2629), and "no findings" was a lie then too.
#
# Usage: bug_discovery_report <hermetic|findings> <log-path>
# Writes the report to stdout; the caller redirects it to the job summary.

# The status line, as one ERE: an optional "TRY n " prefix, the status token,
# an optional "n/m" counter (FLKY-FL carries one), then the duration in
# brackets. The token is every status that is NOT a pass: a failure, a
# termination (the test did not finish: a timeout or a hang), a leak, an
# abort, or a flaky -- each one means the test did not survive the run.
BUG_DISCOVERY_STATUS_RE='^[[:space:]]*(TRY[[:space:]]+[0-9]+[[:space:]]+)?(FAIL|TMT|FLKY-FL|FLKY|LEAK|ABORT)([[:space:]]+[0-9]+/[0-9]+)?[[:space:]]+\['

# The same shape, capturing the status token and the test path: after the
# duration there is an optional attempt counter ("(1/2)") and then the path.
BUG_DISCOVERY_PATH_SED='s#^[[:space:]]*(TRY[[:space:]]+[0-9]+[[:space:]]+)?(FAIL|TMT|FLKY-FL|FLKY|LEAK|ABORT)([[:space:]]+[0-9]+/[0-9]+)?[[:space:]]+\[[^]]*\][[:space:]]+(\([^)]*\)[[:space:]]+)?(.*)$#\2 \5#p'

# The summary line nextest prints when the run completed.
BUG_DISCOVERY_SUMMARY_RE='Summary[[:space:]]+\['

# bug_discovery_report <hermetic|findings> <log-path>
#
# hermetic: the poisoned-env job ("are the tests hermetic under a poisoned
#           environment?").
# findings: the ignored-tests job ("did the ignored suite find a bug?").
bug_discovery_report() {
  local mode="${1:-}" log="${2:-}"
  local header fail_header ok clean

  case "$mode" in
    hermetic)
      header="## Poisoned Env Results"
      fail_header="Tests that FAIL, TIME OUT or flake with poisoned env are NOT hermetic:"
      ok="All tests hermetic under poisoned environment."
      ;;
    findings)
      header="## Production Bug Discovery"
      fail_header="Findings here = potential production bugs (failed, timed out or flaky):"
      ok="No findings: all ignored tests passed."
      ;;
    *)
      printf 'bug_discovery_report: unknown mode [%s]\n' "$mode" >&2
      return 2
      ;;
  esac

  # The ANSI goes first: in CI the status token and the duration are separated
  # by escape codes, so any matcher that sees the raw line sees nothing.
  clean="$(sed -E 's/\x1b\[[0-9;]*m//g' "$log" 2>/dev/null || true)"

  printf '%s\n\n' "$header"
  if grep -qE "$BUG_DISCOVERY_STATUS_RE" <<<"$clean"; then
    printf '%s\n\n' "$fail_header"
    grep -E "$BUG_DISCOVERY_STATUS_RE" <<<"$clean" \
      | sed -nE "$BUG_DISCOVERY_PATH_SED" \
      | sed -E 's/^/- /' \
      || true
  elif ! grep -qE "$BUG_DISCOVERY_SUMMARY_RE" <<<"$clean"; then
    printf 'nextest never produced a summary: the run did not complete (read the job log).\n'
  else
    printf '%s\n' "$ok"
  fi
}
