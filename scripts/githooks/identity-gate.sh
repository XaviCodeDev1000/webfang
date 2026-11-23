#!/usr/bin/env bash
# identity-gate.sh — fail-closed worktree identity check for pre-commit.
#
# Runs BEFORE review-gate.sh, which stays fail-open for review verdicts.
# The main exception is anchored to the canonical main root AND branch main.
# Rebase/cherry-pick states are deferred explicitly, not inferred from a
# detached HEAD. A missing or mismatched .worktree-identity manifest is DENY:
# it is the second anchor for branches that normalise to the same directory
# name (a/b vs a-b).
#
# A directory/branch mismatch is denied unless a valid owner-issued attestation
# exists in the per-worktree git dir. That attestation replaces ONLY the basename
# proxy: manifest, .envrc target, main, detached HEAD and git-state checks are
# unchanged and still deny. post-commit consumes it after a successful commit.
#
# Exit 0 = identity verified; exit 1 = deny. Never fail open.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091 # resolved relative to SCRIPT_DIR at runtime
source "$SCRIPT_DIR/../lib/worktree_paths.sh"
# shellcheck disable=SC1091 # resolved relative to SCRIPT_DIR at runtime
source "$SCRIPT_DIR/../lib/identity_attestation.sh"

deny() {
    echo "identity-gate: DENY — $1" >&2
    exit 1
}

WORKTREE_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || deny "cannot resolve worktree root"
WORKTREE_CANON="$(wt_canon "$WORKTREE_ROOT")" || deny "cannot canonicalise worktree root"
GIT_DIR="$(git rev-parse --absolute-git-dir 2>/dev/null)" || deny "cannot resolve git dir"
COMMON_GIT="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || deny "cannot resolve common git dir"
MAIN_ROOT="$(wt_canon "$(dirname "$COMMON_GIT")")" || deny "cannot canonicalise main root"

# Explicitly named git states: let git manage its own commits here.
if [[ -d "$GIT_DIR/rebase-merge" || -d "$GIT_DIR/rebase-apply" || \
      -f "$GIT_DIR/CHERRY_PICK_HEAD" || -f "$GIT_DIR/REVERT_HEAD" ]]; then
    exit 0
fi

BRANCH="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"

if [[ "$WORKTREE_CANON" == "$MAIN_ROOT" ]]; then
    [[ "$BRANCH" == "main" ]] || deny "main checkout is on branch '${BRANCH:-detached}'"
    exit 0
fi

[[ -n "$BRANCH" ]] || deny "detached HEAD is not allowed for commits (candidate-view or stray checkout)"

TREE_NAME="$(basename "$WORKTREE_CANON")"
if [[ "$TREE_NAME" != "${BRANCH//\//-}" ]]; then
    ATTESTATION="$(ia_path 2>/dev/null)" || deny "cannot resolve attestation path"
    [[ -f "$ATTESTATION" ]] || deny "directory '$TREE_NAME' does not match branch '$BRANCH' (no attestation)"
    ia_validate "$ATTESTATION" "$BRANCH" "$WORKTREE_CANON" || deny "attestation rejected for branch '$BRANCH'"

    # Bind the exception to the exact staged tree. write-tree honours GIT_INDEX_FILE,
    # which git sets for partial commits (-a / --only), so the binding is the tree
    # that will actually be committed.
    STAGED_TREE="$(git write-tree 2>/dev/null)" || deny "cannot compute staged tree for attestation binding"
    ia_set_field "$ATTESTATION" tree "$STAGED_TREE" || deny "cannot bind attestation to staged tree"
    printf 'identity-gate: attestation accepted for %s (tree %s)\n' "$BRANCH" "$STAGED_TREE" >&2
fi

MANIFEST="$WORKTREE_ROOT/.worktree-identity"
[[ -f "$MANIFEST" ]] || deny "missing identity manifest: $MANIFEST (run scripts/worktree_identity.sh at bootstrap)"
RECORDED="$(sed -n '1p' "$MANIFEST" | tr -d '\r\n')"
[[ "$RECORDED" == "$BRANCH" ]] || deny "branch collision: manifest records '$RECORDED', HEAD is '$BRANCH'"

DECLARED="$(wt_envrc_target "$WORKTREE_ROOT/.envrc")" || deny "no CARGO_TARGET_DIR declaration in this tree's .envrc"
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
    deny "CARGO_TARGET_DIR is unset (direnv did not load). In non-interactive shells run: eval \"\$(direnv export bash)\""
fi
ACTUAL="$(wt_canon "$CARGO_TARGET_DIR")" || deny "cannot canonicalise CARGO_TARGET_DIR"
[[ "$ACTUAL" == "$DECLARED" ]] || deny "CARGO_TARGET_DIR points to $ACTUAL, this tree declares $DECLARED"

exit 0
