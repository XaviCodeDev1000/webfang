#!/usr/bin/env bash
# Wiring harness for the shared claim-manifest runner (#2995).
# Pins the EDGES of ci_fast_gate.sh: both guard lanes, both harness steps and
# the trigger that makes a scripts/**-only change run them.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GATE="$ROOT/scripts/ci_fast_gate.sh"
MANIFEST="$ROOT/scripts/claims-manifest.tsv"
RUNNER="$ROOT/scripts/check_claim_manifest.sh"

PASS=0
FAILED=0
pass() { printf 'ok - %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'not ok - %s\n' "$1" >&2; FAILED=$((FAILED + 1)); }

check_file() {
  if [[ -f $1 ]]; then pass "$2"; else fail "$2"; fi
}
check_gate() {
  if grep -Fq -- "$1" "$GATE"; then pass "$2"; else fail "$2"; fi
}

check_file "$RUNNER" 'runner exists'
check_file "$MANIFEST" 'manifest exists'

GUARD='run_guard "shared claim manifest gate (#2995)" scripts/check_claim_manifest.sh'
count="$(grep -Fc -- "$GUARD" "$GATE" || true)"
if [[ $count -eq 2 ]]; then
  pass 'guard wired in both docs and code lanes'
else
  fail "guard wired in both lanes (found $count of 2)"
fi

check_gate 'run_step "claim manifest semantics (offline)" bash scripts/test_claim_manifest.sh' 'semantics harness wired'
check_gate 'run_step "claim manifest wiring (offline)" bash scripts/test_claim_manifest_wiring.sh' 'wiring harness wired'
check_gate 'scripts/check_claim_manifest\.sh' 'trigger includes the runner'
check_gate 'scripts/test_claim_manifest\.sh' 'trigger includes the semantics harness'

if awk -F'\t' '
  /^[[:space:]]*#/ { next }
  /^[[:space:]]*$/ { next }
  $1 ~ /^\.github\// || $1 == "deny.toml" || $1 ~ /^docs\// || $1 == "CHANGELOG.md" { bad = 1 }
  END { exit bad ? 1 : 0 }
' "$MANIFEST"; then
  pass 'manifest contains no out-of-scope paths'
else
  fail 'manifest contains an out-of-scope path'
fi

rows="$(awk -F'\t' '/^[[:space:]]*#/ { next } /^[[:space:]]*$/ { next } { n++ } END { print n + 0 }' "$MANIFEST")"
if [[ $rows -gt 0 ]]; then pass "manifest has $rows active rows"; else fail 'manifest has no active rows'; fi

printf 'claim-manifest wiring: %d passed, %d failed\n' "$PASS" "$FAILED"
[[ $FAILED -eq 0 ]]
