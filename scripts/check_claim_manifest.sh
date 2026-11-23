#!/usr/bin/env bash
# Shared claim-manifest runner (issue #2995).
# Usage: scripts/check_claim_manifest.sh [manifest.tsv] [base-dir]
# Defaults: scripts/claims-manifest.tsv and the repository root.
# Exit: 0 every registered stale claim is absent · 1 stale claim, bad row,
# out-of-scope path, missing file or unreadable manifest.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="${1:-scripts/claims-manifest.tsv}"
BASE="${2:-$ROOT}"

if [[ $MANIFEST != /* ]]; then
  MANIFEST="$ROOT/$MANIFEST"
fi
[[ -f $MANIFEST ]] || { printf '::error::claim manifest not readable: %s\n' "$MANIFEST" >&2; exit 1; }
[[ -d $BASE ]] || { printf '::error::claim manifest base directory not readable: %s\n' "$BASE" >&2; exit 1; }

FAILED=0
fail() { printf '::error::%s\n' "$*" >&2; FAILED=$((FAILED + 1)); }

ROWS=0
while IFS= read -r line || [[ -n $line ]]; do
  [[ -z $line ]] && continue
  [[ $line == \#* ]] && continue

  IFS=$'\t' read -r -a cols <<<"$line"
  if [[ ${#cols[@]} -ne 4 ]]; then
    fail "claim manifest row has ${#cols[@]} field(s), expected 4: $line"
    continue
  fi
  path="${cols[0]}"
  needle="${cols[1]}"
  source="${cols[2]}"
  rationale="${cols[3]}"

  ROWS=$((ROWS + 1))
  # A cosmetic ./ prefix must not bypass the scope and traversal checks.
  while [[ $path == ./* ]]; do
    path="${path#./}"
  done
  case "$path" in
    .github/*|deny.toml|docs/*|CHANGELOG.md)
      fail "claim manifest row targets out-of-scope path '$path' (issue #2995): $rationale"
      continue
      ;;
    ../*|*/../*|..|*/..|/*)
      fail "claim manifest row escapes the base directory: '$path'"
      continue
      ;;
  esac
  if [[ -z $path || -z $needle || -z $source || -z $rationale ]]; then
    fail "claim manifest row has an empty field: $line"
    continue
  fi

  target="$BASE/$path"
  if [[ ! -f $target ]]; then
    fail "claim manifest target is missing: '$path' (source of truth: $source)"
    continue
  fi
  if grep -Fq -- "$needle" "$target"; then
    fail "stale claim still present in '$path': '$needle' (source of truth: $source; $rationale)"
  fi
done <"$MANIFEST"

if [[ $ROWS -eq 0 ]]; then
  fail "claim manifest has no active rows: an empty manifest cannot guard anything"
fi
if (( FAILED > 0 )); then
  printf '::error::check_claim_manifest: %d finding(s)\n' "$FAILED" >&2
  exit 1
fi
printf 'check_claim_manifest: %d row(s) verified, 0 stale\n' "$ROWS"
