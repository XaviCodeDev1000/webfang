#!/usr/bin/env bash
# test_sweep_orphan_targets.sh — semantics harness for sweep_orphan_targets.sh.
# Uses an ephemeral git repo and a fake cache root; no real target dirs touched.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SWEEP="$ROOT/scripts/sweep_orphan_targets.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

REPO="$TMP/repo"
CACHE="$TMP/cache"
WT="$TMP/wt"
mkdir -p "$REPO" "$CACHE"

git -C "$REPO" init -q
git -C "$REPO" -c user.email=test@example.com -c user.name=test commit -q --allow-empty -m init

printf 'export CARGO_TARGET_DIR=%s/main\n' "$CACHE" >"$REPO/.envrc"
git -C "$REPO" worktree add -q "$WT" -b test/branch
printf 'export CARGO_TARGET_DIR=%s/test-branch\n' "$CACHE" >"$WT/.envrc"

make_target() {
    mkdir -p "$1"
    : >"$1/CACHEDIR.TAG"
    printf 'payload\n' >"$1/file"
}

make_target "$CACHE/main"
make_target "$CACHE/test-branch"
make_target "$CACHE/orphan-old"
make_target "$CACHE/orphan-new"
touch -d '10 days ago' "$CACHE/main/file" "$CACHE/main/CACHEDIR.TAG" \
    "$CACHE/orphan-old/file" "$CACHE/orphan-old/CACHEDIR.TAG"

for reserved in seeds seeds-webfang quarantine quarantine-webfang; do
    make_target "$CACHE/$reserved"
    touch -d '10 days ago' "$CACHE/$reserved/file"
done

OUTPUT="$("$SWEEP" --repo "$REPO" --target-cache "$CACHE" --idle 72h)"
printf '%s\n' "$OUTPUT"

assert_json() {
    local pattern="$1"
    if ! grep -q "$pattern" <<<"$OUTPUT"; then
        echo "FAIL: expected report to contain: $pattern" >&2
        exit 1
    fi
}

assert_json '"scanned": 8'
assert_json '"reserved": 4'
assert_json '"live": 2'
assert_json '"protected": 1'
assert_json '"orphaned": 1'
assert_json '"orphan-old"'

if grep -q '"main"' <<<"$OUTPUT" || grep -q '"test-branch"' <<<"$OUTPUT"; then
    echo "FAIL: live targets were classified as orphans" >&2
    exit 1
fi
for reserved in seeds seeds-webfang quarantine quarantine-webfang; do
    if grep -q "\"$reserved\"" <<<"$OUTPUT"; then
        echo "FAIL: reserved namespace was classified as orphan: $reserved" >&2
        exit 1
    fi
done

if [[ ! -d "$CACHE/orphan-old" ]]; then
    echo "FAIL: dry-run deleted a target" >&2
    exit 1
fi

if "$SWEEP" --execute --repo "$REPO" --target-cache "$CACHE" >/dev/null 2>&1; then
    echo "FAIL: --execute without confirmation token was accepted" >&2
    exit 1
fi

if "$SWEEP" --repo "$TMP/does-not-exist" --target-cache "$CACHE" >/dev/null 2>&1; then
    echo "FAIL: missing repo was accepted (fail-open)" >&2
    exit 1
fi

echo "sweep harness: PASS"
