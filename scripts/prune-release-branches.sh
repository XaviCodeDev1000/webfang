#!/usr/bin/env bash
#
# prune-release-branches.sh — delete `chore/release-*` branches whose PR is gone.
#
# WHY THIS EXISTS (webfang#2918)
#   release-plz REGENERATES the Release PR on every push to `main`, and every
#   regeneration creates a NEW branch with a fresh timestamp. The PR it
#   supersedes is left CLOSED and UNMERGED, and its branch is never deleted:
#   `delete_branch_on_merge: true` acts on merged PRs only. Measured 2026-11-18:
#   267 PRs with head `chore/release-*` = 62 merged (branch auto-deleted) + 205
#   closed unmerged, of which 191 branches were still alive out of 200 remote
#   branches.
#
#   This is a BACKSTOP, not the primary mechanism: the primary path is the merge
#   that deletes its own branch. This sweep only removes what that path
#   structurally cannot reach.
#
# THE PREDICATE — all three must hold
#   1. the name matches ^chore/release-<RFC3339 UTC>Z$ — strict on purpose.
#      `main`, `master`, `gh-pages`, `support/*`, `release/*` and `hotfix/*`
#      can never match, so no amount of predicate drift can make this delete a
#      branch a human owns. The regex IS the control; do not loosen it.
#   2. no OPEN PR has that head ref. This is the load-bearing one: deleting the
#      head branch of an open PR closes the PR. Note that `--merged` is NOT a
#      usable substitute — these branches are squash-merged, so their commits
#      are never ancestors of `main` and `--merged` would keep all 191.
#   3. the tip commit is older than MIN_AGE_HOURS (default 24). release-plz
#      pushes the branch and opens the PR in two separate steps; a sweep landing
#      inside that window would delete a branch whose PR is about to exist.
#
# FAIL-CLOSED, IN TWO PHASES
#   Phase 1 resolves the ENTIRE candidate list (branch listing, open PR heads,
#   per-branch detail). ANY failure there exits non-zero with ZERO deletions, so
#   a bad token or a rate-limited API cannot degrade into a partial delete.
#   Phase 2 deletes. A failed delete is reported, does not stop the rest, and
#   makes the run exit non-zero — the branch simply survives for the next run.
#
# Idempotent: no candidates is a no-op that exits 0.
#
# Usage:
#   scripts/prune-release-branches.sh [--dry-run] [--min-age-hours N] [--repo O/R]
#
# Environment:
#   GITHUB_TOKEN  required (contents:write to delete, pull-requests:read to list)
#   GITHUB_REPOSITORY  default for --repo
#   MIN_AGE_HOURS  default for --min-age-hours (default 24)
#   DRY_RUN  "true" for the same effect as --dry-run
#   GITHUB_STEP_SUMMARY  when set, a run summary is appended to that file

set -euo pipefail

BRANCH_RE='^chore/release-[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}-[0-9]{2}-[0-9]{2}Z$'

REPO="${WEBFANG_REPO:-${GITHUB_REPOSITORY:-}}"
MIN_AGE_HOURS="${MIN_AGE_HOURS:-24}"
DRY_RUN="${DRY_RUN:-false}"

usage() {
  cat <<'USAGE'
Usage: prune-release-branches.sh [options]

Deletes `chore/release-*` branches that have no open pull request and whose tip
is older than the minimum age. Deletes NOTHING when the branch or pull-request
listing cannot be read.

Options:
  --dry-run            print the candidates and exit 0 without deleting
  --min-age-hours N    minimum branch age in hours (default 24)
  --repo OWNER/REPO    repository to inspect (default $GITHUB_REPOSITORY)
  -h, --help           show this help
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=true ;;
    --min-age-hours)
      [ $# -ge 2 ] || { echo "error: --min-age-hours needs a value" >&2; exit 2; }
      MIN_AGE_HOURS="$2"
      shift
      ;;
    --min-age-hours=*) MIN_AGE_HOURS="${1#*=}" ;;
    --repo)
      [ $# -ge 2 ] || { echo "error: --repo needs a value" >&2; exit 2; }
      REPO="$2"
      shift
      ;;
    --repo=*) REPO="${1#*=}" ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [ -z "$REPO" ]; then
  echo "error: no repository given (use --repo or set GITHUB_REPOSITORY)" >&2
  exit 2
fi

case "$MIN_AGE_HOURS" in
  '' | *[!0-9]*)
    echo "error: --min-age-hours must be a non-negative integer, got '$MIN_AGE_HOURS'" >&2
    exit 2
    ;;
esac

MAX_AGE_SECONDS=$((MIN_AGE_HOURS * 3600))

