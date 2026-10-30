#!/usr/bin/env bash
# Semantics harness for scripts/declare_pending_publication.sh (issue #2144).
#
# The script declares the PENDING PUBLICATION of a release in
# .github/support.json: `latest` is a FACT about what this repository
# published, and the enforcing gate
# (`scripts/check_support_drift.sh --scope=published`, a REQUIRED check) turns
# red for EVERY pull request when the declared `latest` lags behind the newest
# published patch. The gate tolerates exactly one narrow state — a declaration
# ONE patch ahead of the newest published patch of its own minor, with no tag
# of its own yet: the in-flight window of a sanctioned release.
#
# The support patch flow already produces that state. The release-plz path
# declared NOTHING: the tag was published with the declaration still behind it
# and the whole repository went red (measured on v4.0.1). This script is the
# release-plz path's half of that contract, and it has two modes with two
# different contracts:
#
#   WRITER (--write)  declares the pending publication of the CURRENT
#                     checkout's workspace version, through the SINGLE WRITER of
#                     the `latest` values (scripts/refresh_support_latest.sh).
#                     It never commits, never pushes and never invents a line.
#   ORCHESTRATOR      finds the open Release PR, declares the publication in a
#                     throwaway worktree of that branch, and commits and
#                     pushes it there.
#
# What this harness pins, and why each one is a real failure mode:
#
#   (a) the writer declares the publication of a DECLARED line, and changes
#       NOTHING but the `latest` value — the declaration is a fact about one
#       field, and a rewrite that reformats or reorders the file would make
#       every later diff unreadable;
#   (b) a PATCH release on a line that is NOT declared is NOT invented:
#       declaring a new line is a governance decision (an issue plus the
#       maintainer), so the writer warns, names the version and exits 0 — a
#       release is never blocked by a support governance matter;
#   (b') a MINOR release on a line that is NOT declared ROTATES the support
#       window: the Release PR is the only place where the new window can be
#       declared, because the tag it is about to publish does not exist yet
#       (#2531, option A);
#   (b'') a rotation that CANNOT be done (a published minor that was never
#       declared) fails the writer instead of skipping it: the release must
#       not publish a tag without its declaration;
#   (c) the writer is idempotent: the release-plz job re-runs on every push to
#       `main` (release-plz regenerates the Release PR branch every time), so
#       a writer that rewrites the file every run would produce a commit per
#       push and wake the whole CI pipeline for nothing;
#   (d) a version that is not X.Y.Z is refused, not guessed — a declaration of
#       "4.0" would be a fact nobody published;
#   (e) the orchestrator declares the publication IN THE RELEASE BRANCH, with
#       the bot identity, and pushes it — a declaration that lands on the
#       wrong branch, or with the wrong identity, or nowhere, is invisible from
#       the outside and turns the gate red after the tag is published;
#   (f) an open pull request list with no Release PR is a no-op with exit 0:
#       release-plz decided there is no release to prepare, and failing here
#       would red every push to `main` for nothing.
#
# Everything runs offline: mini repos in a temp dir, a local bare repo as the
# remote (a path, not a URL — the script treats a local remote as
# credential-free), a real branch, a real worktree, and a fake `gh` on PATH
# that answers exactly the two calls the script makes. No network, no GitHub,
# and no mutation of the repository this harness runs from.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

SUBJECT="scripts/declare_pending_publication.sh"
# The writer delegates to the single writer of the `latest` values, and that
# writer delegates the rendered projection to its own renderer. All three have
# to exist in a fixture for the writer to be exercisable at all.
DEPS=(
  "scripts/refresh_support_latest.sh"
  "scripts/render-support-md.sh"
  # The MINOR path rotates the support window through
  # scripts/rotate-stable.sh, which EOLs the third line through
  # scripts/eol-line.sh. Both have to exist in the fixture for the rotation
  # to be exercisable at all.
  "scripts/rotate-stable.sh"
  "scripts/eol-line.sh"
)

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

if ! command -v jq >/dev/null 2>&1; then
  echo "::error::this harness needs jq to emulate 'gh --jq' and to read the declaration" >&2
  exit 1
fi
for f in "$SUBJECT" "${DEPS[@]}"; do
  if [[ ! -f "$REPO_ROOT/$f" ]]; then
    echo "::error::the harness has no subject: $f is missing" >&2
    printf '\nharness: 0 ok, 1 fail\n'
    exit 1
  fi
done

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves the two reads the orchestrator performs and nothing else: the list of
# open pull requests and the state of one pull request. An unhandled
# invocation exits 99, so a change in the orchestrator's gh usage fails loudly
# instead of silently reading a stale fixture.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_GH_STATE"

