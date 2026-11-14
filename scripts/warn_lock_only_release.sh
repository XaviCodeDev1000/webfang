#!/usr/bin/env bash
#
# Warn when the window since the last tag moves `Cargo.lock` and no release was
# cut.
#
# WHY THIS EXISTS
#   With `release_commits` (release-plz.toml) a commit that touches ONLY
#   `Cargo.lock` reports `no commit matches` and does not release, even when its
#   subject matches: release-plz decides by the files of each package, and the
#   lockfile lives at the workspace root, outside every package. Measured on
#   release-plz 0.3.168 and 0.3.169 with a clean window (one lock-only commit on
#   top of v7.7.1): without the filter the same commit yields `next version is
#   7.7.2`, with it, nothing.
#
#   It is material because the repository's security patches are usually exactly
#   that shape (#2433 pulldown-cmark, #2361 lru). The failure is silent: the fix
#   sits on main, the changelog of the next release that fires for another
#   reason carries it, and nothing says that the security bump did not ship.
#
# WHAT IT DOES
#   Emits ONE non-blocking `::warning::` when ALL of these hold:
#     * release-plz did not open or update a Release PR in this run;
#     * `[workspace.package] version` is the same as the last tag's (no release
#       was cut);
#     * at least one commit since that tag touches `Cargo.lock`;
#     * that commit's subject matches `release_commits` (read from
#       release-plz.toml): when it does not, not releasing is the agreed policy
#       and silence is correct;
#     * none of those commits is covered by the `skip-lock-warning` label.
#
#   When the filter cannot be read (no release-plz.toml, no python3, an invalid
#   pattern) the step still warns, but it says the state is UNDETERMINED instead
#   of asserting that a release was skipped: a security bump must not be silenced
#   by a missing tool, and it must not be explained by a guess either.
#
#   It never fails the job: the release pipeline must not be blocked by a
#   warning about a convention.
#
# INPUTS (all optional — they exist so the flow can be exercised offline)
#   LOCK_WARN_REPO_ROOT    repository to read (default: this repo)
#   LOCK_WARN_PRS_CREATED  release-plz output: `true` when it opened/updated a
#                          Release PR (default: unknown)
#   LOCK_WARN_SKIP_LABEL   label that silences the warning (default:
#                          skip-lock-warning)
#
# EXIT: always 0.
set -euo pipefail

REPO_ROOT="${LOCK_WARN_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
PRS_CREATED="${LOCK_WARN_PRS_CREATED:-unknown}"
SKIP_LABEL="${LOCK_WARN_SKIP_LABEL:-skip-lock-warning}"

note() { printf '%s\n' "$*"; }

# The release PR is being regenerated: whatever the window holds rides it, and
# warning here would fire on every ordinary release.
if [[ "$PRS_CREATED" == "true" ]]; then
  note "lock-warning: release-plz opened or updated a Release PR; nothing to warn about."
  exit 0
fi

LAST_TAG="$(git -C "$REPO_ROOT" tag --list 'v[0-9]*' --sort=-v:refname | head -n 1)"
if [[ -z "$LAST_TAG" ]]; then
  note "lock-warning: no v* tag yet (initial release); nothing to warn about."
  exit 0
fi

workspace_version() {
  sed -n '/^\[workspace\.package\]/,/^\[/p' "$1" |
    sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1
}

VERSION_HEAD="$(workspace_version "$REPO_ROOT/Cargo.toml")"
VERSION_TAG="$(git -C "$REPO_ROOT" show "$LAST_TAG:Cargo.toml" | sed -n '/^\[workspace\.package\]/,/^\[/p' | sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"

if [[ -z "$VERSION_HEAD" || -z "$VERSION_TAG" ]]; then
  note "lock-warning: could not read [workspace.package] version; skipping the check."
  exit 0
fi

# A moved version means a release was cut for this window: the lock change is
# already on its way out.
if [[ "$VERSION_HEAD" != "$VERSION_TAG" ]]; then
  note "lock-warning: the workspace version moved ($VERSION_TAG -> $VERSION_HEAD); a release was cut."
  exit 0
fi