# ─── Phase 1: resolve the candidate list ────────────────────────────────────
# Nothing is deleted until every read below has succeeded.

echo "== prune-release-branches: repo=$REPO min_age=${MIN_AGE_HOURS}h dry_run=$DRY_RUN"

if ! branches_raw="$(gh api "repos/$REPO/branches?per_page=100" --paginate --jq '.[].name')"; then
  echo "error: could not list branches for $REPO — fail-closed, nothing deleted" >&2
  exit 1
fi

# Open PR heads. `state=open` is deliberate: closed and merged PRs are
# irrelevant, and an open one is exactly what must protect its branch.
if ! open_heads_raw="$(gh api "repos/$REPO/pulls?state=open&per_page=100" --paginate --jq '.[].head.ref')"; then
  echo "error: could not list open pull requests for $REPO — fail-closed, nothing deleted" >&2
  exit 1
fi

candidates=()
protected_by_pr=0
too_young=0
checked=0

while IFS= read -r branch; do
  [ -n "$branch" ] || continue
  [[ "$branch" =~ $BRANCH_RE ]] || continue

  checked=$((checked + 1))

  if printf '%s\n' "$open_heads_raw" | grep -Fxq -- "$branch"; then
    echo "  keep $branch: an open pull request owns this head"
    protected_by_pr=$((protected_by_pr + 1))
    continue
  fi

  # One detail call per surviving branch, only to read the tip date. The date is
  # read through --jq so no local jq dependency is introduced.
  if ! committed_at="$(gh api "repos/$REPO/branches/$branch" --jq '.commit.commit.committer.date' 2>&1)"; then
    if printf '%s' "$committed_at" | grep -q 'HTTP 404'; then
      # The branch is already gone: another sweep, or a human. Not a failure.
      echo "  skip $branch: already gone (404)"
      continue
    fi
    echo "error: could not read branch $branch: $committed_at — fail-closed, nothing deleted" >&2
    exit 1
  fi

  if ! committed_epoch="$(date -u -d "$committed_at" +%s 2>/dev/null)"; then
    echo "error: unparseable committer date for $branch: '$committed_at' — fail-closed, nothing deleted" >&2
    exit 1
  fi

  age_seconds=$(( $(date -u +%s) - committed_epoch ))
  if [ "$age_seconds" -lt "$MAX_AGE_SECONDS" ]; then
    echo "  keep $branch: younger than ${MIN_AGE_HOURS}h (${age_seconds}s) — release-plz may be about to open its PR"
    too_young=$((too_young + 1))
    continue
  fi

  candidates+=("$branch")
done <<<"$branches_raw"

echo "== resolved: $checked release branch(es) checked, ${#candidates[@]} candidate(s), $protected_by_pr protected by an open PR, $too_young too young"

if [ "${#candidates[@]}" -eq 0 ]; then
  echo "== nothing to delete"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
      echo "### Release branch cleanup"
      echo
      echo "No orphaned \`chore/release-*\` branch. Checked $checked, $protected_by_pr protected by an open PR, $too_young under ${MIN_AGE_HOURS}h."
    } >>"$GITHUB_STEP_SUMMARY"
  fi
  exit 0
fi

if [ "$DRY_RUN" = true ]; then
  echo "== dry-run: would delete ${#candidates[@]} branch(es)"
  printf '  %s\n' "${candidates[@]}"
  exit 0
fi

# ─── Phase 2: delete ────────────────────────────────────────────────────────

deleted=0
failed=0

for branch in "${candidates[@]}"; do
  # A branch name contains slashes; the git refs API takes them verbatim in the
  # path, so the ref is NOT percent-encoded here.
  if ! delete_error="$(gh api --method DELETE "repos/$REPO/git/refs/heads/$branch" 2>&1)"; then
    if printf '%s' "$delete_error" | grep -q 'HTTP 404'; then
      # Already gone between phase 1 and phase 2 — another sweep, or a human.
      echo "  skip $branch: already gone (404)"
      deleted=$((deleted + 1))
      continue
    fi
    echo "error: could not delete $branch: $delete_error" >&2
    failed=$((failed + 1))
    continue
  fi
  echo "  deleted $branch"
  deleted=$((deleted + 1))
done

echo "== done: $deleted deleted, $failed failed"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Release branch cleanup"
    echo
    echo "| result | count |"
    echo "| --- | --- |"
    echo "| release branches checked | $checked |"
    echo "| protected by an open PR | $protected_by_pr |"
    echo "| under ${MIN_AGE_HOURS}h | $too_young |"
    echo "| deleted | $deleted |"
    echo "| failed | $failed |"
  } >>"$GITHUB_STEP_SUMMARY"
fi

[ "$failed" -eq 0 ] || exit 1