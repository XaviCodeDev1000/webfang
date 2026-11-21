#!/usr/bin/env bash
# test_sweep_mtime_lsof.sh — registry, newest-file mtime, lsof and symlink cases.

set -euo pipefail

SD="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SD/../.." && pwd)"
SWEEP="$ROOT/scripts/sweep_orphan_targets.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

R="$TMP/repo"
T="$TMP/cache"
W="$TMP/wt-one"
mkdir -p "$R" "$T"

git -C "$R" init -q -b main
git -C "$R" -c user.email=t@e -c user.name=t commit -q --allow-empty -m init
printf 'export CARGO_TARGET_DIR=%s/declared-live\n' "$T" >"$R/.envrc"
git -C "$R" worktree add -q "$W" -b feat/one
printf 'export CARGO_TARGET_DIR=%s/one-target\n' "$T" >"$W/.envrc"

mk() {
    mkdir -p "$1"
    : >"$1/CACHEDIR.TAG"
    printf 'payload\n' >"$1/file"
}

for d in declared-live one-target wt-one old mtime-dirnew-fileold mtime-dirold-filenew \
    seeds seeds-webfang quarantine quarantine-webfang; do
    mk "$T/$d"
done
find "$T" -type f -exec touch -d '10 days ago' {} +
touch "$T/mtime-dirnew-fileold"
touch -d '10 days ago' "$T/mtime-dirold-filenew"
touch "$T/mtime-dirold-filenew/file"
ln -s "$T/seeds" "$T/evil-link"

OUT="$("$SWEEP" --repo "$R" --target-cache "$T" --idle 72h)"
for d in wt-one old mtime-dirnew-fileold; do
    grep -q "\"$d\"" <<<"$OUT" || { echo "FAIL orphan $d" >&2; exit 1; }
done
for d in declared-live one-target mtime-dirold-filenew seeds seeds-webfang quarantine \
    quarantine-webfang evil-link; do
    ! grep -q "\"$d\"" <<<"$OUT" || { echo "FAIL classified $d" >&2; exit 1; }
done

mk "$T/held"
find "$T/held" -type f -exec touch -d '10 days ago' {} +
exec 9>"$T/held/file"
OUT="$("$SWEEP" --repo "$R" --target-cache "$T" --idle 72h)"
exec 9>&-
! grep -q '"held"' <<<"$OUT" || { echo "FAIL lsof-held classified" >&2; exit 1; }

echo "sweep mtime/lsof harness: PASS"
