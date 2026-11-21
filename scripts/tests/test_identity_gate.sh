#!/usr/bin/env bash
# test_identity_gate.sh — fail-closed semantics harness for identity-gate.sh.
# All fixtures are ephemeral; no real worktree or cache dir is modified.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GATE="$ROOT/scripts/githooks/identity-gate.sh"
HELPER="$ROOT/scripts/worktree_identity.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

export HOME="$TMP/home"
mkdir -p "$HOME"
REPO="$TMP/repo"
WT="$TMP/test-branch"

git init -q -b main "$REPO"
git -C "$REPO" -c user.email=test@example.com -c user.name=test commit -q --allow-empty -m init
mkdir -p "$HOME/.cache/cargo-target/main"
printf 'export CARGO_TARGET_DIR=%s/.cache/cargo-target/main\n' "$HOME" >"$REPO/.envrc"
(cd "$REPO" && "$HELPER" >/dev/null)

git -C "$REPO" worktree add -q "$WT" -b test/branch
TARGET="$HOME/.cache/cargo-target/test-branch"
mkdir -p "$TARGET"
printf 'export CARGO_TARGET_DIR=%s\n' "$TARGET" >"$WT/.envrc"
(cd "$WT" && "$HELPER" >/dev/null)

run_gate() {
    local dir="$1"
    shift
    (cd "$dir" && env "$@" "$GATE" >/dev/null 2>&1)
}

expect_code() {
    local expected="$1"
    shift
    local actual=0
    "$@" || actual=$?
    if [[ "$actual" -ne "$expected" ]]; then
        echo "FAIL: expected exit $expected, got $actual: $*" >&2
        exit 1
    fi
}

MAIN_TARGET="$HOME/.cache/cargo-target/main"
expect_code 0 run_gate "$REPO" CARGO_TARGET_DIR="$MAIN_TARGET"
expect_code 0 run_gate "$WT" CARGO_TARGET_DIR="$TARGET"

rm -f "$WT/.worktree-identity"
expect_code 1 run_gate "$WT" CARGO_TARGET_DIR="$TARGET"

printf 'other/branch\n' >"$WT/.worktree-identity"
expect_code 1 run_gate "$WT" CARGO_TARGET_DIR="$TARGET"

(cd "$WT" && "$HELPER" >/dev/null)
if (cd "$WT" && env -u CARGO_TARGET_DIR "$GATE" >/dev/null 2>&1); then
    echo "FAIL: unset CARGO_TARGET_DIR was accepted" >&2
    exit 1
fi

# Silent inheritance of main's target must be denied in a feature worktree.
expect_code 1 run_gate "$WT" CARGO_TARGET_DIR="$MAIN_TARGET"

# A checkout of a non-main branch in the main directory is not a valid exception.
git -C "$REPO" symbolic-ref HEAD refs/heads/other
expect_code 1 run_gate "$REPO" CARGO_TARGET_DIR="$MAIN_TARGET"

# Detached HEAD (candidate-view) is denied. This checkout acts on the temp
# fixture only, never on a session worktree.
git -C "$REPO" symbolic-ref HEAD refs/heads/main
git -C "$WT" checkout --detach -q HEAD
expect_code 1 run_gate "$WT" CARGO_TARGET_DIR="$TARGET"

echo "identity gate harness: PASS"
