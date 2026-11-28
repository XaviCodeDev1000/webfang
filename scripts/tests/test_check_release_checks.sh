#!/usr/bin/env bash
#
# test_check_release_checks.sh — semantics harness for scripts/check_release_checks.sh
# (#3105, Guard 1).
#
# WHAT IT PINS. The decision table the guard's header documents: exit 0 (a
# green `CI Gate`, with a non-required failure warned and never blocking), exit 1
# (a required context missing, failed or still running) and exit 2 (cannot tell,
# with the operator's remedy). The API read itself cannot run here — no network,
# no token — so the harness drives the guard's fixture mode, plus a `gh` STUBBED
# ON PATH for the API-error branches.
#
# THE REQUIRED SET. Releases are squash merges, so the tag commit carries the
# aggregate `CI Gate` but not the PR-scoped contexts (`Validate PR metadata`,
# `cargo-mutants (PR diff)`, `breaking-surface`): those attach to the PR head SHA
# the squash replaced. Requiring all four would block every release, which is
# what the old case table accidentally asserted and the squash-merge row below
# now prevents from coming back.
#
# Usage: bash scripts/tests/test_check_release_checks.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="$ROOT/scripts/check_release_checks.sh"
RELEASE_YML="$ROOT/.github/workflows/release.yml"

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
if [ ! -r "$RELEASE_YML" ]; then
  echo "wiring target not found: $RELEASE_YML" >&2
  exit 1
fi

FIXTURES="$(mktemp -d)"
trap 'rm -rf "$FIXTURES"' EXIT

# run_case <fixture|-> : drive the guard's fixture mode. `-` passes a path that
# does not exist — the cannot-tell shape where the answer file is missing.
OUT=""
RC=0
run_case() {
  local fixture="$1"
  if [ "$fixture" != "-" ]; then
    OUT="$(RELEASE_CHECKS_FIXTURE="$fixture" GITHUB_SHA=0000000000000000000000000000000000000000 \
      bash "$GUARD" checks 2>&1)"
  else
    OUT="$(RELEASE_CHECKS_FIXTURE="$FIXTURES/does-not-exist.json" GITHUB_SHA=0000000000000000000000000000000000000000 \
      bash "$GUARD" checks 2>&1)"
  fi
  RC=$?
}

# run_api_case <stub-dir> : drive the real API branch with a `gh` stub first on
# PATH; the stub decides what the API "answers", nothing leaves the machine.
run_api_case() {
  OUT="$(env -u RELEASE_CHECKS_FIXTURE PATH="$1:$PATH" GH_TOKEN=stub-token \
    GITHUB_REPOSITORY=owner/repo GITHUB_SHA=0000000000000000000000000000000000000000 \
    bash "$GUARD" checks 2>&1)"
  RC=$?
}

# make_failing_gh <dir> <stderr-line> : a `gh` that fails with that message.
make_failing_gh() {
  local dir="$1" msg="$2"
  mkdir -p "$dir"
  {
    echo '#!/usr/bin/env bash'
    printf 'echo %q >&2\n' "$msg"
    echo 'exit 1'
  } >"$dir/gh"
  chmod +x "$dir/gh"
}

# One JSON document per decision-table row, in the check-runs API shape the
# guard normalizes. A PR-shaped answer plus one failed advisory job: green with
# a warning, never a block.
cat >"$FIXTURES/green.json" <<'JSON'
{
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "success"},
    {"name": "cargo-mutants (PR diff)", "status": "completed", "conclusion": "success"},
    {"name": "Validate PR metadata", "status": "completed", "conclusion": "success"},
    {"name": "breaking-surface", "status": "completed", "conclusion": "success"},
    {"name": "an unrelated advisory job", "status": "completed", "conclusion": "failure"}
  ]
}
JSON