mapfile -t LOCK_COMMITS < <(git -C "$REPO_ROOT" log "$LAST_TAG..HEAD" --format=%H -- Cargo.lock)
if [[ ${#LOCK_COMMITS[@]} -eq 0 ]]; then
  note "lock-warning: no Cargo.lock change since $LAST_TAG; nothing to warn about."
  exit 0
fi

# The escape hatch is a label on the PR that carried the change. The mapping is
# the commit -> PR endpoint, not a search by message: a squash merge keeps the
# PR's subject, and a direct push has no PR at all (then the commit is not
# covered, which is the safe direction).
pr_labels() {
  local sha="$1" labels=""
  if ! command -v gh >/dev/null 2>&1; then
    return 0
  fi
  local repo="${GITHUB_REPOSITORY:-}"
  if [[ -z "$repo" ]]; then
    repo="$(git -C "$REPO_ROOT" remote get-url origin 2>/dev/null | sed -E 's#^.*[:/]([^/]+/[^/]+?)(\.git)?$#\1#' || true)"
  fi
  if [[ -z "$repo" ]]; then
    return 0
  fi
  labels="$(gh api "repos/$repo/commits/$sha/pulls" \
    --jq '[.[].labels[].name] | join(" ")' 2>/dev/null | head -n 1 || true)"
  printf '%s' "$labels"
}

# 0 = the subject matches release_commits, 1 = it does not (the filter does not
# release it by policy, so silence is correct), 2 = cannot be determined (then
# the warning stands: a security bump must not be silenced by a missing tool).
release_commits_matches() {
  local subject="$1"
  if ! command -v python3 >/dev/null 2>&1; then
    return 2
  fi
  python3 - "$REPO_ROOT/release-plz.toml" "$subject" <<'PY'
import re
import sys
import tomllib

cfg, subject = sys.argv[1], sys.argv[2]
try:
    with open(cfg, "rb") as handle:
        pattern = tomllib.load(handle).get("workspace", {}).get("release_commits")
except (OSError, tomllib.TOMLDecodeError):
    sys.exit(2)
if not pattern:
    sys.exit(2)
try:
    matched = re.search(pattern, subject) is not None
except re.error:
    sys.exit(2)
sys.exit(0 if matched else 1)
PY
}

UNCOVERED=()
INDETERMINATE=0
for sha in "${LOCK_COMMITS[@]}"; do
  labels="$(pr_labels "$sha")"
  if [[ " $labels " == *" $SKIP_LABEL "* ]]; then
    note "lock-warning: $(git -C "$REPO_ROOT" log -1 --format='%h %s' "$sha") is covered by '$SKIP_LABEL'."
    continue
  fi
  subject="$(git -C "$REPO_ROOT" log -1 --format=%s "$sha")"
  set +e
  release_commits_matches "$subject"
  matches=$?
  set -e
  if [[ $matches -eq 1 ]]; then
    # The filter does not release this type by policy: not releasing is the
    # agreed behaviour, not the hole this warning exists for.
    note "lock-warning: $(git -C "$REPO_ROOT" log -1 --format='%h %s' "$sha") does not match release_commits; the filter is doing its job."
    continue
  fi
  if [[ $matches -eq 2 ]]; then
    INDETERMINATE=1
  fi
  UNCOVERED+=("$(git -C "$REPO_ROOT" log -1 --format='%h %s' "$sha")")
done

if [[ ${#UNCOVERED[@]} -eq 0 ]]; then
  note "lock-warning: every lock-only commit in the window is covered by '$SKIP_LABEL'."
  exit 0
fi

if [[ $INDETERMINATE -eq 1 ]]; then
  # No regex, no python3, or an unreadable config: the state cannot be decided,
  # so the message must not claim the filter skipped a release.
  printf '::warning::%s\n' \
    "The window since $LAST_TAG moves Cargo.lock and the workspace version is still $VERSION_TAG, but it could not be determined whether these commits match release_commits (release-plz.toml or python3 unreadable), so whether this should have released is unknown. Commits: ${UNCOVERED[*]}. Check the filter manually before assuming the patch ships with the next release."
  exit 0
fi

printf '::warning::%s\n' \
  "The window since $LAST_TAG moves Cargo.lock and no release was cut (workspace version is still $VERSION_TAG). With release_commits, a commit that only touches Cargo.lock does not release, so this dependency patch will ship with the next release that fires for another reason. Commits: ${UNCOVERED[*]}. If it must ship now, touch a packaged file of the affected crate (a version requirement it owns, or a comment naming the advisory in its Cargo.toml). Silence it with the '$SKIP_LABEL' label if that is intended."