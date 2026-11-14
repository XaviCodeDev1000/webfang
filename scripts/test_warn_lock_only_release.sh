#!/usr/bin/env bash
# Semantics harness for scripts/warn_lock_only_release.sh.
#
# WHY THIS EXISTS
#   The warning is the only thing that says "this window moved Cargo.lock and no
#   release was cut". With `release_commits`, a commit that only touches the
#   lockfile does not release (measured on release-plz 0.3.168/0.3.169), and the
#   repository's security patches are usually exactly that shape (#2433, #2361).
#   A warning that fires when a release WAS cut is noise, and one that never
#   fires is the silent failure this step exists to remove.
#
# WHAT IT PINS
#   - a lock-only window with no release PR and no version bump -> ONE warning;
#   - a Release PR being regenerated (PRS_CREATED=true) -> silence;
#   - no Cargo.lock change in the window -> silence;
#   - a moved workspace version (a release was cut) -> silence;
#   - the `skip-lock-warning` label on the PR that carried the change -> silence;
#   - the step NEVER fails the job (it is a warning, not a gate).
#
# Offline: local bare repos as the remote, real clones, and a fake `gh` for the
# label escape. No network, no repo state touched.
set -euo pipefail

# The operator's global config must not leak into the fixtures (tag.gpgsign
# would make `git tag` reach for a passphrase).
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="scripts/warn_lock_only_release.sh"
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

write_cargo() {
  cat >"$1" <<EOF
[workspace]
members = []

[workspace.package]
version = "$2"
EOF
}

# The fixture carries a release-plz.toml with the same filter the repository
# uses, so the guard reads a real pattern (and the "does not match" branch is
# exercised, not the missing-config one).
write_release_plz() {
  cat >"$1" <<'EOF'
[workspace]
git_only = true
release_commits = '^(feat|fix|perf|revert|chore\(deps\))(\([^)]+\))?!?:|^[a-z]+(\([^)]+\))?!:'
EOF
}

# seed_repo <dir>: a bare origin with one commit at 1.0.0, tagged v1.0.0, and a
# clone of it. The clone is the repository the guard reads.
seed_repo() {
  local dir="$1"
  mkdir -p "$dir"
  git init --bare --quiet --initial-branch=main "$dir/origin.git"
  git -C "$dir/origin.git" symbolic-ref HEAD refs/heads/main
  git init --quiet --initial-branch=main "$dir/seed"
  git -C "$dir/seed" config user.email harness@example.com
  git -C "$dir/seed" config user.name harness
  write_cargo "$dir/seed/Cargo.toml" 1.0.0
  write_release_plz "$dir/seed/release-plz.toml"
  git -C "$dir/seed" add -A
  git -C "$dir/seed" commit --quiet -m "chore: release v1.0.0"
  git -C "$dir/seed" remote add origin "$dir/origin.git"
  git -C "$dir/seed" push --quiet origin main
  git -C "$dir/seed" tag v1.0.0
  git -C "$dir/seed" push --quiet origin refs/tags/v1.0.0
  git clone --quiet "$dir/origin.git" "$dir/clone"
  git -C "$dir/clone" config user.email harness@example.com
  git -C "$dir/clone" config user.name harness
}

# run_guard <clone> <prs_created> <logfile> [path-prefix]
run_guard() {
  local clone="$1" prs="$2" log="$3" prefix="${4:-}"
  set +e
  PATH="${prefix:+$prefix:}$PATH" \
    LOCK_WARN_REPO_ROOT="$clone" \
    LOCK_WARN_PRS_CREATED="$prs" \
    bash "$REPO_ROOT/$GUARD" >"$log" 2>&1
  RC=$?
  set -e
}

lock_commit() {
  printf '[[package]]\nname = "libc"\nversion = "%s"\n' "$2" >>"$1/Cargo.lock"
  git -C "$1" add -A
  git -C "$1" -c user.email=harness@example.com -c user.name=harness \
    -c commit.gpgsign=false commit --quiet -m "$3"
}

# ─── (a) a lock-only window with no release PR warns ─────────────────────────
D="$WORK/warn"
seed_repo "$D"
lock_commit "$D/clone" 0.2.191 "chore(deps): patch libc 0.2.186 -> 0.2.191 (RUSTSEC-2026-1)"
RC=0
run_guard "$D/clone" false "$WORK/a"
if [[ $RC -eq 0 ]] && grep -q '::warning::' "$WORK/a" &&
  grep -qF 'Cargo.lock' "$WORK/a" && grep -qF 'patch libc' "$WORK/a"; then
  pass "(a) a lock-only window with no release PR warns"