# The RED required context: the one decision this gate exists to make.
cat >"$FIXTURES/red-gate.json" <<'JSON'
{
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "failure"},
    {"name": "Validate PR metadata", "status": "completed", "conclusion": "success"}
  ]
}
JSON

# The commit carries no `CI Gate` run at all: missing is red, not "unknown".
cat >"$FIXTURES/missing-gate.json" <<'JSON'
{
  "check_runs": [
    {"name": "cargo-mutants (PR diff)", "status": "completed", "conclusion": "success"},
    {"name": "Validate PR metadata", "status": "completed", "conclusion": "success"},
    {"name": "breaking-surface", "status": "completed", "conclusion": "success"}
  ]
}
JSON

# The SQUASH-MERGE shape: only the aggregate survives on main, which is what a
# real release tag points at — and it must be GREEN.
cat >"$FIXTURES/squash-merge.json" <<'JSON'
{
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "success"}
  ]
}
JSON

cat >"$FIXTURES/scoped-running.json" <<'JSON'
{
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "success"},
    {"name": "cargo-mutants (PR diff)", "status": "in_progress", "conclusion": null}
  ]
}
JSON

cat >"$FIXTURES/red-and-queued.json" <<'JSON'
{
  "check_runs": [
    {"name": "CI Gate", "status": "completed", "conclusion": "cancelled"},
    {"name": "cargo-mutants (PR diff)", "status": "queued", "conclusion": null}
  ]
}
JSON

cat >"$FIXTURES/malformed.json" <<'JSON'
{
  "message": "Resource not accessible by integration"
}
JSON

# decision table
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

echo "B: a green CI Gate proceeds; a failed NON-required job warns, never blocks"
run_case "$FIXTURES/green.json"
expect_eq "green fixture" 0 "$RC"
expect_not_contains "green fixture prints no error annotation" "$OUT" "::error::"
expect_contains "the failed advisory job is NAMED" "$OUT" "an unrelated advisory job"
expect_contains "its conclusion is NAMED, not summarized" "$OUT" "conclusion=failure"

echo "C: a red CI Gate blocks -> 1 and the message NAMES it"
run_case "$FIXTURES/red-gate.json"
expect_eq "red CI Gate" 1 "$RC"
expect_contains "the red context is named" "$OUT" "CI Gate"
expect_contains "failure annotation" "$OUT" "::error::"

echo "D: a MISSING CI Gate blocks -> 1 and the message says it is missing"
run_case "$FIXTURES/missing-gate.json"
expect_eq "missing CI Gate" 1 "$RC"
expect_contains "the missing context is named" "$OUT" "CI Gate"
expect_contains "missing is reported as missing" "$OUT" "missing"

echo "E: the SQUASH-MERGE shape is green — the PR-scoped set cannot be required here"
run_case "$FIXTURES/squash-merge.json"
expect_eq "squash-merge shape" 0 "$RC"
expect_not_contains "squash-merge shape prints no error annotation" "$OUT" "::error::"
expect_contains "the absent PR-scoped context is NAMED as absent" "$OUT" \
  "cargo-mutants (PR diff): no such check-run"
expect_contains "absence is attributed to the squash merge" "$OUT" "expected for a squash merge"

echo "F: a still-running NON-required context warns with its status, does not block"
run_case "$FIXTURES/scoped-running.json"
expect_eq "running non-required context" 0 "$RC"
expect_not_contains "running non-required context prints no error annotation" "$OUT" "::error::"
expect_contains "the running context is named" "$OUT" "cargo-mutants (PR diff)"
expect_contains "its observed status is named" "$OUT" "in_progress"

echo "G: red CI Gate + a queued non-required context — error for one, warning for the other"
run_case "$FIXTURES/red-and-queued.json"
expect_eq "red gate with a queued scoped context" 1 "$RC"
expect_contains "the red context is named" "$OUT" "CI Gate"
expect_contains "the non-required context is named in a warning" "$OUT" \
  "::warning::Release checks: cargo-mutants (PR diff): conclusion=none, status=queued"
