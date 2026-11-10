#!/usr/bin/env bash
#
# Semantics harness for the Production Bug Discovery reporter (#2666).
#
# The property here is behavioral and cannot be proven by grepping: a run that
# is RED must be REPORTED as red. The reporter used to grep the nextest log
# for a line starting with "FAIL", and nextest 0.9.131 prints none: a failed
# test is "  TRY 3 FAIL [   0.482s] (1/2) <binary> <test_path>" and an
# enforced flaky is " FLKY-FL 2/3 [  79.107s] (5239/5241) <test_path>", both
# colorized in CI (CARGO_TERM_COLOR=always) so the escape codes sit between
# the status token and the duration. The result was a red run whose summary
# said "All tests hermetic under poisoned environment." / "No findings: all
# ignored tests passed." -- for 33 days, nobody triaged what the lane found.
#
# The harness does NOT re-implement the reporter: it EXTRACTS the workflow's
# own publish steps and executes them against fixture logs, so a change in
# .github/workflows/bug-discovery.yml fails here instead of turning the nightly
# lane's summary into a lie.
#
# The fixtures are real shapes, copied from the CI logs of the runs this issue
# is about (the ANSI included: that is the part that broke the matcher).
#
# Everything runs against mktemp fixtures. No network, no real run, no state.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKFLOW="$REPO_ROOT/.github/workflows/bug-discovery.yml"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0
check() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '  OK   %s\n' "$label"
    PASS=$((PASS + 1))
  else
    printf '  FAIL %s — expected [%s], got [%s]\n' "$label" "$expected" "$actual"
    FAIL=$((FAIL + 1))
  fi
}

echo "test_bug_discovery_report: behavioral checks for the Bug Discovery reporter"

# ─── Fixtures (real nextest shapes, ANSI included) ───────────────────────────
ESC=$'\033'
RED="${ESC}[31;1m"
GREEN="${ESC}[32;1m"
RESET="${ESC}[0m"

# A failed test whose retries were exhausted. The status token is colorized
# and the escape code sits between it and the duration: this is the shape that
# no "FAIL at the start of the line" matcher ever sees.
cat >"$WORK/fixture-retry-fail.log" <<EOF
${RED}  TRY 3 FAIL${RESET} [   0.482s] (1/2) ${RED}webfang_core::behavioral${RESET} ${RED}cli::export_test${RESET}${RED}::${RESET}${RED}vector_export_total_documents_matches_documents${RESET}
${RED}     Summary${RESET} [  54.615s] 2 tests run: 0 passed, 2 failed, 5248 skipped
EOF

# An enforced flaky (flaky-result = "fail"): it says FLKY, not FAIL.
cat >"$WORK/fixture-flaky.log" <<EOF
${RED} FLKY-FL 2/3${RESET} [  79.107s] (5239/5241) ${RED}webfang_mcp::cli_mcp_export_jsonl_parity_test${RESET} ${RED}cli_vs_mcp_export_jsonl_record_parity${RESET}
${RED}     Summary${RESET} [ 687.892s] 5241 tests run: 5239 passed (5 slow), 2 failed, 9 skipped
EOF

