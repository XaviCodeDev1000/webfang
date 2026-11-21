#!/usr/bin/env bash
# worktree_identity.sh — write the per-worktree identity manifest.
# Run once at bootstrap, from inside the worktree, before the identity gate
# is active. The manifest records the exact (unnormalised) branch name so the
# gate can disambiguate a/b from a-b. It is local state and must stay ignored.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091 # resolved relative to SCRIPT_DIR at runtime
source "$SCRIPT_DIR/lib/worktree_paths.sh"

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "error: not inside a git worktree" >&2; exit 2; }
BRANCH="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
[[ -n "$BRANCH" ]] || { echo "error: detached HEAD cannot have an identity manifest" >&2; exit 2; }

printf '%s\n' "$BRANCH" >"$ROOT/.worktree-identity"
echo "identity manifest written for '$BRANCH' at $ROOT/.worktree-identity"
