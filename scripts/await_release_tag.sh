#!/usr/bin/env bash
#
# Refuse to generate a Release PR against a main that is ahead of its own tag.
#
# WHY THIS EXISTS (webfang#2768)
#   `release-plz release-pr` derives "the next version" from the commits between
#   the newest v* tag and main. On 2026-11-12 two runs of this workflow
#   overlapped:
#
#     17:46:04  run A (push of #2753) starts `release-plz-pr`
#     17:46:23  #2767 (`chore: release v7.7.0`) merges -> main is at 7.7.0
#     17:46:30  run A's release-plz still reads its pre-merge checkout
#     17:46:33  it opens #2768: a Release PR for 7.7.0
#     17:46:58  run B (push of #2767) pushes the v7.7.0 tag
#
#   The PR job of run A had no way to know that main had moved under it and that
#   the tag was seconds away, so it "released" a version that was already on
#   main. The phantom carried no content: every file of the Release PR was
#   already on main, and its only conflict was three timestamps in
#   .github/support.json.
#
#   The intra-run ordering fix (webfang#1546, #1550: `needs: [release-plz-release]`)
#   does NOT cover this. Inside run A the jobs were already ordered; the race is
#   CROSS-RUN — the PR job of an older run overlapping the release job of a newer
#   one, each with its own checkout.
#
# WHAT THIS ENFORCES
#   Before `release-plz release-pr` runs, main must not be ahead of its tag.
#   That state means one of two things:
#
#     * a release is in flight — the version bump is merged and the sibling run
#       is pushing the tag. Normal, seconds wide;
#     * a release is broken — the tag never came. Nothing useful can be
#       generated from an unreleased main, and a phantom is exactly what the
#       next run would produce.
#
#   So this step (1) syncs the checkout to origin/main — a pre-merge checkout is
#   what made run A propose a version that was already on main — and (2) waits a
#   bounded time for the tag. When it arrives, release-plz sees
#   `last tag == main` and correctly proposes nothing. When it does not, the step
#   fails loudly instead of opening a phantom.
#
# INPUTS (all optional — they exist so the flow can be exercised offline)
#   RELEASE_TAG_REPO_ROOT     repository to work in (default: this repo)
#   RELEASE_TAG_REMOTE        remote to read main and the tags from (default: origin)
#   RELEASE_TAG_WAIT_SECONDS  how long to wait for the tag (default: 120)
#   RELEASE_TAG_POLL_SECONDS  delay between polls (default: 5)
#
# EXIT: 0 main is tagged, or there is nothing to tag · 1 main is ahead of its
#       tag and the tag did not arrive (with `::error::` on stderr)
set -euo pipefail

REPO_ROOT="${RELEASE_TAG_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
REMOTE="${RELEASE_TAG_REMOTE:-origin}"
WAIT_SECONDS="${RELEASE_TAG_WAIT_SECONDS:-120}"
POLL_SECONDS="${RELEASE_TAG_POLL_SECONDS:-5}"

note() { printf '::notice::%s\n' "$*"; }
warn() { printf '::warning::%s\n' "$*" >&2; }
err() { printf '::error::%s\n' "$*" >&2; exit 1; }

# ─── helpers ─────────────────────────────────────────────────────────────────

# workspace_version: the one version the workspace releases under. Read from
# `[workspace.package]` of Cargo.toml — the same file the release bump writes —
# so this cannot drift from what release-plz would bump.
workspace_version() {
  sed -n '/^\[workspace\.package\]/,/^\[/p' "$REPO_ROOT/Cargo.toml" |
    sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1
}

tag_exists_local() {
  git -C "$REPO_ROOT" rev-parse -q --verify "refs/tags/v$1" >/dev/null 2>&1
}

# ls-remote, not `git tag`: the tag is pushed by a SIBLING RUN, so the local
# clone is exactly as stale as the checkout the phantom came from. The remote is
# the only place the fresh truth lives.
tag_exists_remote() {
  [[ -n "$(git -C "$REPO_ROOT" ls-remote --tags "$REMOTE" "refs/tags/v$1" 2>/dev/null)" ]]
}

any_tag_exists() {
  [[ -n "$(git -C "$REPO_ROOT" tag --list 'v*' 2>/dev/null)" ]]
}

# ─── 1. the checkout must be the main the tag will point at ──────────────────
#
# actions/checkout runs once, at the start of the job; release-plz runs seconds
# later. In the measured race the checkout was the PRE-merge main (7.6.6) while
# origin/main was already the release commit (7.7.0): release-plz merged a stale
# tree with a live PR query and found neither the tag nor the open Release PR it
# should have updated.
if ! git -C "$REPO_ROOT" fetch --tags --force "$REMOTE" >/dev/null 2>&1; then
  err "could not fetch tags from '$REMOTE' in $REPO_ROOT - refusing to decide the release state from a clone whose tag list cannot be refreshed."
fi
if ! git -C "$REPO_ROOT" merge --ff-only --quiet "refs/remotes/$REMOTE/main" >/dev/null 2>&1; then
  err "the checkout could not fast-forward to '$REMOTE/main' - the clone diverged from the remote. Refusing to generate a Release PR from a main this job cannot prove."
fi

VERSION="$(workspace_version)"
[[ -n "$VERSION" ]] || err "could not read [workspace.package] version from $REPO_ROOT/Cargo.toml."

if tag_exists_local "$VERSION"; then
  note "main is at $VERSION and v$VERSION is tagged - nothing to await."
  exit 0
fi

# A repository with no v* tag at all is the initial release: release-plz must
# propose the first version, and there is no tag that could arrive.
if ! any_tag_exists; then
  note "main is at $VERSION and the repository has no v* tag yet (initial release) - nothing to await."
  exit 0
fi

# ─── 2. wait for the tag a sibling run is about to push ──────────────────────
#
# Bounded on purpose. The sibling release job resolves in seconds (the measured
# window was 28 s), and a wait that never ends would turn a broken release into
# a job that hangs instead of failing.
START=$SECONDS
DEADLINE=$((START + WAIT_SECONDS))
while :; do
  if tag_exists_remote "$VERSION"; then
    if git -C "$REPO_ROOT" fetch --tags --force "$REMOTE" >/dev/null 2>&1 &&
      tag_exists_local "$VERSION"; then
      note "v$VERSION arrived after $((SECONDS - START))s: main is tagged again, so release-plz sees no unreleased commit and proposes nothing."
      exit 0
    fi
    warn "v$VERSION exists on '$REMOTE' but could not be fetched into the clone."
  fi

  if ((SECONDS >= DEADLINE)); then
    break
  fi
  sleep "$POLL_SECONDS"
done

err "main is at $VERSION but v$VERSION does not exist on '$REMOTE' after ${WAIT_SECONDS}s. A Release PR for $VERSION would be a phantom: the version bump is already on main, so the PR would carry no content (webfang#2768). Fix the release (the tag never landed) instead of merging a phantom."