# gh exposes ONLY the fields requested with --json to the --jq expression. A
# field that was not requested evaluates to null and gh does NOT error — it
# just yields nothing. Emulating that keeps this harness honest about the
# field set the orchestrator asks for: a regression that stops requesting a
# field it reads has to fail here, not in production.
expose_requested() {
  local json="$1" fields="$2"
  local requested
  requested="$(printf '%s' "$fields" | tr ',' '\n' | jq -Rsc 'split("\n") | map(select(length > 0))')"
  printf '%s' "$json" | jq -c --argjson req "$requested" \
    'if type == "array"
     then [.[] | with_entries(select(.key as $k | $req | index($k)))]
     else with_entries(select(.key as $k | $req | index($k))) end'
}

if [[ "${1:-}" != "pr" ]]; then
  echo "fake gh: unhandled invocation: $*" >&2
  exit 99
fi
sub="${2:-}"
shift 2

pr_number=""
if [[ $# -gt 0 && "${1:-}" != --* ]]; then pr_number="$1"; shift; fi
fields=""; jqexpr=""; state_filter=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --json) shift; fields="${1:-}" ;;
    --jq) shift; jqexpr="${1:-}" ;;
    --state) shift; state_filter="${1:-}" ;;
    *) echo "fake gh: unhandled flag: $1" >&2; exit 99 ;;
  esac
  shift
done

[[ -f "$state/prs.json" ]] || { echo "fake gh: missing prs.json fixture" >&2; exit 99; }
json="$(cat "$state/prs.json")"

case "$sub" in
  list)
    # 'gh pr list --state X' returns only the pull requests in that state.
    # GitHub reports the state in upper case ('OPEN'); the flag is lower case.
    if [[ -n $state_filter ]]; then
      json="$(printf '%s' "$json" | jq -c --arg s "$state_filter" \
        '[.[] | select((.state | ascii_downcase) == ($s | ascii_downcase))]')"
    fi
    if [[ -n $jqexpr ]]; then
      printf '%s' "$(expose_requested "$json" "$fields")" | jq -r "$jqexpr"
    else
      printf '%s\n' "$json"
    fi
    ;;
  view)
    pr_state="$(jq -r --argjson n "${pr_number:-0}" \
      '[.[] | select(.number == $n)][0].state // "OPEN"' "$state/prs.json")"
    if [[ -n $jqexpr ]]; then
      printf '%s' "$(expose_requested "{\"number\":${pr_number:-0},\"state\":\"$pr_state\"}" "$fields")" | jq -r "$jqexpr"
    else
      printf '%s\n' "$pr_state"
    fi
    ;;
  *) echo "fake gh: unhandled pr $sub" >&2; exit 99 ;;
esac
FAKE
chmod +x "$BIN/gh"

# ─── Fixtures ───────────────────────────────────────────────────────────────
# write_cargo <dir> <version>: a workspace whose [workspace.package] version is
# the one under test. The writer reads it with the same sed discipline
# scripts/apply_release_pins.sh uses, so the shape here is the real one.
write_cargo() {
  local dir="$1" version="$2"
  cat > "$dir/Cargo.toml" <<EOF
[workspace]
members = []

[workspace.package]
version = "$version"
edition = "2021"
EOF
}

# write_support_json <dir> <minor> <latest>: a declaration with ONE line. The
# file is produced by jq, so it has the exact canonical shape the single
# writer rewrites it into — a fixture with a different shape would make the
# "nothing else changed" assertion measure the fixture, not the writer.
write_support_json() {
  local dir="$1" minor="$2" latest="$3"
  mkdir -p "$dir/.github"
  jq -n --arg m "$minor" --arg l "$latest" \
    '{schema_version:1, lines:[{minor:$m, state:"STABLE", branch:null, latest:$l, released_at:"2026-01-01T00:00:00+00:00", accepts:["bugfix","security"]}]}' \
    > "$dir/.github/support.json"
}

# install_scripts <dir>: the script under test plus the two scripts it
# delegates to. The writer resolves them relative to the checkout it runs in,
# so a fixture without them cannot exercise the real path.
install_scripts() {
  local dir="$1"
  mkdir -p "$dir/scripts"
  cp "$REPO_ROOT/$SUBJECT" "$dir/scripts/"
  for dep in "${DEPS[@]}"; do cp "$REPO_ROOT/$dep" "$dir/scripts/"; done
  chmod +x "$dir/scripts/"*.sh
}

