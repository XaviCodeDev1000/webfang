#!/usr/bin/env bash
# test_workflow_concurrency_hygiene.sh
#
# Semantics harness for scripts/check_workflow_concurrency_hygiene.sh.
# A gate that cannot fail is not a gate: each control writes fixtures that
# exercise ONE branch of the rule and asserts the verdict that branch must
# produce. Controls 2 and 3 are the two halves of the rule — the rule fails
# only when BOTH hold, so a control that kept firing with one half removed
# would prove nothing about which half it detected.
#
#   1  the REAL tree satisfies the gate            (vacuity: not red on healthy)
#   2  constant group + cancel-in-progress TRUE   (the 17:46 incident: W1 fires)
#   3  constant group + cancellation OFF          (the deliberate repo choice:
#                                                 it QUEUES, it must NOT fire)
#   4  the fixed shape (scoped run_id)            (the gate must accept it)
#   5  non-dispatchable + constant group + TRUE   (no manual run to protect: OK)
#   6  an empty workflow directory                (fail closed, never green)
#
# Every fixture is written into a throwaway directory and the gate runs against
# it through WORKFLOW_CONCURRENCY_ROOT, so no repo state is read or written and
# the harness is fully offline.

set -uo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
CHECK="$REPO_ROOT/scripts/check_workflow_concurrency_hygiene.sh"

failures=0
printf '== Workflow concurrency hygiene (%s)\n' "$REPO_ROOT"

run_gate() {
  WORKFLOW_CONCURRENCY_ROOT="$1" bash "$CHECK" >/dev/null 2>&1
}

expect_rc() {
  local what="$1" want="$2" got="$3"
  if [ "$got" = "$want" ]; then
    printf 'OK: %s\n' "$what"
  else
    printf 'FAIL: %s (expected rc=%s, got rc=%s)\n' "$what" "$want" "$got"
    failures=$((failures + 1))
  fi
}

# The fixed shape, exactly as ci.yml now carries it, and the constant shape
# that was there before. Both are GitHub Actions expressions that must reach the
# fixture VERBATIM, so they are deliberately single-quoted and shellcheck's
# SC2016 ("$ in single quotes") is disabled at each line with that reason — the
# values are not shell expansions, they are the literal group text under test.
# shellcheck disable=SC2016
fixed_group='${{ github.workflow }}-${{ github.event_name }}-${{ github.head_ref || github.ref }}${{ github.event_name == '"'"'workflow_dispatch'"'"' && format('"'"'-run-{0}'"'"', github.run_id) || '"'"''"'"' }}'
# shellcheck disable=SC2016
constant_group='${{ github.workflow }}-${{ github.event_name }}-${{ github.head_ref || github.ref }}'

write_workflow() {
  local root="$1" name="$2" body="$3"
  mkdir -p "$root/.github/workflows"
  printf '%s\n' "$body" >"$root/.github/workflows/$name"
}

# ---------------------------------------------------------------------------
# Control 1 — the real tree satisfies the gate.
# ---------------------------------------------------------------------------
run_gate "$REPO_ROOT"
expect_rc "the real tree satisfies the workflow concurrency gate" 0 "$?"

# ---------------------------------------------------------------------------
# Controls 2-6 — the gate must produce the required verdict on each shape.
# ---------------------------------------------------------------------------
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

# 2 — THE INCIDENT SHAPE: dispatchable, constant group, cancellation ON for the
#     event. This is a verbatim ci.yml before #2776, and the shape that killed
#     a 240-minute run at minute 128 on 2026-11-12.
# NEGATION: this control MUST fail. It is the one that proves the gate has teeth:
# if it ever reports 0 here, the gate has stopped detecting the defect it exists
# for and its green on the real tree means nothing.
write_workflow "$T" "incident.yml" "on:
  workflow_dispatch:
concurrency:
  group: ${constant_group}
  cancel-in-progress: true
jobs:
  probe:
    runs-on: ubuntu-latest
    steps:
      - run: true
"
run_gate "$T"
expect_rc "control 2: constant group WITH cancellation is rejected (the 17:46 shape)" 1 "$?"

# 3 — THE DELIBERATE CHOICE: constant group but cancellation is false. The
#     repository writes this at benches.yml ("never kill a nightly
#     mid-measurement") and ci-health.yml ("never drop a tracking update
#     mid-write"). Such a run QUEUES behind its predecessor instead of
#     destroying it, so the rule must NOT fire — this control is what proves
#     the gate did not overreach and override an informed decision.
# NEGATION: this control must NOT fail. It is the boundary assertion: the first
# version of the gate demanded uniqueness from every dispatchable workflow and
# failed on 16 of 17, which was an overreaching rule and not a broken repo.
# Without this control the gate could overreach again with nothing to catch it.
write_workflow "$T" "incident.yml" "on:
  workflow_dispatch:
concurrency:
  group: ${constant_group}
  cancel-in-progress: false
jobs:
  probe:
    runs-on: ubuntu-latest
    steps:
      - run: true
"
run_gate "$T"
expect_rc "control 3: constant group with cancellation OFF is accepted (it queues, it does not kill)" 0 "$?"

# 4 — the FIXED SHAPE. If the gate ever rejected this, the gate would be the bug.
write_workflow "$T" "incident.yml" "on: [push, workflow_dispatch]
concurrency:
  group: ${fixed_group}
  cancel-in-progress: true
jobs:
  probe:
    runs-on: ubuntu-latest
    steps:
      - run: true
"
run_gate "$T"
expect_rc "control 4: the fixed shape (run_id scoped to workflow_dispatch) is accepted" 0 "$?"

# 5 — not dispatchable: there is no manual run to protect, so a constant group
#     with cancellation ON is out of scope for this rule.
write_workflow "$T" "incident.yml" "on: [push]
concurrency:
  group: ${constant_group}
  cancel-in-progress: true
jobs:
  probe:
    runs-on: ubuntu-latest
    steps:
      - run: true
"
run_gate "$T"
expect_rc "control 5: a non-dispatchable workflow is out of scope" 0 "$?"

# 6 — an empty workflow directory is a red gate, never a green one: a glob that
#     matched nothing must not read as "every workflow is healthy".
EMPTY="$(mktemp -d)"
mkdir -p "$EMPTY/.github/workflows"
run_gate "$EMPTY"
expect_rc "control 6: an empty workflow directory fails closed" 1 "$?"
rm -rf "$EMPTY"

# ---------------------------------------------------------------------------
printf '\n'
if [ "$failures" -ne 0 ]; then
  printf 'FAIL: %d control(s) did not produce the required verdict\n' "$failures"
  exit 1
fi
printf 'OK: all %d controls produced the required verdict\n' 6
exit 0