# A test that did not FINISH (terminated by the slow-timeout). It is a finding
# too, and it is NOT a failure: saying "FAIL" about it would be a lie of a
# different kind.
cat >"$WORK/fixture-terminated.log" <<EOF
${ESC}[35m   TRY 1 TMT${RESET} [ 180.046s] (───) ${RED}webfang_mcp::cli_mcp_export_jsonl_parity_test${RESET} ${RED}cli_vs_mcp_export_jsonl_record_parity${RESET}
${ESC}[35mTRY 1 TRMNTG${RESET} [>180.000s] (───) ${RED}webfang_mcp::cli_mcp_export_jsonl_parity_test${RESET}
${RED}     Summary${RESET} [ 687.892s] 5241 tests run: 5239 passed (5 slow), 2 failed, 9 skipped
EOF

# A green run.
cat >"$WORK/fixture-green.log" <<EOF
${GREEN}        PASS${RESET} [   0.007s] (   1/2) ${RED}webfang_core::behavioral${RESET} ${RED}cli::export_test${RESET}${RED}::${RED}${RED}some_passing_test${RESET}
${RED}     Summary${RESET} [ 687.892s] 5241 tests run: 5241 passed (5 slow), 9 skipped
EOF

# nextest died before it ran anything (#2629): no summary at all.
cat >"$WORK/fixture-no-summary.log" <<'EOF'
error: failed to parse filterset
EOF

# The filter line mentions a test name and NOTHING failed (#2233): a mention
# is not a finding.
cat >"$WORK/fixture-filter-only.log" <<EOF
2026-10-14T06:00:12.4567890Z   -E 'not(test(/session_pool/) | test(/sitemap_entry_max_pages_truncates_many_url_fixture/))'
${RED}     Summary${RESET} [   4.210s] 128 tests run: 128 passed
EOF

# ─── Extract the workflow's own publish steps ────────────────────────────────
extract_run_block() {
  local needle="$1" out="$2"
  awk -v needle="$needle" '
    stage == 0 && index($0, needle) > 0 { stage = 1; next }
    stage == 1 && $0 ~ /^        run: \|$/ { stage = 2; next }
    stage == 2 {
      if ($0 ~ /^[[:space:]]*$/) { print ""; next }
      if ($0 !~ /^          /) { exit }
      sub(/^          /, "")
      print
    }
  ' "$WORKFLOW" >"$out"
}

extract_run_block "- name: Publish results" "$WORK/publish_poisoned.sh"
extract_run_block "- name: Publish failure names" "$WORK/publish_ignored.sh"

# run_step <extracted-step> <fixture> -> the summary the step produced
run_step() {
  local script="$1" fixture="$2"
  local tmp="$WORK/tmp"
  rm -rf "$tmp"
  mkdir -p "$tmp"
  cp "$fixture" "$tmp/bug-discovery-nextest.log"
  : >"$WORK/summary.md"
  GITHUB_WORKSPACE="$REPO_ROOT" \
    RUNNER_TEMP="$tmp" \
    GITHUB_STEP_SUMMARY="$WORK/summary.md" \
    bash "$script" >/dev/null
  cat "$WORK/summary.md"
}

echo "== 1. the reporter is the workflow's own code =="
check "the poisoned-env publish step is extracted" "yes" \
  "$([[ -s "$WORK/publish_poisoned.sh" ]] && echo yes || echo no)"
check "the ignored-tests publish step is extracted" "yes" \
  "$([[ -s "$WORK/publish_ignored.sh" ]] && echo yes || echo no)"
check "both steps call the shared reporter" "2" \
  "$(cat "$WORK/publish_poisoned.sh" "$WORK/publish_ignored.sh" | grep -c 'bug_discovery_report' || true)"
check "both steps read the log from RUNNER_TEMP" "2" \
  "$(cat "$WORK/publish_poisoned.sh" "$WORK/publish_ignored.sh" | grep -c 'RUNNER_TEMP' || true)"

echo "== 2. a red run is reported as red (#2666) =="
out="$(run_step "$WORK/publish_poisoned.sh" "$WORK/fixture-retry-fail.log")"
check "poisoned-env: the failed test is named" "yes" \
  "$(grep -q 'vector_export_total_documents_matches_documents' <<<"$out" && echo yes || echo no)"
check "poisoned-env: it does NOT claim hermeticity" "no" \
  "$(grep -q 'All tests hermetic' <<<"$out" && echo yes || echo no)"
check "poisoned-env: it says the tests are not hermetic" "yes" \
  "$(grep -q 'NOT hermetic' <<<"$out" && echo yes || echo no)"
check "poisoned-env: it lists exactly one finding" "1" \
  "$(grep -c '^- ' <<<"$out" || true)"

out="$(run_step "$WORK/publish_ignored.sh" "$WORK/fixture-retry-fail.log")"
check "ignored-tests: the failed test is named" "yes" \
  "$(grep -q 'vector_export_total_documents_matches_documents' <<<"$out" && echo yes || echo no)"
check "ignored-tests: it does NOT claim 'no findings'" "no" \
  "$(grep -q 'No findings' <<<"$out" && echo yes || echo no)"
check "ignored-tests: it asks for triage" "yes" \
  "$(grep -q 'potential production bugs' <<<"$out" && echo yes || echo no)"

echo "== 3. an enforced flaky is a finding too (flaky-result = fail) =="
out="$(run_step "$WORK/publish_poisoned.sh" "$WORK/fixture-flaky.log")"
check "poisoned-env: the flaky test is named" "yes" \
  "$(grep -q 'cli_vs_mcp_export_jsonl_record_parity' <<<"$out" && echo yes || echo no)"
check "poisoned-env: it does NOT claim hermeticity" "no" \
  "$(grep -q 'All tests hermetic' <<<"$out" && echo yes || echo no)"

out="$(run_step "$WORK/publish_ignored.sh" "$WORK/fixture-flaky.log")"
check "ignored-tests: the flaky test is named" "yes" \
  "$(grep -q 'cli_vs_mcp_export_jsonl_record_parity' <<<"$out" && echo yes || echo no)"
check "ignored-tests: it does NOT claim 'no findings'" "no" \
  "$(grep -q 'No findings' <<<"$out" && echo yes || echo no)"

echo "== 4. a test that did not finish is a finding too =="
out="$(run_step "$WORK/publish_poisoned.sh" "$WORK/fixture-terminated.log")"
check "poisoned-env: the terminated test is named" "yes" \
  "$(grep -q 'cli_vs_mcp_export_jsonl_record_parity' <<<"$out" && echo yes || echo no)"
check "poisoned-env: it does NOT claim hermeticity" "no" \
  "$(grep -q 'All tests hermetic' <<<"$out" && echo yes || echo no)"
check "poisoned-env: it says the status was a termination" "yes" \
  "$(grep -q 'TMT' <<<"$out" && echo yes || echo no)"

echo "== 5. a green run still says so =="
out="$(run_step "$WORK/publish_poisoned.sh" "$WORK/fixture-green.log")"
check "poisoned-env: a green run is reported green" "yes" \
  "$(grep -q 'All tests hermetic under poisoned environment.' <<<"$out" && echo yes || echo no)"
check "poisoned-env: a green run lists nothing" "0" \
  "$(grep -c '^- ' <<<"$out" || true)"

out="$(run_step "$WORK/publish_ignored.sh" "$WORK/fixture-green.log")"
check "ignored-tests: a green run is reported green" "yes" \
  "$(grep -q 'No findings: all ignored tests passed.' <<<"$out" && echo yes || echo no)"
check "ignored-tests: a green run lists nothing" "0" \
  "$(grep -c '^- ' <<<"$out" || true)"

echo "== 6. a run that never completed is not a green run (#2629) =="
out="$(run_step "$WORK/publish_poisoned.sh" "$WORK/fixture-no-summary.log")"
check "poisoned-env: it does NOT claim hermeticity" "no" \
  "$(grep -q 'All tests hermetic' <<<"$out" && echo yes || echo no)"
check "poisoned-env: it says the run did not complete" "yes" \
  "$(grep -q 'did not complete' <<<"$out" && echo yes || echo no)"

out="$(run_step "$WORK/publish_ignored.sh" "$WORK/fixture-no-summary.log")"
check "ignored-tests: it does NOT claim 'no findings'" "no" \
  "$(grep -q 'No findings' <<<"$out" && echo yes || echo no)"
check "ignored-tests: it says the run did not complete" "yes" \
  "$(grep -q 'did not complete' <<<"$out" && echo yes || echo no)"

echo "== 7. a mention of a test name is not a finding (#2233) =="
out="$(run_step "$WORK/publish_ignored.sh" "$WORK/fixture-filter-only.log")"
check "ignored-tests: a filter line is not a finding" "yes" \
  "$(grep -q 'No findings' <<<"$out" && echo yes || echo no)"
check "ignored-tests: the mentioned test is not listed" "no" \
  "$(grep -q 'sitemap_entry_max_pages_truncates_many_url_fixture' <<<"$out" && echo yes || echo no)"

echo "test_bug_discovery_report: PASS=$PASS FAIL=$FAIL"
if ((FAIL > 0)); then
  exit 1
fi
echo "OK: a red Bug Discovery run is reported as red."
