#!/usr/bin/env bash
# test_identity_gate.sh — fail-closed semantics harness for identity-gate.sh.
# All fixtures are ephemeral; no real worktree or cache dir is modified.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
GATE="$ROOT/scripts/githooks/identity-gate.sh"
POST_COMMIT="$ROOT/scripts/githooks/post-commit"
ISSUER="$ROOT/scripts/githooks/identity-attestation.sh"
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

run_post_commit() {
    (cd "$1" && "$POST_COMMIT" >/dev/null 2>&1)
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

assert_file_absent() {
    if [[ -e "$1" ]]; then
        echo "FAIL: expected absent: $1" >&2
        exit 1
    fi
}

assert_file_present() {
    if [[ ! -e "$1" ]]; then
        echo "FAIL: expected present: $1" >&2
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

# ---------------------------------------------------------------------------
# Identity attestation: scope, expiry, ancestry, and single-use consumption.
# The fixtures write the record directly: the interactive issuer is checked
# separately below for its TTY/token gates, which cannot be faked in a harness.
# ---------------------------------------------------------------------------

MISMATCH_WT="$TMP/mismatch-tree"
git -C "$REPO" worktree add -q "$MISMATCH_WT" -b feat/mismatch
MISMATCH_TARGET="$HOME/.cache/cargo-target/mismatch-tree"
mkdir -p "$MISMATCH_TARGET"
printf 'export CARGO_TARGET_DIR=%s\n' "$MISMATCH_TARGET" >"$MISMATCH_WT/.envrc"
(cd "$MISMATCH_WT" && "$HELPER" >/dev/null)
ATT="$(git -C "$MISMATCH_WT" rev-parse --absolute-git-dir)/webfang-identity-attestation"
REPO_ATT="$(git -C "$REPO" rev-parse --absolute-git-dir)/webfang-identity-attestation"
ANCHOR="$(git -C "$MISMATCH_WT" rev-parse HEAD)"
NOW="$(date +%s)"

write_attestation() {
    local file="$1" branch="$2" head="$3" expires="$4" tree="${5:-}"
    {
        printf 'version=1\n'
        printf 'branch=%s\n' "$branch"
        printf 'anchored_head=%s\n' "$head"
        printf 'evidence=issue 1495\n'
        printf 'expires_at=%s\n' "$expires"
        printf 'nonce=harness-nonce\n'
        if [[ -n "$tree" ]]; then
            printf 'tree=%s\n' "$tree"
        fi
    } >"$file"
}

valid_attestation() {
    write_attestation "$ATT" feat/mismatch "$ANCHOR" "$((NOW + 900))"
}

# A mismatch without an attestation stays denied.
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"

# A valid attestation unlocks ONLY the basename check.
valid_attestation
expect_code 0 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"

# Expired attestation is rejected.
write_attestation "$ATT" feat/mismatch "$ANCHOR" "$((NOW - 1))"
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"

# Anchor that is not even a commit is rejected.
write_attestation "$ATT" feat/mismatch 0000000000000000000000000000000000000000 "$((NOW + 900))"
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"

# A real commit that is not an ancestor is rejected (ancestry, not equality).
EMPTY_TREE="$(git -C "$REPO" hash-object -t tree /dev/null)"
STRAY="$(printf 'stray\n' | git -C "$REPO" -c user.email=test@example.com -c user.name=test commit-tree "$EMPTY_TREE")"
write_attestation "$ATT" feat/mismatch "$STRAY" "$((NOW + 900))"
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"

# Attestation naming a different branch is rejected.
write_attestation "$ATT" other/branch "$ANCHOR" "$((NOW + 900))"
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"

# The other guards are never skipped by a valid attestation.
valid_attestation
rm -f "$MISMATCH_WT/.worktree-identity"
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"
(cd "$MISMATCH_WT" && "$HELPER" >/dev/null)

valid_attestation
mv "$MISMATCH_WT/.envrc" "$TMP/mismatch.envrc"
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"
mv "$TMP/mismatch.envrc" "$MISMATCH_WT/.envrc"

valid_attestation
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MAIN_TARGET"

valid_attestation
git -C "$MISMATCH_WT" checkout --detach -q HEAD
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"
git -C "$MISMATCH_WT" symbolic-ref HEAD refs/heads/feat/mismatch

# The canonical main root is never excepted, even with a well-formed record.
write_attestation "$REPO_ATT" other "$ANCHOR" "$((NOW + 900))"
git -C "$REPO" symbolic-ref HEAD refs/heads/other
expect_code 1 run_gate "$REPO" CARGO_TARGET_DIR="$MAIN_TARGET"
git -C "$REPO" symbolic-ref HEAD refs/heads/main
rm -f "$REPO_ATT"

# A spent record left behind by a failed delete cannot authorize another commit.
valid_attestation
printf 'spent=1\n' >>"$ATT"
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"

# A worktree reached through a symlink binds through canonicalised paths on both sides.
SYMLINK_REAL="$TMP/symlink-real"
SYMLINK_WT="$TMP/link-tree"
mkdir -p "$SYMLINK_REAL"
ln -s "$SYMLINK_REAL" "$SYMLINK_WT"
git -C "$REPO" worktree add -q "$SYMLINK_WT" -b feat/symlink
SYMLINK_TARGET="$HOME/.cache/cargo-target/link-tree"
mkdir -p "$SYMLINK_TARGET"
printf 'export CARGO_TARGET_DIR=%s\n' "$SYMLINK_TARGET" >"$SYMLINK_REAL/.envrc"
(cd "$SYMLINK_WT" && "$HELPER" >/dev/null)
SYM_ATT="$(git -C "$SYMLINK_WT" rev-parse --absolute-git-dir)/webfang-identity-attestation"
write_attestation "$SYM_ATT" feat/symlink "$(git -C "$SYMLINK_WT" rev-parse HEAD)" "$((NOW + 900))"
expect_code 0 run_gate "$SYMLINK_WT" CARGO_TARGET_DIR="$SYMLINK_TARGET"

# post-commit consumes only a matching tree; a stale tree leaves it alive.
valid_attestation
expect_code 0 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"
BOUND="$(awk 'index($0,"tree=")==1 { sub("tree=",""); print; exit }' "$ATT")"
STAGED="$(git -C "$MISMATCH_WT" write-tree)"
[[ "$BOUND" == "$STAGED" ]] || { echo "FAIL: gate did not bind the staged tree" >&2; exit 1; }
write_attestation "$ATT" feat/mismatch "$ANCHOR" "$((NOW + 900))" "$STRAY"
run_post_commit "$MISMATCH_WT"
assert_file_present "$ATT"

# Matching tree: consumed after success, so an amend cannot reuse it.
valid_attestation
expect_code 0 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"
run_post_commit "$MISMATCH_WT"
assert_file_absent "$ATT"
expect_code 1 run_gate "$MISMATCH_WT" CARGO_TARGET_DIR="$MISMATCH_TARGET"

# End-to-end through git: a real commit consumes the record; an amend cannot reuse it.
HOOKS="$TMP/e2e/hooks"
LIBS="$TMP/e2e/lib"
mkdir -p "$HOOKS" "$LIBS"
cp "$GATE" "$POST_COMMIT" "$ROOT/scripts/githooks/pre-commit" "$HOOKS/"
cp "$ROOT/scripts/lib/worktree_paths.sh" "$ROOT/scripts/lib/identity_attestation.sh" "$LIBS/"
printf '#!/usr/bin/env bash\nexit 0\n' >"$HOOKS/review-gate.sh"
printf '#!/usr/bin/env bash\nexit 0\n' >"$HOOKS/static-guards.sh"
chmod +x "$HOOKS/"*
valid_attestation
CARGO_TARGET_DIR="$MISMATCH_TARGET" git -C "$MISMATCH_WT" \
    -c core.hooksPath="$HOOKS" -c user.email=t@e -c user.name=t \
    commit -q --allow-empty -m "attested e2e"
assert_file_absent "$ATT"
if CARGO_TARGET_DIR="$MISMATCH_TARGET" git -C "$MISMATCH_WT" \
    -c core.hooksPath="$HOOKS" -c user.email=t@e -c user.name=t \
    commit -q --allow-empty --amend -m "attested e2e amend" 2>/dev/null; then
    echo "FAIL: amend reused a consumed attestation" >&2
    exit 1
fi

# The owner issuer refuses a missing token and a non-interactive caller.
expect_code 1 env -u OWNER_IDENTITY_ATTEST_TOKEN "$ISSUER" \
    --branch feat/mismatch --evidence 'issue 1495' </dev/null
expect_code 1 env OWNER_IDENTITY_ATTEST_TOKEN=holder "$ISSUER" \
    --branch feat/mismatch --evidence 'issue 1495' </dev/null

echo "identity gate harness: PASS"
