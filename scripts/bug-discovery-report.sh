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
#   "   TRY 3 TMT [ 180.046s] (───) <binary> <test_path>"
# a crashed one as
#   "   TRY 1 ABRT [   0.094s] (───) <binary> <test_path>"  (or SEGV, KILL)
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
# THAT case is not a finding, it is the ABSENCE of evidence: the reporter
# exits 1, so the job cannot read as green.
#
# Usage: bug_discovery_report <hermetic|findings> <log-path>
# Writes the report to stdout; the caller redirects it to the job summary.
# Exit: 0 the run completed (with or without findings) · 1 the run did not
# complete, or the mode is unknown.

# The status line, as one ERE: an optional "TRY n " prefix, the status token,
# an optional "n/m" counter (FLKY-FL carries one), then the duration in
# brackets.
#
# The token is NOT enumerated on purpose. A hand-written list is a list that
# misses the next status nextest prints: the first version of this file knew
# "ABORT" and "FLKY", and nextest prints "ABRT" and "FLAKY" -- so a crashed
# test was invisible and the summary said "no findings" on a red run. What is
# enumerated is the set of tokens that are NOT a finding (below), and that
# list is short and stable: a pass is a pass, however slow.
BUG_DISCOVERY_STATUS_RE='^[[:space:]]*(TRY[[:space:]]+[0-9]+[[:space:]]+)?[A-Z][A-Z0-9-]*[[:space:]]+([0-9]+/[0-9]+[[:space:]]+)?\['

# The statuses that are NOT a finding: the test passed (maybe slowly).
BUG_DISCOVERY_PASS_RE='^[[:space:]]*(TRY[[:space:]]+[0-9]+[[:space:]]+)?(PASS|SLOW|SKIP|IGN)[[:space:]]'

# The same shape, capturing the status token and the test path: after the
# duration there is an optional attempt counter ("(1/2)") and then the path.
BUG_DISCOVERY_PATH_SED='s#^[[:space:]]*(TRY[[:space:]]+[0-9]+[[:space:]]+)?([A-Z][A-Z0-9-]*)[[:space:]]+([0-9]+/[0-9]+[[:space:]]+)?\[[^]]*\][[:space:]]+(\([^)]*\)[[:space:]]+)?(.*)$#\2 \5#p'

# The summary line nextest prints when the run completed.
BUG_DISCOVERY_SUMMARY_RE='Summary[[:space:]]+\['

# bug_discovery_report <hermetic|findings> <log-path>
#
# hermetic: the poisoned-env job ("do the tests survive a poisoned env?").
# findings: the ignored-tests job ("did the ignored suite find a bug?").
bug_discovery_report() {
  local mode="${1:-}" log="${2:-}"
  local header fail_header ok clean findings complete=1

  case "$mode" in
    hermetic)
      header="## Poisoned Env Results"
      fail_header="Tests that FAIL, TIME OUT, CRASH or flake with poisoned env are NOT hermetic:"
      ok="All tests hermetic under poisoned environment."
      ;;
    findings)
      header="## Production Bug Discovery"
      fail_header="Findings here = potential production bugs (failed, timed out, crashed or flaky):"
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

  # A run that never produced a summary did not complete: nextest aborted, or
  # the log is empty. That is the ABSENCE of evidence, never "no findings".
  grep -qE "$BUG_DISCOVERY_SUMMARY_RE" <<<"$clean" || complete=0

  findings="$(grep -E "$BUG_DISCOVERY_STATUS_RE" <<<"$clean" \
    | grep -vE "$BUG_DISCOVERY_PASS_RE" || true)"

  printf '%s\n\n' "$header"
  if [ "$complete" -eq 0 ]; then
    printf 'nextest never produced a summary: the run did not complete (read the job log).\n\n'
  fi
  if [ -n "$findings" ]; then
    printf '%s\n\n' "$fail_header"
    # One line per TEST, not per attempt: nextest prints a status line per
    # attempt AND repeats the last one in its summary block, so a test that
    # failed three times was listed FOUR times ("4 findings" for one test --
    # the same kind of lie this reporter exists to stop). The LAST status of a
    # path is its verdict: the enforced-flaky one ("FLKY-FL") when a retry
    # saved it, the failure when it did not.
    printf '%s\n' "$findings" \
      | sed -nE "$BUG_DISCOVERY_PATH_SED" \
      | awk '{
          key = $0
          sub(/^[^ ]+ /, "", key)
          last[key] = $0
          if (!(key in seen)) { seen[key] = 1; order[++n] = key }
        }
        END { for (i = 1; i <= n; i++) print "- " last[order[i]] }' || true
  elif [ "$complete" -eq 1 ]; then
    printf '%s\n' "$ok"
  fi

  [ "$complete" -eq 1 ]
}
