#!/usr/bin/env bash
# static-guards.sh — fail-closed static gates for pre-commit.
#
# These are the deterministic guards that work on a fresh checkout with no
# extra tool installation and finish in a few seconds. The expensive CI
# surface (clippy, nextest, doc, duplication, intra-crate lint tool) and any
# guard needing jq or an unbuilt binary stay in ci_fast_gate.sh and CI.
#
# Every guard is mandatory: a missing script is a violation, never a skip.
# Adding a guard that needs an absent-by-default tool would deny every commit
# on a fresh checkout for a reason that is not a violation.

set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  printf '::error::static-guards: not inside a git worktree\n' >&2
  exit 1
}
cd "$ROOT" || exit 1

GUARDS=(
  scripts/check_agents_doc.sh
  scripts/check_file_size.sh
  scripts/check_docs_version_pins.sh
  scripts/check_docs_claims.sh
  scripts/check_nextest_invocation_lint.sh
  scripts/check_concurrency_lints.sh
)

FAILED=0
for guard in "${GUARDS[@]}"; do
  if [[ ! -f $guard ]]; then
    printf '::error::static-guards: missing guard %s\n' "$guard" >&2
    FAILED=$((FAILED + 1))
    continue
  fi
  if ! output="$(bash "$guard" 2>&1)"; then
    printf '::error::static-guards: %s denied the commit\n%s\n' "$guard" "$output" >&2
    FAILED=$((FAILED + 1))
  fi
done

if (( FAILED > 0 )); then
  printf '::error::static-guards: %d guard(s) denied the commit\n' "$FAILED" >&2
  exit 1
fi

printf 'OK: static-guards: %d guards passed\n' "${#GUARDS[@]}"