ERROR_LINES="$(printf '%s\n' "$OUT" | grep '::error::' || true)"
expect_not_contains "the non-required context is NOT in the blocking error line" "$ERROR_LINES" "cargo-mutants"
if [ "$(printf '%s\n' "$ERROR_LINES" | grep -c '::error::' || true)" -eq 1 ]; then
  ok "offenders are reported in ONE blocking error line"
else
  bad "expected exactly one ::error:: line, got: $ERROR_LINES"
fi

echo "H: cannot tell -> 2 (never a silent pass)"
run_case -
expect_eq "unreadable answer" 2 "$RC"
expect_contains "cannot-tell annotation" "$OUT" "::error::"

run_case "$FIXTURES/malformed.json"
expect_eq "unparsable answer" 2 "$RC"
expect_contains "unparsable annotation" "$OUT" "::error::"

echo "I: HTTP 403 carries the REMEDY — permission, file, retry (one line)"
make_failing_gh "$FIXTURES/gh-403" "gh: Resource not accessible by integration (HTTP 403)"
run_api_case "$FIXTURES/gh-403"
expect_eq "403 is cannot-tell" 2 "$RC"
expect_contains "403 is named" "$OUT" "HTTP 403"
expect_contains "the missing permission is named" "$OUT" "'checks: read'"
expect_contains "the file that declares it is named" "$OUT" ".github/workflows/release.yml"
expect_contains "the retry action is named" "$OUT" "re-run the release workflow"
if [ "$(printf '%s\n' "$OUT" | grep -c '::error::' || true)" -eq 1 ]; then
  ok "the 403 remedy is one line"
else
  bad "expected exactly one ::error:: line for 403"
fi

echo "J: HTTP 404 and auth/network failures carry their own remedy"
make_failing_gh "$FIXTURES/gh-404" "gh: Not Found (HTTP 404)"
run_api_case "$FIXTURES/gh-404"
expect_eq "404 is cannot-tell" 2 "$RC"
expect_contains "404 is named" "$OUT" "HTTP 404"
expect_contains "404 names the retry action" "$OUT" "re-run the release workflow"

make_failing_gh "$FIXTURES/gh-network" "error connecting to api.github.com: could not resolve host"
run_api_case "$FIXTURES/gh-network"
expect_eq "network failure is cannot-tell" 2 "$RC"
expect_contains "the network failure names the endpoint" "$OUT" "api.github.com"
expect_contains "the network failure names the retry action" "$OUT" "re-run the release workflow"

# wiring (asserted by the commit that wires the step)
echo "K: release.yml wiring — preflight, after L1.5, before build, with checks: read"
PREFLIGHT_BLOCK="$(awk '
  /^  preflight:$/ { inm = 1; print; next }
  inm && /^  [A-Za-z0-9_-]+:$/ { inm = 0 }
  inm { print }
' "$RELEASE_YML")"

if grep -qE '^[[:space:]]*bash scripts/check_release_checks\.sh checks$' <<<"$PREFLIGHT_BLOCK"; then
  ok "the guard is invoked from the preflight job with the checks subcommand"
else
  bad "release.yml no longer invokes 'bash scripts/check_release_checks.sh checks' from preflight"
fi

INVOKE_LINE="$(grep -nE '^[[:space:]]*bash scripts/check_release_checks\.sh checks$' "$RELEASE_YML" | head -1 | cut -d: -f1)"
L15_LINE="$(grep -nE '^[[:space:]]*- name: L1\.5 Tag Ruleset Check$' "$RELEASE_YML" | head -1 | cut -d: -f1)"
BUILD_LINE="$(grep -nE '^  build:$' "$RELEASE_YML" | head -1 | cut -d: -f1)"
if [ -n "$INVOKE_LINE" ] && [ -n "$L15_LINE" ] && [ "$INVOKE_LINE" -gt "$L15_LINE" ]; then
  ok "the check-runs step is declared after the L1.5 Tag Ruleset step (line $INVOKE_LINE > $L15_LINE)"