# setup_writer_repo <name> <version> <minor> <latest>: a mini repo whose
# checkout IS the subject of the declaration.
setup_writer_repo() {
  local name="$1" version="$2" minor="$3" latest="$4"
  local dir="$WORK/$name"

  mkdir -p "$dir"
  install_scripts "$dir"
  write_cargo "$dir" "$version"
  write_support_json "$dir" "$minor" "$latest"

  git init -q -b main "$dir"
  git -C "$dir" config user.email "seed@example.com"
  git -C "$dir" config user.name "seed"
  git -C "$dir" config commit.gpgsign false
  # `tag.gpgsign` is off on purpose: on this workstation the global config
  # signs tags, and a signed tag opens an editor to compose the message —
  # a harness that hangs on somebody's machine is a harness that gets
  # deleted instead of run.
  git -C "$dir" config tag.gpgsign false
  # The rendered projection is the collaborator of the single writer: it has to
  # exist and be in sync before the run, or the "nothing changed" assertion
  # would measure a stale render instead of the declaration.
  ( cd "$dir" && bash scripts/render-support-md.sh >/dev/null )
  git -C "$dir" add -A
  git -C "$dir" commit -q -m "seed: fixture"

  printf '%s\n' "$dir"
}

# setup_release_repo <name> <main-version> <branch-version> <minor> <latest>:
# a bare remote plus a clone whose `chore/release-*` branch carries a BUMPED
# workspace version — the exact state a Release PR is in right after
# release-plz regenerates it. The branch is built in a throwaway worktree, so
# the clone's own branch is never switched.
setup_release_repo() {
  local name="$1" main_version="$2" branch_version="$3" minor="$4" latest="$5"
  local dir="$WORK/$name"

  mkdir -p "$dir/clone"
  install_scripts "$dir/clone"
  write_cargo "$dir/clone" "$main_version"
  write_support_json "$dir/clone" "$minor" "$latest"

  git init -q -b main "$dir/clone"
  git -C "$dir/clone" config user.email "seed@example.com"
  git -C "$dir/clone" config user.name "seed"
  git -C "$dir/clone" config commit.gpgsign false
  git -C "$dir/clone" config tag.gpgsign false
  ( cd "$dir/clone" && bash scripts/render-support-md.sh >/dev/null )
  git -C "$dir/clone" add -A
  git -C "$dir/clone" commit -q -m "seed: main without the release bump"

  git init -q --bare "$dir/remote.git"
  git -C "$dir/clone" remote add origin "$dir/remote.git"
  git -C "$dir/clone" push -q origin main

  # The release branch: same tree plus the version bump release-plz makes.
  git -C "$dir/clone" worktree add -q -b "chore/release-$branch_version" "$dir/branch-wt"
  write_cargo "$dir/branch-wt" "$branch_version"
  git -C "$dir/branch-wt" add -A
  git -C "$dir/branch-wt" commit -q -m "chore: release v$branch_version"
  git -C "$dir/branch-wt" push -q origin "chore/release-$branch_version"
  git -C "$dir/clone" worktree remove --force "$dir/branch-wt"

  printf '%s\n' "$dir"
}

# write_prs <json>: the fixture the fake gh serves for `pr list`.
write_prs() {
  printf '%s\n' "$1" > "$STATE/prs.json"
}

# run_writer <dir> [args...]: the WRITER, offline, against the fixture.
run_writer() {
  local dir="$1"; shift
  local out rc=0
  out="$( cd "$dir" && bash "$dir/scripts/declare_pending_publication.sh" "$@" 2>&1 )" || rc=$?
  LAST_RC=$rc
  LAST_OUT="$out"
}

# run_orchestrator <clone-dir> [token]: the ORCHESTRATOR, offline, against the
# fixture. The token is the credential the real flow uses (the App token); it
# is passed empty to exercise the "no credential" path.
run_orchestrator() {
  local dir="$1" token="${2:-}"
  local out rc=0
  out="$( cd "$dir/clone" && PATH="$BIN:$PATH" \
    FAKE_GH_STATE="$STATE" \
    DECLARE_PENDING_REPO_ROOT="$dir/clone" \
    DECLARE_PENDING_BRANCH_PREFIX="chore/release-" \
    DECLARE_PENDING_REMOTE="origin" \
    GITHUB_TOKEN="$token" \
    bash "$dir/clone/scripts/declare_pending_publication.sh" 2>&1 )" || rc=$?
  LAST_RC=$rc
  LAST_OUT="$out"
}

# remote_branch_sha <dir> <branch>: the head of a branch on the bare remote.
remote_branch_sha() {
  git -C "$1/remote.git" rev-parse "refs/heads/$2" 2>/dev/null || echo ""
}

