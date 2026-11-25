#!/usr/bin/env bash
set -euo pipefail
# test_static_guards_hook.sh — pins the pre-commit static-guard contract.
# Hermetic: every write lands in a mktemp git repo; the live tree is read-only.
#
# Proves:
#   1. pre-commit wires identity -> static-guards -> review, in that order.
#   2. static-guards allows the commit when every declared guard passes.
#   3. a failing guard denies the commit (exit 1) and surfaces its output.
#   4. a missing guard denies the commit (fail-closed, never a skip).
#
# Guard names are parsed from the runner itself, so the harness cannot drift
# from the production guard list by hand.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUNNER="$ROOT/scripts/githooks/static-guards.sh"
HOOK="$ROOT/scripts/githooks/pre-commit"

fail() { printf '::error::%s\n' "$*" >&2; exit 1; }

[[ -f $RUNNER ]] || fail "missing $RUNNER"
[[ -f $HOOK ]] || fail "missing $HOOK"

i_id=$(grep -n -E '^(exec )?".*identity-gate\.sh' "$HOOK" | head -1 | cut -d: -f1)
i_sg=$(grep -n -E '^(exec )?".*static-guards\.sh' "$HOOK" | head -1 | cut -d: -f1)
i_rv=$(grep -n -E '^(exec )?".*review-gate\.sh' "$HOOK" | head -1 | cut -d: -f1)
[[ -n $i_id && -n $i_sg && -n $i_rv ]] || fail "pre-commit does not wire all three gates"
(( i_id < i_sg && i_sg < i_rv )) || fail "pre-commit order must be identity -> static-guards -> review"

mapfile -t guards < <(sed -n '/^GUARDS=(/,/^)/p' "$RUNNER" | grep -E '^  scripts/' | tr -d ' ')
(( ${#guards[@]} > 0 )) || fail "no guards declared in $RUNNER"

# A hook guard must work on a fresh checkout. jq is not part of the Rust
# toolchain and is absent on minimal images; a guard that needs it belongs in
# ci_fast_gate.sh / CI, not in pre-commit.
for g in "${guards[@]}"; do
  if grep -v '^[[:space:]]*#' "$ROOT/$g" | grep -qE '(^|[^[:alnum:]_])jq([^[:alnum:]_]|$)'; then
    fail "guard $g requires jq, which is absent on fresh checkouts"
  fi
done

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
git -C "$TMP" init -q
mkdir -p "$TMP/scripts/githooks"
cp "$RUNNER" "$TMP/scripts/githooks/static-guards.sh"

for g in "${guards[@]}"; do printf '#!/usr/bin/env bash\nexit 0\n' > "$TMP/$g"; done
out="$(cd "$TMP" && bash scripts/githooks/static-guards.sh 2>&1)" || fail "all-pass case failed: $out"
printf '%s\n' "$out" | grep -q '^OK:' || fail "all-pass case did not report OK"

printf '#!/usr/bin/env bash\necho GUARD-DENY-MARKER >&2\nexit 1\n' > "$TMP/${guards[0]}"
if out="$(cd "$TMP" && bash scripts/githooks/static-guards.sh 2>&1)"; then
  fail "a failing guard was allowed"
fi
printf '%s\n' "$out" | grep -q 'GUARD-DENY-MARKER' || fail "failing guard output was swallowed"

rm "$TMP/${guards[0]}"
if out="$(cd "$TMP" && bash scripts/githooks/static-guards.sh 2>&1)"; then
  fail "a missing guard was skipped"
fi
printf '%s\n' "$out" | grep -q 'missing guard' || fail "missing guard not reported"

printf 'OK: test_static_guards_hook: wiring order, allow, deny and missing-guard paths verified (%d guards)\n' "${#guards[@]}"