else
  bad "the check-runs step is not declared after L1.5 (invoke=${INVOKE_LINE:-none}, L1.5=${L15_LINE:-none})"
fi
if [ -n "$INVOKE_LINE" ] && [ -n "$BUILD_LINE" ] && [ "$INVOKE_LINE" -lt "$BUILD_LINE" ]; then
  ok "the check-runs step is declared before build (line $INVOKE_LINE < $BUILD_LINE)"
else
  bad "the check-runs step is not declared before build (invoke=${INVOKE_LINE:-none}, build=${BUILD_LINE:-none})"
fi

# The step that invokes the guard, extracted as a block: from its `- name:`/
# `- uses:` header to the next one. Matching the INVOCATION LINE matters — the
# step's own comment names the script too, and a prose mention is not the step.
INVOKING_STEP_MARKER='bash scripts/check_release_checks[.]sh checks'
TAIL_AFTER_L15="$(printf '%s\n' "$PREFLIGHT_BLOCK" | awk -v marker="$INVOKING_STEP_MARKER" '
  /^      - (name|uses):/ {
    if (in_step && buf ~ marker) { printf "%s", buf; found = 1; exit }
    in_step = 1; buf = $0 ORS; next
  }
  in_step { buf = buf $0 ORS }
  END { if (!found && buf ~ marker) printf "%s", buf }
')"
expect_contains "the step passes the API token" "$TAIL_AFTER_L15" "GH_TOKEN: \${{ github.token }}"
expect_contains "the step passes the repository" "$TAIL_AFTER_L15" "GITHUB_REPOSITORY: \${{ github.repository }}"
expect_contains "the step passes the VALIDATED commit" "$TAIL_AFTER_L15" \
  "PROV_VALIDATED_COMMIT: \${{ steps.prov.outputs.validated_commit }}"

PREFLIGHT_PERMS="$(printf '%s\n' "$PREFLIGHT_BLOCK" | awk '
  /^    permissions:/ { p = 1; next }
  p && /^    [a-z-]+:/ { p = 0 }
  p { print }
')"
if grep -qxE '[[:space:]]*checks: read' <<<"$PREFLIGHT_PERMS"; then
  ok "the preflight job declares 'checks: read' (the API read is permitted)"
else
  bad "the preflight job does not declare 'checks: read' — the API read can only answer 403"
fi
if grep -qxE '[[:space:]]*contents: read' <<<"$PREFLIGHT_PERMS"; then
  ok "the preflight job keeps 'contents: read'"
else
  bad "the preflight job lost 'contents: read'"
fi

# summary
if [ "$fail" -ne 0 ]; then
  echo "test_release_checks: FAIL ($fail failed, $pass passed)"
  exit 1
fi
echo "test_release_checks: PASS ($pass assertions)"
exit 0

# NEGATION: the guard replaced by `exit 0` must turn this harness red (the case
# table would report a silent pass on every row that expects 1 or 2).
# NEGATION: adding a PR-scoped context back to REQUIRED_CONTEXTS must turn it
# red — E is the squash-merge shape and expects 0, while the guard would then
# report the absent context as an offender and exit 1.
# NEGATION: making a non-success conclusion pass must turn it red — C, D and G.
# NEGATION: turning an absent or failed NON-required context into a failure must
# turn it red — B, E and F expect 0 plus a ::warning:: naming it.
# NEGATION: turning `cannot tell` into a pass must turn it red — H, I and J.
# NEGATION: dropping the 403 remedy sentence (permission, file or retry) must
# turn it red — I asserts each of the three parts by text.
# NEGATION: deleting the check-runs step from release.yml, disabling
# `checks: read`, or moving the step below `build` must turn it red — K.