# remote_commit_count <dir> <branch>: how many commits the branch carries.
remote_commit_count() {
  git -C "$1/remote.git" rev-list --count "refs/heads/$2" 2>/dev/null || echo 0
}

# strip_latest <file>: the file with the `latest` value normalized, for a
# BYTE-level comparison of everything that is not the declared value.
strip_latest() {
  sed 's/"latest": "[^"]*"/"latest": "<VERSION>"/' "$1"
}

# ─── (a) the writer declares a DECLARED line ────────────────────────────────
# The whole point of the flow: the release-plz path declared nothing, so the
# tag shipped and the declaration lagged behind it, and the enforcing gate
# turned red for EVERY pull request in the repository.
# NEGATION: pre-fix the release-plz path wrote no declaration at all, so this
# row is the behaviour that was missing, not a refinement of it.
D="$(setup_writer_repo writer-declared 4.0.1 4.0 4.0.0)"
cp "$D/.github/support.json" "$WORK/before.json"
run_writer "$D" --write
if [[ $LAST_RC -ne 0 ]]; then
  fail "(a) the writer exits 0 on a declared line (rc=$LAST_RC: $LAST_OUT)"
else
  pass "(a) the writer exits 0 on a declared line"
  if [[ "$(jq -r '.lines[] | select(.minor == "4.0") | .latest' "$D/.github/support.json")" == "4.0.1" ]]; then
    pass "(a) the declared line now declares the pending publication 4.0.1"
  else
    fail "(a) the declared line now declares the pending publication 4.0.1 ($(jq -r '.lines[] | select(.minor == "4.0") | .latest' "$D/.github/support.json"))"
  fi
  # ONLY the `latest` value may change. The rest of the file is compared
  # BYTE for byte: a rewrite that reformats or reorders the declaration
  # would make every later diff of it unreadable.
  if cmp -s <(strip_latest "$WORK/before.json") <(strip_latest "$D/.github/support.json"); then
    pass "(a) nothing but the 'latest' value changed in the declaration"
  else
    fail "(a) nothing but the 'latest' value changed in the declaration"
  fi
  # `diff` exits 1 when the files differ, so its output is captured first:
  # under `pipefail` a `diff | grep` pipeline would report the diff's status.
  DIFF_OUT="$(diff -u "$WORK/before.json" "$D/.github/support.json" 2>/dev/null || true)"
  CHANGED="$(printf '%s\n' "$DIFF_OUT" | grep -cE '^[+-][^+-]' || true)"
  if [[ "$CHANGED" == "2" ]] \
    && grep -qE '^-.*"latest": "4\.0\.0",?$' <<< "$DIFF_OUT" \
    && grep -qE '^\+.*"latest": "4\.0\.1",?$' <<< "$DIFF_OUT"; then
    pass "(a) the declaration moved exactly one line: the 'latest' value"
  else
    fail "(a) the declaration moved exactly one line: the 'latest' value ($CHANGED changed lines)"
  fi
  if grep -qF "declared 4.0=4.0.1" <<< "$LAST_OUT"; then
    pass "(a) the writer names the declaration it made"
  else
    fail "(a) the writer names the declaration it made ($LAST_OUT)"
  fi
fi

# ─── (b) a PATCH release on a line that is NOT declared is not invented ─────
# Declaring a new line is a governance decision (an issue plus the
# maintainer — AGENTS.md "Agent routing"), so the writer warns, names the
# version and exits 0 without touching the declaration. This path is NOT
# touched by the minor rotation (#2531): a patch release does not open a new
# support line.
# NEGATION: pre-fix nothing declared a publication at all, so the boundary
# between "declared" and "not declared" did not exist yet.
D="$(setup_writer_repo writer-undeclared 4.1.1 4.0 4.0.0)"
cp "$D/.github/support.json" "$WORK/before-undeclared.json"
run_writer "$D" --write
if [[ $LAST_RC -ne 0 ]]; then
  fail "(b) an undeclared line exits 0 (rc=$LAST_RC: $LAST_OUT)"
else
  pass "(b) an undeclared line exits 0"
  if grep -qF "::warning::" <<< "$LAST_OUT" \
    && grep -qF "4.1.1" <<< "$LAST_OUT" \
    && grep -qF "4.1" <<< "$LAST_OUT"; then
    pass "(b) the warning names the version and the line it would have declared"
  else
    fail "(b) the warning names the version and the line it would have declared ($LAST_OUT)"
  fi
  if cmp -s "$WORK/before-undeclared.json" "$D/.github/support.json"; then
    pass "(b) the declaration is byte-identical: no line was invented"
  else
    fail "(b) the declaration is byte-identical: no line was invented"
  fi
