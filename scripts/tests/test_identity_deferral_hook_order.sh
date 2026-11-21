#!/usr/bin/env bash
# test_identity_deferral_hook_order.sh — deferral cases and pre-commit ordering.

set -euo pipefail

SD="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SD/../.." && pwd)"
GATE="$ROOT/scripts/githooks/identity-gate.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export HOME="$TMP/home"
mkdir -p "$HOME"

R="$TMP/repo"
mkdir -p "$R"
git init -q -b main "$R"
git -C "$R" -c user.email=t@e -c user.name=t commit -q --allow-empty -m init
MAIN="$HOME/.cache/cargo-target/main"
mkdir -p "$MAIN"
printf 'export CARGO_TARGET_DIR=%s\n' "$MAIN" >"$R/.envrc"

run() {
    (cd "$1" && shift && env "$@" "$GATE" >/dev/null 2>&1)
}

run "$R" CARGO_TARGET_DIR="$MAIN"

git -C "$R" symbolic-ref HEAD refs/heads/other
! run "$R" CARGO_TARGET_DIR="$MAIN" || { echo "FAIL non-main-in-main" >&2; exit 1; }

mkdir -p "$R/.git/rebase-merge"
run "$R" CARGO_TARGET_DIR="$MAIN"
rmdir "$R/.git/rebase-merge"
: >"$R/.git/CHERRY_PICK_HEAD"
run "$R" CARGO_TARGET_DIR="$MAIN"
rm "$R/.git/CHERRY_PICK_HEAD"
: >"$R/.git/REVERT_HEAD"
run "$R" CARGO_TARGET_DIR="$MAIN"
rm "$R/.git/REVERT_HEAD"

git -C "$R" symbolic-ref HEAD refs/heads/main
mkdir -p "$TMP/hooks" "$TMP/lib"
cp "$ROOT/scripts/githooks/pre-commit" "$ROOT/scripts/githooks/identity-gate.sh" "$TMP/hooks/"
cp "$ROOT/scripts/lib/worktree_paths.sh" "$TMP/lib/"
printf '#!/usr/bin/env bash\n: > "%s/reached"\n' "$TMP" >"$TMP/hooks/review-gate.sh"
chmod +x "$TMP/hooks/"*

git -C "$R" symbolic-ref HEAD refs/heads/other
rm -f "$TMP/reached"
! git -C "$R" -c core.hooksPath="$TMP/hooks" -c user.email=t@e -c user.name=t commit -q --allow-empty -m blocked || {
    echo "FAIL blocked commit" >&2
    exit 1
}
[[ ! -e "$TMP/reached" ]] || { echo "FAIL review ran before identity" >&2; exit 1; }

git -C "$R" symbolic-ref HEAD refs/heads/main
git -C "$R" -c core.hooksPath="$TMP/hooks" -c user.email=t@e -c user.name=t commit -q --allow-empty -m allowed
[[ -e "$TMP/reached" ]] || { echo "FAIL review not reached" >&2; exit 1; }

echo "identity deferral/hook harness: PASS"
