#!/usr/bin/env bash
# Semantics harness for the test-inventory drift guard
# (scripts/check_test_inventory.sh).
#
# A committed artifact that no gate reads is a claim with an expiry date
# nobody watches: docs/test-inventory.json described 163 files / 1,131 tests
# while the tree had 173 files and the suite ran 4,835 (audit 2026-10-24).
# The guard regenerates into the working tree and compares; this harness pins
# the behavior that cannot be proven by reading the script:
#
#   1. it PASSES when the committed inventory matches a fresh regeneration;
#   2. it FAILS when the inventory is stale, names the generator in the
#      remediation message, and prints a diff (not a bare exit);
#   3. it never leaves the working tree mutated — the original file is
#      restored on every exit path, including the failure one;
#   4. it fails CLOSED (exit 2) when the inventory is missing;
#   5. it fails CLOSED (exit 2) when the generator cannot run;
#   6. it fails CLOSED (exit 2) when the generator exits nonzero — a producer
#      that crashes must not look like a green comparison.
#
# Everything runs against mktemp fixtures. No network, no mutation of the
# repository this harness runs from.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# new_fixture <name>: a tree with the guard, a deterministic generator, and a
# committed inventory. The generator is deterministic and independent of the
# tree contents: it writes a fixed payload, so "current" and "stale" differ
# only by what the fixture commits.
new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/scripts" "$dir/docs"
  cp "$REPO_ROOT/scripts/check_test_inventory.sh" "$dir/scripts/"
  chmod +x "$dir/scripts/check_test_inventory.sh"

  cat > "$dir/scripts/ci_test_inventory.py" <<'EOF'
import json, pathlib
payload = {"files": [{"path": "crates/webfang_core/src/lib.rs", "test_count": 1}]}
pathlib.Path("docs/test-inventory.json").write_text(json.dumps(payload, indent=2) + "\n")
EOF

  # The committed inventory is produced by the generator itself, so "current"
  # means byte-identical to a fresh regeneration by construction.
  (cd "$dir" && python3 scripts/ci_test_inventory.py)
  printf '%s\n' "$dir"
}

# run <fixture> <expected-exit> <name> — runs the guard, asserts the exit code,
# and leaves stdout+stderr in the fixture's out.log for content assertions.
run() {
  local dir="$1" want="$2" name="$3" got=0
  (cd "$dir" && bash scripts/check_test_inventory.sh) >"$dir/out.log" 2>&1 || got=$?
  if [[ $got -eq $want ]]; then
    pass "$name (exit $got)"
  else
    fail "$name (esperaba exit $want, obtuve $got)"
    sed -n '1,5p' "$dir/out.log" | sed 's/^/       /'
  fi
}

printf 'check_test_inventory semantics\n'

# 1. current inventory passes
d="$(new_fixture current)"
run "$d" 0 "inventario al día"

# NEGATION: pre-fix nothing compared the committed artifact against a fresh
# regeneration, so it described 163 files while the tree had 173 — four
# months of drift read by nobody.
d="$(new_fixture stale)"
printf '%s\n' '{"files": [{"path": "crates/webfang_core/src/lib.rs", "test_count": 999}]}' \
  >"$d/docs/test-inventory.json"
run "$d" 1 "inventario desactualizado"
if grep -q 'ci_test_inventory.py' "$d/out.log"; then
  pass "el mensaje de remediación nombra al generador"
else
  fail "el mensaje de remediación no nombra al generador"
fi
if grep -q '^[-+]' "$d/out.log"; then
  pass "el fallo imprime el diff"
else
  fail "el fallo no imprime el diff"
fi

# NEGATION: pre-fix a crash mid-run left the regenerated (wrong) inventory in
# the tree; the original file is restored on every exit path.
d="$(new_fixture restored)"
printf '%s\n' '{"files": [{"path": "crates/webfang_core/src/lib.rs", "test_count": 999}]}' \
  >"$d/docs/test-inventory.json"
cp "$d/docs/test-inventory.json" "$d/expected.json"
run "$d" 1 "fallo por desactualización"
if cmp -s "$d/expected.json" "$d/docs/test-inventory.json"; then
  pass "el árbol queda intacto tras el fallo"
else
  fail "el fallo dejó el árbol mutado"
fi

# NEGATION: pre-fix a missing artifact was indistinguishable from an empty
# one; a guard that cannot see must fail closed, never skip.
d="$(new_fixture no-inventory)"
rm -f "$d/docs/test-inventory.json"
run "$d" 2 "sin inventario (fail-closed)"

# NEGATION: pre-fix a missing producer failed the comparison open — nothing
# to compare against is not a green run.
d="$(new_fixture no-generator)"
rm -f "$d/scripts/ci_test_inventory.py"
run "$d" 2 "sin generador (fail-closed)"

# NEGATION: pre-fix a generator that crashed produced no output and the guard
# compared garbage; a nonzero producer must fail the step, never green it.
d="$(new_fixture generator-crash)"
cat > "$d/scripts/ci_test_inventory.py" <<'EOF'
import sys
print("simulated crash", file=sys.stderr)
sys.exit(3)
EOF
run "$d" 2 "generador que falla (fail-closed)"

printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
(( FAILED == 0 )) || exit 1