fi

# ─── (b') a MINOR release on a line that is NOT declared rotates the window ─
# This is the row option A exists for. A minor release publishes its tag only
# AFTER the Release PR merges, so the Release PR itself has to carry the new
# support window: the tag it is about to publish does not exist yet. The
# writer delegates the rotation to scripts/rotate-stable.sh
# --pending-publication, whose premise is the workspace version of the
# checkout it runs in.
# NEGATION: pre-#2531 the writer SKIPPED this case with a warning, so the
# Release PR carried no window at all, the tag landed without its
# declaration, and the enforcing gate turned red for EVERY pull request in
# the repository (#2052, #2520).
D="$(setup_writer_repo writer-minor-rotation 4.1.0 4.0 4.0.0)"
# The published tag of the current STABLE: the repository always carries it,
# and the rotation reads the published minors to find the gap.
git -C "$D" tag v4.0.0
run_writer "$D" --write
if [[ $LAST_RC -ne 0 ]]; then
  fail "(b') the writer rotates the window of a pending minor (rc=$LAST_RC: $LAST_OUT)"
else
  pass "(b') the writer rotates the window of a pending minor"
  if [[ "$(jq -r '.lines[] | select(.minor == "4.1") | .state' "$D/.github/support.json")" == "STABLE" ]]; then
    pass "(b') the new line 4.1 is STABLE"
  else
    fail "(b') the new line 4.1 is STABLE ($(jq -r '.lines[] | select(.minor == "4.1") | .state // "absent"' "$D/.github/support.json"))"
  fi
  if [[ "$(jq -r '.lines[] | select(.minor == "4.1") | .latest' "$D/.github/support.json")" == "4.1.0" ]]; then
    pass "(b') the new line 4.1 declares the pending publication 4.1.0"
  else
    fail "(b') the new line 4.1 declares the pending publication 4.1.0"
  fi
  if [[ "$(jq -r '.lines[] | select(.minor == "4.0") | .state' "$D/.github/support.json")" == "MAINTENANCE" ]]; then
    pass "(b') the previous STABLE 4.0 is demoted to MAINTENANCE"
  else
    fail "(b') the previous STABLE 4.0 is demoted to MAINTENANCE ($(jq -r '.lines[] | select(.minor == "4.0") | .state' "$D/.github/support.json"))"
  fi
  if [[ "$(jq -c '.lines[] | select(.minor == "4.0") | .accepts' "$D/.github/support.json")" == '["security"]' ]]; then
    pass "(b') the demoted line 4.0 admits only security"
  else
    fail "(b') the demoted line 4.0 admits only security"
  fi
  if grep -qF "rotated the support window for 4.1" <<< "$LAST_OUT"; then
    pass "(b') the writer names the rotation it made"
  else
    fail "(b') the writer names the rotation it made ($LAST_OUT)"
  fi
  if ( cd "$D" && bash scripts/render-support-md.sh --check >/dev/null 2>&1 ); then
    pass "(b') the rendered projection travels with the rotated declaration"
  else
    fail "(b') the rendered projection travels with the rotated declaration"
  fi
fi

# ─── (b'') a rotation that CANNOT be done fails the writer ─────────────────
# v4.1.0 is published but was never declared: rotating over it would leave
# the chain broken, and closing that gap is a governance decision. The
# release must stop BEFORE the tag exists — publishing a tag whose
# declaration is missing is what reddened the whole repository.
# NEGATION: pre-#2531 this case was a warning and a skip, and the release
# went on to publish the tag without a declaration.
D="$(setup_writer_repo writer-gap 4.2.0 4.0 4.0.0)"
git -C "$D" tag v4.1.0
cp "$D/.github/support.json" "$WORK/before-gap.json"
run_writer "$D" --write
if [[ $LAST_RC -ne 0 ]]; then
  pass "(b'') a rotation that cannot be done fails the writer"
  if grep -qF "::error::" <<< "$LAST_OUT" && grep -qF "4.2.0" <<< "$LAST_OUT"; then
    pass "(b'') the refusal is a named error that names the version"
  else
    fail "(b'') the refusal is a named error that names the version ($LAST_OUT)"
  fi
  if cmp -s "$WORK/before-gap.json" "$D/.github/support.json"; then
    pass "(b'') the declaration is byte-identical after the refusal"
  else
    fail "(b'') the declaration is byte-identical after the refusal"
  fi
  # The success line must be unreachable on the refusal path: a stdout line
  # reporting a rotation would tell unattended log consumers that key on the
  # message instead of the exit code that the rotation landed — the
  # silent-success class this change exists to eliminate. Today `err`
  # terminates the process; this row keeps that invariant true even if the
  # helper's contract ever changes.
  if ! grep -qF "rotated the support window" <<< "$LAST_OUT"; then
    pass "(b'') the refusal output carries no success line"
  else
    fail "(b'') the refusal output carries no success line"
  fi
