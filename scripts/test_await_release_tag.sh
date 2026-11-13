#!/usr/bin/env bash
# Semantics harness for scripts/await_release_tag.sh.
#
# The guard exists because of ONE measured state: `main` ahead of its own tag
# while `release-plz release-pr` runs. Inside that state release-plz cannot tell
# "the version bump is merged and the tag is seconds away" from "the tag will
# never come", and both look identical to it — it proposes the version that is
# already on main (webfang#2768: #2768 proposed 7.7.0 while v7.7.0 was being
# pushed 25 s later by the sibling run).
#
# What matters here is behaviour, not structure:
#
#   (a) the tag is already there -> exit 0 at once, nothing awaited;
#   (b) the tag arrives during the wait -> exit 0, AND the tag is in the local
#       clone (the whole point: release-plz reads LOCAL tags);
#   (c) the tag never arrives -> non-zero, and the error names the phantom
#       instead of letting release-plz open it;
#   (d) no v* tag at all (initial release) -> exit 0 without waiting, because
#       there is no tag that could arrive;
#   (e) the clone is a PRE-merge checkout (the measured shape of run A) -> the
#       clone is fast-forwarded to origin/main, so release-plz reads the same
#       main the tag points at.
#
# Everything runs offline: local bare repos as the remote, real clones, no
# network, no GitHub, and no mutation of the repository this harness runs from.

set -euo pipefail

# The operator's global config must not leak into the fixtures. On a developer
# machine `tag.gpgsign = true` makes `git tag` reach for a passphrase (the
# harness hangs on a prompt instead of failing), and `init.defaultBranch` would
# change the fixture layout. Both are pinned here on purpose.
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="scripts/await_release_tag.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [[ ! -f "$REPO_ROOT/$GUARD" ]]; then
  printf '::error::the harness has no subject: %s is missing\n' "$GUARD" >&2
  printf '\nharness: 0 ok, 1 fail\n'
  exit 1
fi

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# write_cargo <path> <version>: the smallest Cargo.toml that carries the field
# the guard reads. `[workspace.package]` must come before the next `[` section,
# which is exactly the shape the sed range in the guard depends on.
write_cargo() {
  cat >"$1" <<EOF
[workspace]
members = []

[workspace.package]
version = "$2"
edition = "2021"

[workspace.dependencies]
EOF
}

# seed_repo <dir> <version> [tag-version|none]: a bare origin plus a clone, both
# with a commit that declares <version> in [workspace.package]. The tag defaults
# to <version>; pass an older version to reproduce the in-flight state (main is
# at 1.2.3, the newest tag is v1.2.2), or `none` for the initial release.
seed_repo() {
  local dir="$1" version="$2" tag_version="${3:-$2}"
  mkdir -p "$dir"
  git init --bare --quiet --initial-branch=main "$dir/origin.git"
  git -C "$dir/origin.git" symbolic-ref HEAD refs/heads/main
  git init --quiet --initial-branch=main "$dir/seed"
  git -C "$dir/seed" config user.email harness@example.com
  git -C "$dir/seed" config user.name harness
  write_cargo "$dir/seed/Cargo.toml" "$version"
  git -C "$dir/seed" add -A
  git -C "$dir/seed" commit --quiet -m "chore: release v$version"
  git -C "$dir/seed" remote add origin "$dir/origin.git"
  git -C "$dir/seed" push --quiet origin main
  if [[ "$tag_version" != "none" ]]; then
    git -C "$dir/seed" tag "v$tag_version"
    git -C "$dir/seed" push --quiet origin "refs/tags/v$tag_version"
  fi
  git clone --quiet "$dir/origin.git" "$dir/clone"
  git -C "$dir/clone" config user.email harness@example.com
  git -C "$dir/clone" config user.name harness
}

# run_guard <clone> <wait> <poll>: runs the guard against <clone>; sets RC and
# leaves the combined output in $WORK/out.
run_guard() {
  local clone="$1" wait="$2" poll="$3"
  RELEASE_TAG_REPO_ROOT="$clone" \
    RELEASE_TAG_REMOTE="origin" \
    RELEASE_TAG_WAIT_SECONDS="$wait" \
    RELEASE_TAG_POLL_SECONDS="$poll" \
    bash "$REPO_ROOT/$GUARD" >"$WORK/out" 2>&1 || RC=$?
  RC="${RC:-0}"
}

# ─── (a) the tag is already there: nothing to await ──────────────────────────
# NEGATION: pre-fix this path did not exist; the guard ran release-plz against
# the in-flight state unconditionally.
D="$WORK/tagged"
seed_repo "$D" 1.2.3
RC=0
run_guard "$D/clone" 5 1
if [[ $RC -eq 0 ]] && grep -qF 'v1.2.3 is tagged' "$WORK/out"; then
  pass "(a) an already tagged main is not awaited"
else
  fail "(a) an already tagged main is not awaited (rc=$RC: $(cat "$WORK/out"))"
fi

