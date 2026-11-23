#!/usr/bin/env bash
# Semantics harness for the shared claim-manifest runner (#2995).
# Offline fixtures only; asserts observable exit codes, never incidental text.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="$ROOT/scripts/check_claim_manifest.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAILED=0
pass() { printf 'ok - %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'not ok - %s\n' "$1" >&2; FAILED=$((FAILED + 1)); }

expect() {
  local name="$1" expected="$2" manifest="$3" actual
  set +e
  bash "$RUNNER" "$manifest" "$TMP/base" >"$TMP/out" 2>&1
  actual=$?
  set -e
  if [[ $actual -eq $expected ]]; then
    pass "$name"
  else
    fail "$name (expected $expected, got $actual)"
  fi
}

mkdir -p "$TMP/base"
printf '# clean fixture\n' >"$TMP/base/doc.md"
printf '%s\n' '# fixture manifest' >"$TMP/clean.tsv"
printf 'doc.md\tselectors 0.35.0\tCargo.lock\tlocked versions differ\n' >>"$TMP/clean.tsv"

printf 'doc.md\tselectors 0.35.0\tCargo.lock\tlocked versions differ\n' >"$TMP/stale.tsv"
printf 'missing.md\tselectors 0.35.0\tCargo.lock\tmissing target\n' >"$TMP/missing.tsv"
printf 'doc.md\tselectors 0.35.0\tCargo.lock\n' >"$TMP/malformed.tsv"
printf '.github/workflows/ci.yml\tselectors 0.35.0\tCargo.lock\tout of scope\n' >"$TMP/scope.tsv"
printf '%s\n' '# only comments' >"$TMP/empty.tsv"

expect 'clean manifest passes' 0 "$TMP/clean.tsv"
printf 'selectors 0.35.0\n' >>"$TMP/base/doc.md"
expect 'stale claim fails (RED repro)' 1 "$TMP/stale.tsv"
printf '# clean fixture\n' >"$TMP/base/doc.md"
expect 'restored file passes again' 0 "$TMP/clean.tsv"
expect 'missing target fails closed' 1 "$TMP/missing.tsv"
expect 'malformed row fails closed' 1 "$TMP/malformed.tsv"
expect 'out-of-scope path fails closed' 1 "$TMP/scope.tsv"
expect 'empty manifest fails closed' 1 "$TMP/empty.tsv"
expect 'unreadable manifest fails closed' 1 "$TMP/does-not-exist.tsv"

printf 'claim-manifest semantics: %d passed, %d failed\n' "$PASS" "$FAILED"
[[ $FAILED -eq 0 ]]