else
  fail "(b'') a rotation that cannot be done fails the writer (rc=0: $LAST_OUT)"
fi

# ─── (c) the writer is idempotent ──────────────────────────────────────────
# The release-plz job re-runs on every push to `main`, because release-plz
# regenerates the Release PR branch every time. A writer that rewrote the
# declaration on every run would produce a commit per push and wake the whole
# CI pipeline for nothing.
# NEGATION: pre-fix the flow did not exist, and the human bump it replaces
# was deleted by release-plz on the next push to `main` — measured three times
# in one afternoon (v2.12.0 twice, v3.0.1 once).
D="$(setup_writer_repo writer-idempotent 4.0.1 4.0 4.0.0)"
run_writer "$D" --write
cp "$D/.github/support.json" "$WORK/after-first.json"
cp "$D/SUPPORT.md" "$WORK/after-first-SUPPORT.md"
run_writer "$D" --write
if [[ $LAST_RC -ne 0 ]]; then
  fail "(c) the second run exits 0 (rc=$LAST_RC: $LAST_OUT)"
else
  pass "(c) the second run exits 0"
  if grep -qF "already in sync" <<< "$LAST_OUT"; then
    pass "(c) the second run reports that the declaration is already in sync"
  else
    fail "(c) the second run reports that the declaration is already in sync ($LAST_OUT)"
  fi
  if cmp -s "$WORK/after-first.json" "$D/.github/support.json"; then
    pass "(c) the declaration is byte-identical after the second run"
  else
    fail "(c) the declaration is byte-identical after the second run"
  fi
  if cmp -s "$WORK/after-first-SUPPORT.md" "$D/SUPPORT.md"; then
    pass "(c) the rendered projection is byte-identical after the second run"
  else
    fail "(c) the rendered projection is byte-identical after the second run"
  fi
fi

# ─── (d) a version that is not X.Y.Z is refused ─────────────────────────────
# A declaration of "4.0" would be a fact nobody published, and a declaration
# of "banana" is not a version at all. The writer refuses, names the version
# and leaves the declaration untouched.
# NEGATION: pre-fix the version was not read at all, so a malformed one could
# not be refused.
for BAD in "4.0" "banana"; do
  D="$(setup_writer_repo "writer-malformed-$BAD" "$BAD" 4.0 4.0.0)"
  cp "$D/.github/support.json" "$WORK/before-malformed.json"
  run_writer "$D" --write
  if [[ $LAST_RC -eq 0 ]]; then
    fail "(d) the malformed version '$BAD' exits non-zero"
  else
    pass "(d) the malformed version '$BAD' exits non-zero"
    if grep -qF "::error::" <<< "$LAST_OUT" && grep -qF "$BAD" <<< "$LAST_OUT"; then
      pass "(d) the malformed version '$BAD' is refused with a named error"
    else
      fail "(d) the malformed version '$BAD' is refused with a named error ($LAST_OUT)"
    fi
    if cmp -s "$WORK/before-malformed.json" "$D/.github/support.json"; then
      pass "(d) the declaration is byte-identical after the refusal of '$BAD'"
    else
      fail "(d) the declaration is byte-identical after the refusal of '$BAD'"
    fi
  fi
done