else
  fail "(a) a lock-only window with no release PR warns (rc=$RC: $(cat "$WORK/a"))"
fi

# ─── (b) a Release PR being regenerated is silent ────────────────────────────
D="$WORK/pr"
seed_repo "$D"
lock_commit "$D/clone" 0.2.191 "chore(deps): patch libc 0.2.186 -> 0.2.191 (RUSTSEC-2026-1)"
RC=0
run_guard "$D/clone" true "$WORK/b"
if [[ $RC -eq 0 ]] && ! grep -q '::warning::' "$WORK/b"; then
  pass "(b) a regenerated Release PR is silent"
else
  fail "(b) a regenerated Release PR is silent (rc=$RC: $(cat "$WORK/b"))"
fi

# ─── (c) no Cargo.lock change in the window is silent ────────────────────────
D="$WORK/nolock"
seed_repo "$D"
git -C "$D/clone" commit --quiet --allow-empty -m "docs: nothing"
RC=0
run_guard "$D/clone" false "$WORK/c"
if [[ $RC -eq 0 ]] && ! grep -q '::warning::' "$WORK/c"; then
  pass "(c) a window without a Cargo.lock change is silent"
else
  fail "(c) a window without a Cargo.lock change is silent (rc=$RC: $(cat "$WORK/c"))"
fi

# ─── (d) a moved version (a release was cut) is silent ───────────────────────
D="$WORK/moved"
seed_repo "$D"
lock_commit "$D/clone" 0.2.191 "chore(deps): patch libc (RUSTSEC-2026-1)"
write_cargo "$D/clone/Cargo.toml" 1.0.1
git -C "$D/clone" add -A
git -C "$D/clone" -c commit.gpgsign=false commit --quiet -m "chore: release v1.0.1"
RC=0
run_guard "$D/clone" false "$WORK/d"
if [[ $RC -eq 0 ]] && ! grep -q '::warning::' "$WORK/d"; then
  pass "(d) a moved workspace version is silent"
else
  fail "(d) a moved workspace version is silent (rc=$RC: $(cat "$WORK/d"))"
fi

# ─── (e) the skip label silences it ──────────────────────────────────────────
D="$WORK/skip"
seed_repo "$D"
lock_commit "$D/clone" 0.2.191 "chore(deps): patch libc (RUSTSEC-2026-1)"
mkdir -p "$WORK/bin"
printf 'skip-lock-warning\n' >"$WORK/labels"
cat >"$WORK/bin/gh" <<'FAKE'
#!/usr/bin/env bash
# Fixture stub: every `gh pr list` reports the same label set.
cat "$FAKE_LABELS" 2>/dev/null || true
FAKE
chmod +x "$WORK/bin/gh"
RC=0
FAKE_LABELS="$WORK/labels" run_guard "$D/clone" false "$WORK/e" "$WORK/bin"
if [[ $RC -eq 0 ]] && ! grep -q '::warning::' "$WORK/e" &&
  grep -qF "covered by 'skip-lock-warning'" "$WORK/e"; then
  pass "(e) the skip label silences the warning"
else
  fail "(e) the skip label silences the warning (rc=$RC: $(cat "$WORK/e"))"
fi

# ─── (f) the step never fails the job ────────────────────────────────────────
D="$WORK/neverfail"
seed_repo "$D"
lock_commit "$D/clone" 0.2.191 "chore(deps): patch libc (RUSTSEC-2026-1)"
RC=0
run_guard "$D/clone" unknown "$WORK/f"
if [[ $RC -eq 0 ]]; then
  pass "(f) an unknown PR state still exits 0"
else
  fail "(f) an unknown PR state still exits 0 (rc=$RC: $(cat "$WORK/f"))"
fi

# ─── (g) a lock-only commit the filter does not release by policy is silent ──
# The hole is "a commit that should have released and did not". A `test:` commit
# is not supposed to release, so warning about it would train the maintainer to
# ignore the warning.
D="$WORK/policy"
seed_repo "$D"
lock_commit "$D/clone" 0.2.191 "test: gate the resume tests (touches Cargo.lock)"
RC=0
run_guard "$D/clone" false "$WORK/g"
if [[ $RC -eq 0 ]] && ! grep -q '::warning::' "$WORK/g" &&
  grep -qF 'the filter is doing its job' "$WORK/g"; then
  pass "(g) a lock-only commit the filter does not release by policy is silent"
else
  fail "(g) a lock-only commit the filter does not release by policy is silent (rc=$RC: $(cat "$WORK/g"))"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]