# ─── (b) the tag arrives while the guard waits ───────────────────────────────
# THE CASE THAT PRODUCED #2768: the tag lands ~28 s after the phantom was
# opened. The guard must wait for it AND leave it in the LOCAL clone, because
# release-plz resolves the last release from local tags.
D="$WORK/inflight"
seed_repo "$D" 1.2.3 1.2.2
# The sibling release runs detached, and its output goes to /dev/null: a
# background job that inherits the caller's stdout keeps the pipe open after
# this script exits, and a harness that holds the CI log pipe shut for minutes
# is worse than no harness.
(
  sleep 1
  git -C "$D/seed" tag v1.2.3
  git -C "$D/seed" push --quiet origin refs/tags/v1.2.3
) >/dev/null 2>&1 &
PUSHER=$!
RC=0
run_guard "$D/clone" 20 1
wait "$PUSHER" || true
if [[ $RC -eq 0 ]] &&
  grep -qF 'v1.2.3 arrived after' "$WORK/out" &&
  git -C "$D/clone" rev-parse -q --verify refs/tags/v1.2.3 >/dev/null 2>&1; then
  pass "(b) a tag arriving mid-wait is awaited and ends up in the clone"
else
  fail "(b) a tag arriving mid-wait is awaited and ends up in the clone (rc=$RC: $(cat "$WORK/out"))"
fi

# ─── (c) the tag never arrives: fail loudly, never open the phantom ──────────
# NEGATION: pre-fix the guard did not exist, so release-plz opened the phantom
# and the job went GREEN over it (#2768 sat open, mergeable, proposing 7.7.0).
D="$WORK/never"
seed_repo "$D" 1.2.3 1.2.2
RC=0
run_guard "$D/clone" 1 1
if [[ $RC -ne 0 ]] && grep -qF 'would be a phantom' "$WORK/out"; then
  pass "(c) a tag that never lands fails the step instead of opening a phantom"
else
  fail "(c) a tag that never lands fails the step instead of opening a phantom (rc=$RC: $(cat "$WORK/out"))"
fi

# ─── (d) no tag at all: the initial release must not wait ────────────────────
# A repository whose history has no v* tag has no "last release" to compare
# against; waiting there would delay the very first release for nothing.
D="$WORK/initial"
seed_repo "$D" 0.1.0 none
RC=0
run_guard "$D/clone" 1 1
if [[ $RC -eq 0 ]] && grep -qF 'initial release' "$WORK/out"; then
  pass "(d) the initial release is not awaited"
else
  fail "(d) the initial release is not awaited (rc=$RC: $(cat "$WORK/out"))"
fi

# ─── (e) a pre-merge checkout is synced to origin/main ───────────────────────
# The measured shape of run A: the clone is one commit behind (its tree is the
# pre-merge main), so release-plz reads a tree that no longer exists on origin.
# The guard must move the clone to the main the tag will point at.
D="$WORK/stale"
seed_repo "$D" 1.2.3
write_cargo "$D/seed/Cargo.toml" 1.2.4
git -C "$D/seed" add -A
git -C "$D/seed" commit --quiet -m "chore: release v1.2.4"
git -C "$D/seed" push --quiet origin main
git -C "$D/seed" tag v1.2.4
git -C "$D/seed" push --quiet origin refs/tags/v1.2.4
RC=0
run_guard "$D/clone" 5 1
if [[ $RC -eq 0 ]] &&
  [[ "$(git -C "$D/clone" rev-parse HEAD)" == "$(git -C "$D/origin.git" rev-parse main)" ]] &&
  grep -qF 'main is at 1.2.4' "$WORK/out"; then
  pass "(e) a pre-merge clone is fast-forwarded to the main the tag points at"
else
  fail "(e) a pre-merge clone is fast-forwarded to the main the tag points at (rc=$RC: $(cat "$WORK/out"))"
fi

# ─── (f) a tagged version with non-releasing commits on top is not awaited ───
# The release policy filters which commits may cut a version
# (release-plz.toml `release_commits`), so a `test:`/`ci:`/`refactor:` commit
# lands on main WITHOUT changing [workspace.package] version. The guard keys on
# the DECLARED version, not on HEAD: it must exit at once, because there is no
# tag missing.
# NEGATION: a guard that waits for a tag at HEAD would block until the timeout
# (and then fail) on every such commit, which is the normal state of main under
# the filter.
D="$WORK/nonreleasing"
seed_repo "$D" 1.2.3
git -C "$D/seed" commit --quiet --allow-empty -m "test: gate the resume tests"
git -C "$D/seed" commit --quiet --allow-empty -m "ci: install nasm"
git -C "$D/seed" push --quiet origin main
RC=0
run_guard "$D/clone" 1 1
if [[ $RC -eq 0 ]] &&
  grep -qF 'v1.2.3 is tagged' "$WORK/out" &&
  [[ "$(git -C "$D/clone" rev-parse HEAD)" == "$(git -C "$D/origin.git" rev-parse main)" ]]; then
  pass "(f) a tagged version with non-releasing commits on top is not awaited"
else
  fail "(f) a tagged version with non-releasing commits on top is not awaited (rc=$RC: $(cat "$WORK/out"))"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]