# ─── (e) the orchestrator declares the publication IN THE RELEASE BRANCH ────
# This is the row the whole flow exists for: the declaration has to land on
# the branch the next tag is cut from, with the bot identity, and it has to be
# pushed — a declaration that lands anywhere else is invisible from the
# outside and turns the enforcing gate red after the tag is published.
# NEGATION: pre-fix the release-plz path wrote no declaration at all, and the
# human bump that used to stand in for it was deleted by release-plz on the
# next push to `main`.
D="$(setup_release_repo orch-e2e 4.0.0 4.0.1 4.0 4.0.0)"
write_prs '[{"number":42,"headRefName":"chore/release-4.0.1","updatedAt":"2026-10-20T10:00:00Z","isCrossRepository":false,"state":"OPEN"}]'
BEFORE_SHA="$(remote_branch_sha "$D" chore/release-4.0.1)"
BEFORE_COUNT="$(remote_commit_count "$D" chore/release-4.0.1)"
MAIN_SHA="$(remote_branch_sha "$D" main)"
if run_orchestrator "$D"; then
  pass "(e) the orchestrator exits 0"
  REMOTE_LATEST="$(git -C "$D/remote.git" show "refs/heads/chore/release-4.0.1:.github/support.json" \
    | jq -r '.lines[] | select(.minor == "4.0") | .latest')"
  if [[ "$REMOTE_LATEST" == "4.0.1" ]]; then
    pass "(e) the remote release branch declares the pending publication 4.0.1"
  else
    fail "(e) the remote release branch declares the pending publication 4.0.1 ($REMOTE_LATEST)"
  fi
  if [[ "$(remote_commit_count "$D" chore/release-4.0.1)" -eq $((BEFORE_COUNT + 1)) ]]; then
    pass "(e) exactly one commit landed on the release branch"
  else
    fail "(e) exactly one commit landed on the release branch ($(remote_commit_count "$D" chore/release-4.0.1) commits, was $BEFORE_COUNT)"
  fi
  COMMIT_SUBJECT="$(git -C "$D/remote.git" log -1 --format=%s "refs/heads/chore/release-4.0.1")"
  if [[ "$COMMIT_SUBJECT" == "chore(support): declare the pending publication of v4.0.1" ]]; then
    pass "(e) the commit subject names the publication it declares"
  else
    fail "(e) the commit subject names the publication it declares ($COMMIT_SUBJECT)"
  fi
  COMMIT_IDENTITY="$(git -C "$D/remote.git" log -1 --format='%an <%ae>' "refs/heads/chore/release-4.0.1")"
  if [[ "$COMMIT_IDENTITY" == "github-actions[bot] <bot@github.com>" ]]; then
    pass "(e) the commit is authored by the bot identity"
  else
    fail "(e) the commit is authored by the bot identity ($COMMIT_IDENTITY)"
  fi
  if [[ "$(remote_branch_sha "$D" main)" == "$MAIN_SHA" ]]; then
    pass "(e) the declaration never lands on main"
  else
    fail "(e) the declaration never lands on main"
  fi
  # The rendered projection travels with the declaration: it is the other
  # half of the same fact, and a commit that declares without re-rendering
  # leaves the projection stale.
  REMOTE_SUPPORT_MD="$(git -C "$D/remote.git" show "refs/heads/chore/release-4.0.1:SUPPORT.md")"
  if grep -qF "| 4.0 | STABLE | 4.0.1 |" <<< "$REMOTE_SUPPORT_MD"; then
    pass "(e) the rendered projection travels with the declaration"
  else
    fail "(e) the rendered projection travels with the declaration"
  fi
else
  fail "(e) the orchestrator exits 0 (rc=$LAST_RC: $LAST_OUT)"
fi

# ─── (f) no open Release PR is a no-op ──────────────────────────────────────
# release-plz decided there is no release to prepare, so there is no pending
# publication to declare. Failing here would red every push to `main` that
# carries no version bump.
# NEGATION: pre-fix nothing consumed the "there is no release to prepare"
# outcome; a flow that treated it as an error would red every push to `main`.
D="$(setup_release_repo orch-no-pr 4.0.0 4.0.1 4.0 4.0.0)"
write_prs '[]'
BEFORE_SHA="$(remote_branch_sha "$D" chore/release-4.0.1)"
BEFORE_COUNT="$(remote_commit_count "$D" chore/release-4.0.1)"
if run_orchestrator "$D"; then
  pass "(f) an empty pull request list exits 0"
  if grep -qF "no release to prepare" <<< "$LAST_OUT"; then
    pass "(f) the notice says there is no release to prepare"
  else
    fail "(f) the notice says there is no release to prepare ($LAST_OUT)"
  fi
  if [[ "$(remote_branch_sha "$D" chore/release-4.0.1)" == "$BEFORE_SHA" \
    && "$(remote_commit_count "$D" chore/release-4.0.1)" -eq "$BEFORE_COUNT" ]]; then
    pass "(f) nothing was pushed"
  else
    fail "(f) nothing was pushed"
  fi
else
  fail "(f) an empty pull request list exits 0 (rc=$LAST_RC: $LAST_OUT)"
fi

# ─── (g) a second orchestrator run adds no commit ───────────────────────────
# The release-plz job re-runs on every push to `main` (release-plz regenerates
# the Release PR branch every time), so the flow has to be idempotent at the
# COMMIT level: a second run must not push an empty commit, or every push to
# `main` would wake the whole CI pipeline for a declaration that is already
# there.
# NEGATION: pre-fix the human bump this flow replaces was deleted by
# release-plz on the next push to `main` and had to be redone by hand; a
# second commit would put the branch permanently ahead of the release-plz
# commit and make the Release PR unmergeable.
D="$(setup_release_repo orch-idempotent 4.0.0 4.0.1 4.0 4.0.0)"
write_prs '[{"number":42,"headRefName":"chore/release-4.0.1","updatedAt":"2026-10-20T10:00:00Z","isCrossRepository":false,"state":"OPEN"}]'
BEFORE_COUNT="$(remote_commit_count "$D" chore/release-4.0.1)"
if run_orchestrator "$D"; then
  FIRST_COUNT="$(remote_commit_count "$D" chore/release-4.0.1)"
  if run_orchestrator "$D"; then
    SECOND_COUNT="$(remote_commit_count "$D" chore/release-4.0.1)"
    if [[ "$FIRST_COUNT" -eq $((BEFORE_COUNT + 1)) && "$SECOND_COUNT" -eq "$FIRST_COUNT" ]]; then
      pass "(g) a second orchestrator run adds no commit (idempotent)"
    else
      fail "(g) a second orchestrator run adds no commit (idempotent) (count $BEFORE_COUNT -> $FIRST_COUNT -> $SECOND_COUNT)"
    fi
    if grep -qF "no changes" <<< "$LAST_OUT"; then
      pass "(g) the second run says the branch already declares the publication"
    else
      fail "(g) the second run says the branch already declares the publication ($LAST_OUT)"
    fi
  else
    fail "(g) a second orchestrator run adds no commit (idempotent) (rc=$LAST_RC: $LAST_OUT)"
  fi
else
  fail "(g) a second orchestrator run adds no commit (idempotent) (rc=$LAST_RC: $LAST_OUT)"
fi

# ─── (h) the orchestrator ROTATES the window in the Release PR branch ──────
# The end-to-end row of option A: the Release PR branch itself carries the
# new support window, so the tag it is about to publish finds its
# declaration already there. The commit lands on the release branch with the
# bot identity, and the rendered projection travels with it.
# NEGATION: pre-#2531 the Release PR branch carried NO window: the tag
# landed, the declaration did not, and the enforcing gate turned red for
# every pull request in the repository (#2052, #2520).
D="$(setup_release_repo orch-minor 4.0.0 4.1.0 4.0 4.0.0)"
write_prs '[{"number":42,"headRefName":"chore/release-4.1.0","updatedAt":"2026-10-20T10:00:00Z","isCrossRepository":false,"state":"OPEN"}]'
BEFORE_COUNT="$(remote_commit_count "$D" chore/release-4.1.0)"
if run_orchestrator "$D"; then
  pass "(h) the orchestrator exits 0 on a pending minor"
  REMOTE_LINES="$(git -C "$D/remote.git" show "refs/heads/chore/release-4.1.0:.github/support.json")"
  if [[ "$(jq -r '.lines[] | select(.minor == "4.1") | .state' <<< "$REMOTE_LINES")" == "STABLE" ]]; then
    pass "(h) the Release PR branch declares the new line 4.1 STABLE"
  else
    fail "(h) the Release PR branch declares the new line 4.1 STABLE ($REMOTE_LINES)"
  fi
  if [[ "$(jq -r '.lines[] | select(.minor == "4.0") | .state' <<< "$REMOTE_LINES")" == "MAINTENANCE" ]]; then
    pass "(h) the Release PR branch demotes 4.0 to MAINTENANCE"
  else
    fail "(h) the Release PR branch demotes 4.0 to MAINTENANCE ($REMOTE_LINES)"
  fi
  if [[ "$(remote_commit_count "$D" chore/release-4.1.0)" -eq $((BEFORE_COUNT + 1)) ]]; then
    pass "(h) exactly one commit landed on the release branch"
  else
    fail "(h) exactly one commit landed on the release branch"
  fi
  REMOTE_SUPPORT_MD="$(git -C "$D/remote.git" show "refs/heads/chore/release-4.1.0:SUPPORT.md")"
  if grep -qF "| 4.1 | STABLE | 4.1.0 |" <<< "$REMOTE_SUPPORT_MD"; then
    pass "(h) the rendered projection travels with the rotated declaration"
  else
    fail "(h) the rendered projection travels with the rotated declaration"
  fi
else
  fail "(h) the orchestrator exits 0 on a pending minor (rc=$LAST_RC: $LAST_OUT)"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]
