#!/usr/bin/env bash
# test_ci_fast_gate.sh — semantics harness for the fast gate's lane decision.
#
# The fast gate decides HOW MUCH to verify, and three of the fixes that make
# that decision fail-closed shipped with NO test at all:
#
#   A1  the path classifier's exit status was ignored, so a crash left every
#       flag at its default and the gate fell through to the cheap docs+CI
#       lanes: no cargo, green gate, nothing verified;
#   A2  the docs-only re-assertion's pattern was over-escaped and reached grep
#       as `\\.rs$` (a literal backslash plus any character), so it matched
#       nothing: a stray foo.rs, Cargo.toml or Cargo.lock passed the assert;
#   B45 an unresolvable base ref fell back to HEAD~1, which is not the PR
#       base: the branch tip got classified instead of the branch, NARROWING
#       the check where it had to widen;
#   B51 the changed-scripts filter was `^scripts/.*\.sh$`, so the git hooks
#       — bash WITH a shebang and WITHOUT the .sh suffix — were classified
#       `ci_only` and then skipped as "no changed scripts": never linted;
#   B52 `all=true` ran the FULL lane, which never ran the CI lane's
#       semantics harnesses while its banner claimed "nothing skipped";
#   B53 the pinned grep arms conflated grep's exit 1 (nothing found) with
#       its exit 2 (the scan could not run): an unreadable source tree
#       printed the clean message and the step passed.
#
# A gate that cannot fail is indistinguishable from a gate that is not there,
# and the audit that found these shapes called the first of them CRITICAL. This
# harness pins them by OBSERVABLE BEHAVIOUR only: the gate's exit code, the
# lane it selected, the error it printed, and the steps it reported as failed.
# It never greps the gate's source for a string.
#
# RESOURCE CONTRACT (S16) — this harness compiles NOTHING:
#   * it never runs the real gate against the workspace: every run happens in
#     a synthetic fixture repo with its own .git and its own scripts/;
#   * every tool the gate can invoke in the exercised lanes is STUBBED on
#     PATH (cargo, rustfmt, rustdoc, cargo-clippy, cargo-nextest, ...) and the
#     stub records its own path and argv in a log the harness asserts on;
#     the ONE exception is the DELEGATING stub for shellcheck, which execs
#     the real binary resolved at setup (issue #2606): B51/A15 exercise real
#     analysis semantics, but must never inherit the HOST's tool resolution;
#   * CARGO_TARGET_DIR points at an EMPTY scratch dir, and the harness
#     asserts it is still empty when it finishes;
#   * the scratch lives on DISK. /tmp is a tmpfs on this machine, so a scratch
#     there is RAM: a 1.9 GB cargo target left in /tmp is what took the
#     owner's session down twice.
#
# This harness needs no real cargo at all, so it never resolves one — in
# particular it never uses `command -v cargo`, which in a worktree returns
# the mbx cache wrapper whose whole purpose is to build.
#
# Usage: bash scripts/test_ci_fast_gate.sh
# Env:
#   WEBFANG_HARNESS_SCRATCH_BASE  base dir for the scratch (default
#                                  $HOME/.cache/tmp; a tmpfs is refused)
#   WEBFANG_HARNESS_KEEP_SCRATCH  set to 1 to keep the scratch for inspection
#
# Manual: seconds. No network, no cargo, no mutation of the repository this
# harness runs from.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# The operator's PATH, captured BEFORE any run replaces it inside a subshell.
# Reading `$PATH` after that point would read the subshell's value, and the
# linter is right to call that out (SC2031).
AMBIENT_PATH="$PATH"

PASS=0
FAIL=0
SKIP=0
ok() { PASS=$((PASS + 1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$1"; }
# A case that cannot run here is a VISIBLE omission, never a silent pass and
# never a false failure — the same rule the gate applies to a missing tool.
skip() { SKIP=$((SKIP + 1)); printf '  skip %s\n' "$1"; }

# has <file> <fixed-string> <label>: pass when the string IS in the file.
has() {
  if grep -qF -- "$2" "$1"; then
    ok "$3"
  else
    bad "$3 (not found: $2)"
  fi
}

# lacks <file> <fixed-string> <label>: pass when the string is NOT in the file.
lacks() {
  if grep -qF -- "$2" "$1"; then
    bad "$3 (unexpectedly present: $2)"
  else
    ok "$3"
  fi
}

if ! command -v git >/dev/null 2>&1; then
  echo "harness: git is required to build the fixtures" >&2
  exit 2
fi

START_SECONDS="$(date +%s)"

# --- scratch (on DISK, never on a tmpfs) --------------------------------------
#
# The scratch base is resolved, not assumed: a tmpfs candidate is refused
# outright, because a scratch on RAM is the accident this harness exists to
# prevent. The resolution never depends on the operator's environment beyond
# the filesystem type of the candidate.
fs_type() { df -P "$1" 2>/dev/null | awk 'NR==2 {print $1}'; }

SCRATCH_BASE=""
for candidate in "${WEBFANG_HARNESS_SCRATCH_BASE:-}" "$HOME/.cache/tmp" "$HOME/.cache" "$HOME"; do
  [ -n "$candidate" ] || continue
  mkdir -p "$candidate" 2>/dev/null || continue
  if [ "$(fs_type "$candidate")" = "tmpfs" ]; then
    printf 'harness: refusing %s as a scratch base (tmpfs is RAM)\n' "$candidate" >&2
    continue
  fi
  SCRATCH_BASE="$candidate"
  break
done
if [ -z "$SCRATCH_BASE" ]; then
  SCRATCH_BASE="$HOME/.cache/tmp"
  mkdir -p "$SCRATCH_BASE" 2>/dev/null || true
  echo "harness: no non-tmpfs scratch base found; falling back to $SCRATCH_BASE" >&2
fi

SCRATCH="$(mktemp -d "$SCRATCH_BASE/webfang-ci-fast-gate.XXXXXX")" || {
  echo "harness: cannot create a scratch dir under $SCRATCH_BASE" >&2
  exit 2
}
STUB_DIR="$SCRATCH/stub"
STUB_LOG="$SCRATCH/stub-invocations.log"
TARGET_DIR="$SCRATCH/target"
mkdir -p "$STUB_DIR" "$TARGET_DIR" "$SCRATCH/store/seeds" "$SCRATCH/store/quarantine"
: >"$STUB_LOG"

cleanup() {
  if [ "${WEBFANG_HARNESS_KEEP_SCRATCH:-0}" = "1" ]; then
    printf 'harness: scratch kept at %s\n' "$SCRATCH"
    return 0
  fi
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

# --- tool stubs (the gate can never reach a real toolchain) --------------------
#
# The stub records its own path and argv, and returns success. It never execs
# anything: a compile cannot happen even when the gate asks for one, so the
# "no build" promise is structural instead of a promise.
for stub_name in cargo rustfmt rustdoc cargo-clippy cargo-nextest cargo-fmt \
  cargo-metadata clippy-driver jscpd; do
  cat >"$STUB_DIR/$stub_name" <<'STUB'
#!/usr/bin/env bash
# Fixture stub: records the invocation, returns success, builds nothing.
printf '%s\t%s\t%s\n' "$(basename "$0")" "$0" "$*" >>"$WEBFANG_GATE_STUB_LOG"
exit 0
STUB
  chmod +x "$STUB_DIR/$stub_name"
done

# The shellcheck stub is DELEGATING, unlike the toolchain stubs above: B51/A15
# exercise the gate's shellcheck step with REAL shellcheck semantics (a file
# whose comment line begins with "shellcheck" must be read as a directive, and
# a clean script must pass), so a canned exit-0 stub would pin nothing. What
# the harness must NOT inherit is the HOST's tool RESOLUTION: on a
# version-manager host the PATH's `shellcheck` can be a per-CWD shim that
# exits 1 from any directory without a version config (issue #2606: the mise
# shim reds B51/A15 from inside the fixtures while CI is green on the same
# commits). So the REAL binary is resolved ONCE here — `mise -C "$REPO_ROOT"`
# reads this repository's own config regardless of the caller's CWD, and on a
# host without mise the plain PATH lookup is CI's pinned binary (aqua pin in
# mise.toml == SC_VERSION). With neither, the stub fails 127 loudly: "resolved
# but broken" is a harness-environment defect, never a green gate.
REAL_SHELLCHECK=""
if command -v mise >/dev/null 2>&1; then
  REAL_SHELLCHECK="$(mise -C "$REPO_ROOT" which shellcheck 2>/dev/null)" || REAL_SHELLCHECK=""
fi
if [ -z "$REAL_SHELLCHECK" ] || [ ! -x "$REAL_SHELLCHECK" ]; then
  REAL_SHELLCHECK="$(command -v shellcheck 2>/dev/null)" || REAL_SHELLCHECK=""
fi
if [ -n "$REAL_SHELLCHECK" ] && [ ! -x "$REAL_SHELLCHECK" ]; then
  REAL_SHELLCHECK=""
fi
cat >"$STUB_DIR/shellcheck" <<STUB
#!/usr/bin/env bash
# Fixture stub (DELEGATING, issue #2606): records the invocation, then execs
# the REAL shellcheck resolved at harness setup. Never resolve shellcheck
# from PATH inside the fixture — a per-CWD shim (mise) exits 1 there.
printf '%s\t%s\t%s\n' "\$(basename "\$0")" "\$0" "\$*" >>"\$WEBFANG_GATE_STUB_LOG"
REAL="$REAL_SHELLCHECK"
if [ -n "\$REAL" ] && [ -x "\$REAL" ]; then
  exec "\$REAL" "\$@"
fi
echo "shellcheck-stub: no real shellcheck was resolved at setup (issue #2606)" >&2
exit 127
STUB
chmod +x "$STUB_DIR/shellcheck"

# --- fixtures -----------------------------------------------------------------
#
# A fixture is a synthetic git repo with its own .git, its own scripts/ (the
# REAL gate, the REAL target-isolation guard, and a classifier in the mode
# the case needs) and a committed .envrc that registers the scratch target —
# without that registration the target-isolation guard refuses the run before
# any lane, which would make every case measure the wrong thing.

# fixture_git <dir> <git args...>: git with a hermetic identity and no signing.
fixture_git() {
  local dir="$1"
  shift
  git -C "$dir" \
    -c user.name="ci-fast-gate harness" \
    -c user.email="harness@example.invalid" \
    -c commit.gpgsign=false \
    "$@"
}

# write_classifier <path> <mode>
#
#   real     the repo's own ci_path_classifier.sh
#   crash    exits non-zero without a verdict (A1: the exit status)
#   partial  exits zero with an INCOMPLETE verdict (A1: a missing key)
#   docs     exits zero claiming docs-only (A2: the re-assertion's input)
#   full     exits zero claiming the UNKNOWN scope (B52: the FULL lane)
write_classifier() {
  local path="$1" mode="$2"
  case "$mode" in
    real)
      cp "$REPO_ROOT/scripts/ci_path_classifier.sh" "$path"
      ;;
    crash)
      cat >"$path" <<'CLS'
#!/usr/bin/env bash
# Fixture stub: a classifier that CRASHES — non-zero exit, no verdict.
# The gate must treat that as a failed step and a widening, never as
# "nothing changed, the cheap lanes are fine".
echo "fixture-classifier: simulated crash" >&2
exit 1
CLS
      ;;
    partial)
      cat >"$path" <<'CLS'
#!/usr/bin/env bash
# Fixture stub: a classifier that exits 0 but writes an INCOMPLETE verdict.
# The gate must treat an absent key as an untrustworthy verdict.
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --github-output) out="${2:-}"; shift 2 ;;
    --base-ref | --head-ref | --format | --files) shift 2 ;;
    *) shift ;;
  esac
done
{
  echo "docs_only=true"
  echo "ci_only=false"
  echo "code_changed=false"
} >>"$out"
exit 0
CLS
      ;;
    docs)
      cat >"$path" <<'CLS'
#!/usr/bin/env bash
# Fixture stub: a classifier that CLAIMS docs-only.
#
# The gate hands the classifier the union of the committed range and the live
# worktree, so with the real classifier a .rs file can never coexist with
# docs_only=true. This stub reproduces the state the docs-only re-assertion
# exists to defend against — "the classifier says documentation, the worktree
# holds code" — which is the only way to reach that assertion at all.
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --github-output) out="${2:-}"; shift 2 ;;
    --base-ref | --head-ref | --format | --files) shift 2 ;;
    *) shift ;;
  esac
done
{
  echo "docs_only=true"
  echo "ci_only=false"
  echo "code_changed=false"
  echo "ai_changed=false"
  echo "mcp_changed=false"
  echo "cli_changed=false"
  echo "core_changed=false"
  echo "crawler_changed=false"
  echo "downloader_changed=false"
  echo "tests_changed=false"
  echo "release_changed=false"
  echo "lock_changed=false"
  echo "lib_src_changed=false"
  echo "all=false"
} >>"$out"
exit 0
CLS
      ;;
    full)
      cat >"$path" <<'CLS'
#!/usr/bin/env bash
# Fixture stub: a classifier that reports the UNKNOWN scope.
#
# This is the verdict the gate gets when the changed-file set proves
# nothing about the scope, and it is the only way to reach the FULL lane
# without also failing on an unresolvable base (B52).
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --github-output) out="${2:-}"; shift 2 ;;
    --base-ref | --head-ref | --format | --files) shift 2 ;;
    *) shift ;;
  esac
done
{
  echo "docs_only=false"
  echo "ci_only=false"
  echo "code_changed=false"
  echo "ai_changed=false"
  echo "mcp_changed=false"
  echo "cli_changed=false"
  echo "core_changed=false"
  echo "crawler_changed=false"
  echo "downloader_changed=false"
  echo "tests_changed=false"
  echo "release_changed=false"
  echo "lock_changed=false"
  echo "lib_src_changed=false"
  echo "all=true"
} >>"$out"
exit 0
CLS
      ;;
    *)
      echo "harness: unknown classifier mode '$mode'" >&2
      exit 2
      ;;
  esac
  chmod +x "$path"
}

# make_fixture <name> <branch> <classifier-mode>: prints the fixture path.
make_fixture() {
  local name="$1" branch="$2" mode="$3"
  local dir="$SCRATCH/fixtures/$name"
  mkdir -p "$dir/scripts" "$dir/docs"
  cp "$REPO_ROOT/scripts/ci_fast_gate.sh" "$dir/scripts/ci_fast_gate.sh"
  cp "$REPO_ROOT/scripts/check_target_isolation.sh" "$dir/scripts/check_target_isolation.sh"
  write_classifier "$dir/scripts/ci_path_classifier.sh" "$mode"
  # The gate's coverage table (#2559). The harness owns this fixture so the
  # coverage assertions stay exact while the real table evolves; the real
  # table's shape is pinned against `gate.needs` by
  # scripts/test_ci_gate_structure.sh, which is where that contract belongs.
  #
  # Every row here exists to pin one claim the real table makes, and one
  # review finding each: a note on a COVERED job, a note on a NOT-covered job,
  # a lane list written with a space after the comma, `test-core` partial in
  # the code lane and full in the full lane, and `security` partially covered
  # so it is NOT listed among the jobs no lane covers.
  {
    printf 'change-scope\tdocs,ci,code,full\t-\tthe path classifier\t-\n'
    printf 'fmt\tcode, full\t-\tcargo fmt --all -- --check\t-\n'
    printf 'test-core\tfull\tcode\tworkspace nextest\tnextest for the selected crates only\n'
    printf 'repo-guards\t-\tcode,full\t-\ta subset of the CI steps\n'
    printf 'security\t-\tci,full\t-\tthe cached zizmor audit runs; cargo deny never does\n'
    printf 'toolchain\t-\t-\t-\tno local equivalent\n'
  } >"$dir/scripts/ci_fast_gate_coverage.tsv"
  # The FULL lane runs this one through `bash` (not through run_guard), so the
  # fixture provides a no-op instead of letting the step fail for a reason
  # that has nothing to do with the case under test.
  printf '#!/usr/bin/env bash\nexit 0\n' >"$dir/scripts/test_release_provenance.sh"
  # The FULL lane runs the CI-ONLY lane's semantics harnesses too (B52), so
  # the fixture provides a no-op for each of them: the case under test is the
  # LANE SHAPE, not the harnesses' own semantics, and a missing script would
  # fail the step for a reason that has nothing to do with the case.
  for harness in     test_ci_health_close.sh tests/test_check_compatibility.sh \
    tests/test_check_target_isolation.sh tests/test_check_windows_platform.sh \
    ci_path_classifier_test.sh \
    test_ci_plan.sh test_ci_gate_structure.sh test_ci_queue_metrics.sh \
    test_docs_version_pins.sh test_docs_version_pins_wiring.sh \
    test_bump_docs_version_pins.sh test_apply_release_pins.sh \
    test_release_pins_wiring.sh test_declare_pending_publication.sh \
    test_mcp_surface_wiring.sh test_merge_when_green_breaking.sh \
    test_ci_merge_when_green.sh test_apply_tag_ruleset.sh test_ci_metrics.sh \
    test_ci_mutant_shards.sh test_file_size.sh test_bug_discovery_report.sh \
    test_docs_claims.sh test_docs_claims_wiring.sh; do
    mkdir -p "$dir/scripts/$(dirname "$harness")"
    printf '#!/usr/bin/env bash\nexit 0\n' >"$dir/scripts/$harness"
  done
  # The intra-crate direction gate CONSUMES the AST extractor and fails closed
  # when the binary is absent (there is no fallback scanner), and the local
  # gate builds it before running the gate. The fixture provides a fake
  # executable so the build step does not run cargo: the case under test is
  # the LANE SHAPE, not the extractor's semantics, and a real build in a
  # fixture would fail for a reason that has nothing to do with the case.
  mkdir -p "$dir/tools/intra-crate-lint/target/debug"
  printf '#!/usr/bin/env bash\necho "fixture: fake intra-crate lint extractor"\n' \
    >"$dir/tools/intra-crate-lint/target/debug/intra-crate-lint"
  chmod +x "$dir/tools/intra-crate-lint/target/debug/intra-crate-lint"
  # A real source tree, so the pinned greps of the fmt+guards lane have
  # something to scan. The scan now fails CLOSED (B53), so a missing tree
  # would fail the step for a reason that has nothing to do with the case.
  mkdir -p "$dir/crates/webfang_core/src"
  printf 'pub fn fixture() {}\n' >"$dir/crates/webfang_core/src/lib.rs"
  # The target registry the isolation guard reads: the fixture's own .envrc
  # declares the scratch target, so the guard allows it (own registration)
  # without any real worktree being involved.
  printf 'export CARGO_TARGET_DIR=%s\n' "$TARGET_DIR" >"$dir/.envrc"
  # The gate appends its timing row under docs/ci-metrics/: ignoring it keeps
  # that file out of the changed-file union of the next run in the same tree.
  printf 'docs/ci-metrics/\n' >"$dir/.gitignore"
  fixture_git "$dir" init -q -b "$branch"
  fixture_git "$dir" add -A
  fixture_git "$dir" commit -q -m "fixture: synthetic repo for the fast-gate harness"
  # A second commit, so the fixture has a resolvable HEAD~1: the pre-fix
  # fallback to HEAD~1 (B45) classified the branch TIP instead of the branch,
  # and a single-commit fixture could not tell the two apart.
  printf 'guide\n' >"$dir/docs/guide.rst"
  fixture_git "$dir" add -A
  fixture_git "$dir" commit -q -m "fixture: documentation change"
  printf '%s' "$dir"
}

# touch_worktree <fixture-dir>: an uncommitted edit of a tracked file, so the
# changed-file union is narrow and non-empty (an empty union would widen
# every verdict on its own and make the lane assertions vacuous).
touch_worktree() {
  printf 'section two\n' >>"$1/docs/guide.rst"
}

# --- driving the gate ---------------------------------------------------------

GATE_OUT=""
GATE_ERR=""
GATE_RC=0

# run_gate_path <path> <fixture-dir> [gate args...]: run the fixture's gate with
# an explicit PATH and the scratch target registered.
run_gate_path() {
  local path="$1" dir="$2"
  shift 2
  GATE_OUT="$SCRATCH/gate.out"
  GATE_ERR="$SCRATCH/gate.err"
  (
    cd "$dir" || exit 127
    unset GITHUB_OUTPUT CI_PATH_CLASSIFIER_FILES WEBFANG_REAL_CARGO
    export PATH="$path"
    export CARGO_TARGET_DIR="$TARGET_DIR"
    export WEBFANG_SEEDS_ROOT="$SCRATCH/store/seeds"
    export WEBFANG_QUARANTINE_ROOT="$SCRATCH/store/quarantine"
    export WEBFANG_GATE_STUB_LOG="$STUB_LOG"
    bash scripts/ci_fast_gate.sh "$@"
  ) >"$GATE_OUT" 2>"$GATE_ERR"
  GATE_RC=$?
}

# run_gate <fixture-dir> [gate args...]: run the fixture's gate with the stubs
# on PATH and the scratch target registered.
run_gate() {
  run_gate_path "$STUB_DIR:$AMBIENT_PATH" "$@"
}

# run_gate_no_jscpd <fixture-dir> [gate args...]: the gate runs with a PATH that
# holds the stubs and the core utilities, and NO jscpd anywhere. On a machine
# that has the real binary a plain `run_gate` would take the run branch and the
# fail-open case would measure nothing.
#
# The PATH is BUILT, never pruned. Dropping every ambient directory that carries
# a `jscpd` also drops that directory's `date`, `sed`, `grep` or `bash` on a
# single-prefix layout, and the case would then report a regression that has
# nothing to do with jscpd. Symlinking the binaries the code lane needs into one
# directory keeps the promise ("nothing real is reached") without that coupling.
#
# Returns 1 — and the caller reports a VISIBLE skip — when a needed utility
# cannot be resolved, because a harness that cannot run a case must say so
# instead of failing it.
NO_JSCPD_SKIP_REASON=""
run_gate_no_jscpd() {
  local bare="$SCRATCH/stub-no-jscpd" missing="" tool resolved
  if [ ! -d "$bare" ]; then
    mkdir -p "$bare"
    for tool in "$STUB_DIR"/*; do
      case "$(basename "$tool")" in jscpd) continue ;; esac
      ln -s "$tool" "$bare/$(basename "$tool")"
    done
    for tool in bash sh date sed awk grep git mktemp realpath readlink find \
      sort uniq cut tr tail head basename dirname cat rm cp mv ls mkdir touch \
      chmod diff wc env python3 stat od tee xargs; do
      resolved="$(command -v "$tool" 2>/dev/null)" || resolved=""
      case "$resolved" in
        /*) [ -e "$bare/$tool" ] || ln -s "$resolved" "$bare/$tool" ;;
        *) missing+="$tool " ;;
      esac
    done
    [ -z "$missing" ] || printf '%s' "${missing% }" >"$SCRATCH/stub-no-jscpd.missing"
  fi
  if [ -f "$SCRATCH/stub-no-jscpd.missing" ]; then
    NO_JSCPD_SKIP_REASON="cannot build a jscpd-free PATH: unresolved $(cat "$SCRATCH/stub-no-jscpd.missing")"
    return 1
  fi
  run_gate_path "$bare" "$@"
  return 0
}

echo "fast-gate semantics: the lane decision must fail closed"

# ── 1. control: a documentation-only change still takes the docs lane ─────────
# NEGATION: pre-fix there was no harness for the fast gate at all, so a
# regression in the lane decision (A1, A2, B45) could ship behind a green CI
# and nobody would know. This case is the control that proves the fixture
# really drives the gate: it must be GREEN here, or every other case in this
# suite would be measuring nothing.
D="$(make_fixture control main real)"
touch_worktree "$D"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -eq 0 ]; then
  ok "control: a documentation-only change is GREEN"
else
  bad "control: a documentation-only change failed (rc=$GATE_RC): $(tail -3 "$GATE_OUT")"
fi
has "$GATE_OUT" "fast-gate lane: docs" "control: the gate selected the docs lane"
has "$GATE_OUT" "fast-gate: GREEN" "control: the gate reported GREEN"

# ── 2. A1: a classifier that crashes must fail the gate and widen the run ────
# NEGATION: pre-fix the classifier ran as a bare command, so its exit status
# was discarded: a crash left every flag at its default and the dispatch fell
# through to the cheap docs+CI lanes — no cargo, green gate, nothing verified.
D="$(make_fixture a1-crash main crash)"
touch_worktree "$D"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -ne 0 ]; then
  ok "A1: a crashing classifier makes the gate fail"
else
  bad "A1: a crashing classifier left the gate GREEN"
fi
has "$GATE_ERR" "::error::path classifier exited non-zero" \
  "A1: the gate reports the classifier's non-zero exit"
has "$GATE_OUT" "  - path classification" "A1: 'path classification' is a failed step"
has "$GATE_OUT" "fast-gate lane: FULL" "A1: the gate widened to the FULL lane"
lacks "$GATE_OUT" "fast-gate lane: docs" "A1: the gate did not fall through to the docs lane"
lacks "$GATE_OUT" "fast-gate lane: CI-ONLY" "A1: the gate did not fall through to the CI lane"

# ── 3. A1: a verdict with a missing key is an untrustworthy verdict ──────────
# NEGATION: pre-fix a classifier that wrote fewer keys than the gate read
# left every absent key at the default `false` — a verdict the classifier
# never gave, indistinguishable from a real one.
D="$(make_fixture a1-partial main partial)"
touch_worktree "$D"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -ne 0 ]; then
  ok "A1: an incomplete verdict makes the gate fail"
else
  bad "A1: an incomplete verdict left the gate GREEN"
fi
has "$GATE_ERR" "::error::path classifier output is missing" \
  "A1: the gate names the missing keys"
has "$GATE_OUT" "  - path classification" "A1: 'path classification' is a failed step"
has "$GATE_OUT" "fast-gate lane: FULL" "A1: the gate widened to the FULL lane"

# ── 4. A2: the docs-only re-assertion must really match the code files ───────
# NEGATION: pre-fix the pattern was over-escaped and reached grep as
# `\\.rs$` — a literal backslash plus any character — so it matched NOTHING:
# a stray foo.rs, Cargo.toml or Cargo.lock passed the assert and the docs
# lane ran as if the tree were pure documentation.
assert_a2_catches() {
  local fixture_name="$1" file_name="$2" label="$3"
  local d
  d="$(make_fixture "$fixture_name" main docs)"
  case "$file_name" in
    *.rs) printf 'fn main() {}\n' >"$d/$file_name" ;;
    Cargo.toml) printf '[package]\nname = "fixture"\nversion = "0.0.0"\n' >"$d/$file_name" ;;
    Cargo.lock) printf 'version = 3\n' >"$d/$file_name" ;;
    *) printf 'probe\n' >"$d/$file_name" ;;
  esac
  run_gate "$d" --base-ref HEAD --head-ref HEAD
  if [ "$GATE_RC" -ne 0 ]; then
    ok "A2: the gate fails with $file_name in the worktree"
  else
    bad "A2: the gate stayed GREEN with $file_name in the worktree"
  fi
  has "$GATE_OUT" "::error::FAIL: assert no code files in worktree diff" \
    "A2: the docs-only re-assertion caught $file_name ($label)"
  has "$GATE_OUT" "  - assert no code files in worktree diff" \
    "A2: the failed step is named for $file_name ($label)"
  has "$GATE_OUT" "fast-gate lane: docs" \
    "A2: the docs lane ran, so the strict assertion was reached ($label)"
}
assert_a2_catches a2-rs foo.rs "a Rust source file"
assert_a2_catches a2-cargo-toml Cargo.toml "a workspace manifest"
assert_a2_catches a2-cargo-lock Cargo.lock "a lockfile"

# The same re-assertion, reached with the REAL classifier this time: a path
# the classifier considers documentation-eligible (a nested .github/ directory
# is not the CI surface it matches) but the gate's pattern must still treat as
# code surface. Without this case the suite would only prove the assertion
# works against a stub, and "the assertion can never fire" would look like a
# licence to delete it.
D="$(make_fixture a2-real-classifier main real)"
mkdir -p "$D/docs/.github"
printf 'not really an image\n' >"$D/docs/.github/diagram.png"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -ne 0 ]; then
  ok "A2: the gate fails on a code-surface path the real classifier called docs"
else
  bad "A2: the gate stayed GREEN on a code-surface path the real classifier called docs"
fi
has "$GATE_OUT" "::error::FAIL: assert no code files in worktree diff" \
  "A2: the re-assertion fired with the REAL classifier"
has "$GATE_OUT" "fast-gate lane: docs" \
  "A2: the docs lane ran with the real classifier, so the strict assertion was reached"

# ── 5. B45: an unresolvable base ref must widen the run, never narrow it ─────
# NEGATION: pre-fix the base resolution fell back to HEAD~1, which is not the
# PR base: the branch TIP got classified instead of the branch, so a branch
# that also touched Rust read as documentation-only and skipped cargo.
D="$(make_fixture b45 topic real)"
touch_worktree "$D"
run_gate "$D" --base-ref origin/main --head-ref HEAD
if [ "$GATE_RC" -ne 0 ]; then
  ok "B45: an unresolvable base makes the gate fail"
else
  bad "B45: an unresolvable base left the gate GREEN"
fi
has "$GATE_ERR" "widening to FULL" "B45: the gate says it is widening to FULL"
has "$GATE_OUT" "fast-gate lane: FULL" "B45: the gate selected the FULL lane"
lacks "$GATE_OUT" "fast-gate lane: docs" "B45: the gate did not narrow to the docs lane"
has "$GATE_OUT" "  - base resolution" "B45: 'base resolution' is a failed step"
lacks "$GATE_OUT" "  - path classification" \
  "B45: the classification itself was not the failure"
# The classifier half of B45: asked directly about the same unresolvable
# base, it must report the unknown scope instead of guessing a narrower one.
CLS_OUT="$SCRATCH/classifier.out"
(
  cd "$D" || exit 127
  bash scripts/ci_path_classifier.sh --base-ref origin/main --head-ref HEAD --format github
) >"$CLS_OUT" 2>/dev/null
has "$CLS_OUT" "all=true" "B45: the classifier widens to all=true on an unresolvable base"

# ── 6. B45: a local `main` is NOT a substitute for the requested base ──────
# NEGATION: the fallback chain was "<base> -> main -> HEAD~1". The HEAD~1 half
# is gone, but the `main` half was not: a local main can be stale or AHEAD
# of the real base, so the diff would not be the PR's range — and a branch
# whose tip is a documentation commit read as documentation-only, so the
# Rust change was never compiled. The requested base is the only candidate.
D="$(make_fixture b45-main-exists main real)"
# A branch whose tip is a documentation commit: the old fallback to the local
# `main` would diff against main and classify that commit as docs-only.
fixture_git "$D" checkout -q -b topic
printf 'more\n' >>"$D/docs/guide.rst"
fixture_git "$D" commit -qam "fixture: a documentation commit on top of main"
touch_worktree "$D"
run_gate "$D" --base-ref origin/nonexistent --head-ref HEAD
if [ "$GATE_RC" -ne 0 ]; then
  ok "B45: an unresolvable base fails the gate even when a local main exists"
else
  bad "B45: an unresolvable base left the gate GREEN (the local main was used as the base)"
fi
has "$GATE_ERR" "widening to FULL" "B45: the gate says it is widening to FULL"
has "$GATE_OUT" "fast-gate lane: FULL" "B45: the gate selected the FULL lane"
has "$GATE_OUT" "  - base resolution" "B45: 'base resolution' is a failed step"
lacks "$GATE_OUT" "fast-gate lane: docs" "B45: the local main did not narrow the lane to docs"
# The classifier half: asked about the same unresolvable base, it must report
# the unknown scope instead of diffing against the local main.
CLS_OUT="$SCRATCH/classifier-main-exists.out"
(
  cd "$D" || exit 127
  bash scripts/ci_path_classifier.sh --base-ref origin/nonexistent --head-ref HEAD --format github
) >"$CLS_OUT" 2>/dev/null
has "$CLS_OUT" "all=true" "B45: the classifier widens to all=true even when a local main exists"
lacks "$CLS_OUT" "docs_only=true" "B45: the classifier did not diff against the local main"

# ── 7. B51: a bash file WITHOUT the .sh suffix is linted, not skipped ───────
# NEGATION: pre-fix the changed-scripts list came from `^scripts/.*\.sh$`.
# `scripts/githooks/pre-push` is bash WITH a shebang and WITHOUT the .sh
# suffix, so it was classified `ci_only` and then skipped as "no changed
# scripts": `bash -n` and shellcheck never saw it, and the local gate linted
# LESS than CI's repo-guards job lints.
D="$(make_fixture b51-githook main real)"
mkdir -p "$D/scripts/githooks"
printf '#!/usr/bin/env bash\nexit 0\n' >"$D/scripts/githooks/pre-push"
fixture_git "$D" add -A
fixture_git "$D" commit -q -m "fixture: a git hook without the .sh suffix"
printf '# touched\n' >>"$D/scripts/githooks/pre-push"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -eq 0 ]; then
  ok "B51: a touched git hook leaves the gate GREEN"
else
  bad "B51: a touched git hook failed the gate (rc=$GATE_RC): $(tail -3 "$GATE_OUT")"
fi
has "$GATE_OUT" "PASS: bash syntax (changed scripts)" \
  "B51: the git hook went through the syntax step"
has "$GATE_OUT" "PASS: shellcheck (changed scripts" \
  "B51: the git hook went through the shellcheck step"
lacks "$GATE_OUT" "SKIP: shell syntax/shellcheck" \
  "B51: the git hook was not skipped as 'no changed scripts'"
# The control: a .sh file under scripts/ was linted before the fix, so its
# presence here proves the new path did not break the old one.
D="$(make_fixture b51-control main real)"
printf '#!/usr/bin/env bash\nexit 0\n' >"$D/scripts/ordinary.sh"
fixture_git "$D" add -A
fixture_git "$D" commit -q -m "fixture: an ordinary .sh script"
printf '# touched\n' >>"$D/scripts/ordinary.sh"
run_gate "$D" --base-ref HEAD --head-ref HEAD
has "$GATE_OUT" "PASS: bash syntax (changed scripts)" \
  "B51: the control .sh file still goes through the syntax step"

# ── 8. B52: the FULL lane runs the CI lane's semantics harnesses ────────────
# NEGATION: pre-fix `all=true` ran `lane_full`, which never ran the CI lane's
# harnesses (the path-classifier, compatibility and target-isolation ones)
# while its banner claimed "nothing skipped". A FULL run therefore verified
# LESS than a cheap CI run of the same change.
D="$(make_fixture b52-full main full)"
touch_worktree "$D"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -eq 0 ]; then
  ok "B52: the FULL lane is GREEN with every harness wired"
else
  bad "B52: the FULL lane failed (rc=$GATE_RC): $(tail -3 "$GATE_OUT")"
fi
has "$GATE_OUT" "fast-gate lane: FULL" "B52: the gate selected the FULL lane"
has "$GATE_OUT" "PASS: path classifier harness semantics (offline)" \
  "B52: the FULL lane runs the path-classifier harness"
has "$GATE_OUT" "PASS: compatibility harness semantics (offline)" \
  "B52: the FULL lane runs the compatibility harness"
has "$GATE_OUT" "PASS: target isolation guard semantics (offline)" \
  "B52: the FULL lane runs the target-isolation harness"
# The control: the cheap CI lane runs the same harness when it is triggered.
D="$(make_fixture b52-ci main real)"
printf '# touched\n' >>"$D/scripts/ci_path_classifier.sh"
run_gate "$D" --base-ref HEAD --head-ref HEAD
has "$GATE_OUT" "PASS: path classifier harness semantics (offline)" \
  "B52: the control CI lane still runs the path-classifier harness"

# ── 9. B53: a scan that could NOT run is a failure, not a clean result ─────
# NEGATION: pre-fix the pinned grep arms were `if grep ...; then fail; fi;
# echo OK`, which cannot tell grep's exit 1 (nothing found) from its exit 2
# (the scan could not run): an unreadable source tree printed the clean
# message and the step PASSED.
D="$(make_fixture b53-scan main full)"
touch_worktree "$D"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -eq 0 ]; then
  ok "B53: the FULL lane is GREEN when the scan can run"
else
  bad "B53: the FULL lane failed with a readable source tree (rc=$GATE_RC): $(tail -3 "$GATE_OUT")"
fi
has "$GATE_OUT" "PASS: forbid anyhow in webfang_core" \
  "B53: the pinned scan passed on a readable tree"
lacks "$GATE_OUT" "::error::FAIL: forbid anyhow in webfang_core" \
  "B53: the pinned scan did not fail on a readable tree"
# Now the source tree cannot be read: the scan does not run, which is a
# FAILURE and never "nothing found".
chmod 000 "$D/crates/webfang_core/src"
run_gate "$D" --base-ref HEAD --head-ref HEAD
chmod 755 "$D/crates/webfang_core/src"
if [ "$GATE_RC" -ne 0 ]; then
  ok "B53: an unreadable source tree makes the gate fail"
else
  bad "B53: an unreadable source tree left the gate GREEN"
fi
has "$GATE_OUT" "::error::FAIL: forbid anyhow in webfang_core" \
  "B53: the unreadable tree failed the scan step"
has "$GATE_OUT" "  - forbid anyhow in webfang_core" \
  "B53: the failed scan step is named in the summary"

# ── 10. #2559: the two merge-blocking checks the gate used to skip ───────────
# NEGATION: pre-fix neither `scripts/check_duplication.sh` (merge-blocking in
# the `code-quality` job) nor `scripts/check_test_inventory.sh` (merge-blocking
# in `repo-guards`) appeared anywhere in the gate. Measured on #2548 the gate
# printed 37 pass / 0 fail — GREEN — while BOTH were red in CI, so the local
# cycle the gate exists to save was spent twice on the same change.
#
# The two scripts are STUBBED here: the case under test is the WIRING, and a
# real jscpd over a fixture would measure the fixture, not the gate.
D="$(make_fixture cq-code-lane main real)"
printf '#!/usr/bin/env bash\necho "fixture: duplication stub"\nexit 0\n' \
  >"$D/scripts/check_duplication.sh"
printf '#!/usr/bin/env bash\necho "fixture: inventory stub"\nexit 0\n' \
  >"$D/scripts/check_test_inventory.sh"
fixture_git "$D" add -A
fixture_git "$D" commit -q -m "fixture: the two stubbed checks"
# A Rust edit is what reaches the CODE lane, and it is also the shape that
# staled the inventory in #2548 (a new test function).
printf '// touched\n' >>"$D/crates/webfang_core/src/lib.rs"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -eq 0 ]; then
  ok "#2559: the code lane is GREEN with both checks present"
else
  bad "#2559: the code lane failed (rc=$GATE_RC): $(tail -3 "$GATE_OUT")"
fi
has "$GATE_OUT" "PASS: duplication ratchet (jscpd v5, #2559)" \
  "#2559: the code lane runs the duplication ratchet"
has "$GATE_OUT" "PASS: test inventory drift (#2559)" \
  "#2559: the code lane runs the test-inventory drift check"

# And a red one really reds the gate: the wiring must not be a step that
# cannot fail.
D="$(make_fixture cq-code-red main real)"
printf '#!/usr/bin/env bash\necho "fixture: duplication INCREASED" >&2\nexit 1\n' \
  >"$D/scripts/check_duplication.sh"
printf '#!/usr/bin/env bash\nexit 0\n' >"$D/scripts/check_test_inventory.sh"
fixture_git "$D" add -A
fixture_git "$D" commit -q -m "fixture: a duplication stub that fails"
printf '// touched\n' >>"$D/crates/webfang_core/src/lib.rs"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -ne 0 ]; then
  ok "#2559: a red duplication ratchet reds the gate"
else
  bad "#2559: a red duplication ratchet left the gate GREEN"
fi
has "$GATE_OUT" "::error::FAIL: duplication ratchet" \
  "#2559: the failing ratchet is reported as a failed step"
has "$GATE_OUT" "  - duplication ratchet (jscpd v5, #2559)" \
  "#2559: the failed ratchet is named in the summary"

# A missing jscpd is a VISIBLE SKIP, never a silent pass and never a red gate:
# the gate's documented contract is fail-open on a missing tool. Reached with a
# PATH that carries no jscpd at all — a machine that HAS the real binary would
# otherwise take the run branch and the case would measure nothing.
D="$(make_fixture cq-no-jscpd main real)"
printf '#!/usr/bin/env bash\nexit 0\n' >"$D/scripts/check_duplication.sh"
# Both code-lane checks are stubbed, like the two sibling cases: the inventory
# check does not depend on jscpd, and leaving it unstubbed would let an
# unrelated step decide the verdict.
printf '#!/usr/bin/env bash\nexit 0\n' >"$D/scripts/check_test_inventory.sh"
fixture_git "$D" add -A
fixture_git "$D" commit -q -m "fixture: a duplication stub that never runs"
printf '// touched\n' >>"$D/crates/webfang_core/src/lib.rs"
if run_gate_no_jscpd "$D" --base-ref HEAD --head-ref HEAD; then
  if [ "$GATE_RC" -eq 0 ]; then
    ok "#2559: a missing jscpd does not red the gate"
  else
    bad "#2559: a missing jscpd failed the gate (rc=$GATE_RC): $(tail -3 "$GATE_OUT")"
  fi
  has "$GATE_OUT" "SKIP: duplication ratchet (jscpd v5)" \
    "#2559: the missing jscpd is a logged skip, not a silent pass"
  lacks "$GATE_OUT" "PASS: duplication ratchet" \
    "#2559: the duplication ratchet did not report a pass it never earned"
else
  skip "#2559: the fail-open case cannot run here ($NO_JSCPD_SKIP_REASON)"
fi

# ── 11. #2559: the coverage report states only what the lane really verified ──
# NEGATION: pre-fix the gate's summary implied full coverage of the merge gate,
# two merge-blocking jobs were missing from it entirely, and nothing said so.
# The report is what makes the remaining gap legible instead of invisible — so
# every claim it makes is pinned here, one assertion per claim. A report that
# over-states coverage is worse than no report: it is read and believed.
D="$(make_fixture coverage-code main real)"
printf '// touched\n' >>"$D/crates/webfang_core/src/lib.rs"
run_gate "$D" --base-ref HEAD --head-ref HEAD
has "$GATE_OUT" "fast-gate coverage vs gate.needs (lanes: code)" \
  "#2559: the code lane prints the coverage report"
# A note is kept for a COVERED job (finding: notes were dropped for covered),
# and a lane list written `code, full` means covered here too (finding: the
# gate matched the raw field while the validator normalised the space).
has "$GATE_OUT" "  covered here: change-scope (the path classifier), fmt (cargo fmt --all -- --check)" \
  "#2559: covered jobs keep their note, and a spaced lane list still counts"
# `test-core` is PARTIAL in the code lane: only the classifier-selected crates
# run. Reporting it as covered was the green-local/red-CI gap, re-endorsed.
has "$GATE_OUT" "  partial here: test-core (nextest for the selected crates only), repo-guards (a subset of the CI steps)" \
  "#2559: a job the lane covers only in part is reported as partial, with the gap named"
# A note is kept for a NOT-covered job (finding: the explanation was dropped
# exactly where it matters most), and `security` is NOT in the never-covered
# list because the cached zizmor audit does run in some lanes.
has "$GATE_OUT" "  NOT covered here: security (the cached zizmor audit runs; cargo deny never does), toolchain (no local equivalent)" \
  "#2559: not-covered jobs keep their note"
has "$GATE_OUT" "  no lane covers these, ever: toolchain (no local equivalent)" \
  "#2559: a job NO lane covers is called out separately, with its reason"
lacks "$GATE_OUT" "no lane covers these, ever: security" \
  "#2559: a partially covered job is never listed as never covered"

# The same table, a different lane: the report is lane-aware, not a constant,
# and `test-core` is FULL in the full lane (workspace nextest).
D="$(make_fixture coverage-full main full)"
touch_worktree "$D"
run_gate "$D" --base-ref HEAD --head-ref HEAD
has "$GATE_OUT" "fast-gate coverage vs gate.needs (lanes: full)" \
  "#2559: the full lane prints its own coverage report"
has "$GATE_OUT" "test-core (workspace nextest)" \
  "#2559: test-core is covered in the full lane, where the workspace run happens"

# And in the CI lane, `security` is partial — the zizmor audit is the part of
# that job a pre-push run really does.
D="$(make_fixture coverage-ci main real)"
printf '# touched\n' >>"$D/scripts/ci_path_classifier.sh"
run_gate "$D" --base-ref HEAD --head-ref HEAD
has "$GATE_OUT" "fast-gate coverage vs gate.needs (lanes: ci)" \
  "#2559: the CI lane prints its own coverage report"
has "$GATE_OUT" "  partial here: security (the cached zizmor audit runs; cargo deny never does)" \
  "#2559: the CI lane reports the zizmor half of the security job as partial"

# The docs lane covers almost nothing, and says so instead of staying silent.
# `change-scope` is covered there (the classifier runs in every lane) and every
# other row is not — the same shape the real table produces.
D="$(make_fixture coverage-docs main real)"
touch_worktree "$D"
run_gate "$D" --base-ref HEAD --head-ref HEAD
has "$GATE_OUT" "fast-gate coverage vs gate.needs (lanes: docs)" \
  "#2559: the docs lane prints its own coverage report"
has "$GATE_OUT" "  covered here: change-scope (the path classifier)" \
  "#2559: the docs lane reports the one job it really covers"
has "$GATE_OUT" "  NOT covered here: fmt, test-core (" \
  "#2559: the docs lane names the cargo jobs it does not run"

# The declaration is not a measurement, and the report says so on the LAST
# screen — the SKIP lines that make it a non-measurement scrolled past earlier.
has "$GATE_OUT" "  declaration, not a measurement:" \
  "#2559: the report says it is a declaration, not a measurement"

# An unreadable table is UNKNOWN, never an empty gap: "I cannot name the gap"
# and "there is no gap" must not read the same.
D="$(make_fixture coverage-unknown main real)"
rm "$D/scripts/ci_fast_gate_coverage.tsv"
fixture_git "$D" add -A
fixture_git "$D" commit -q -m "fixture: the coverage table is gone"
touch_worktree "$D"
run_gate "$D" --base-ref HEAD --head-ref HEAD
has "$GATE_OUT" "fast-gate coverage: UNKNOWN" \
  "#2559: a missing coverage table is reported as UNKNOWN"
lacks "$GATE_OUT" "  NOT covered here: none" \
  "#2559: a missing coverage table did not claim a complete gap list"
if [ "$GATE_RC" -eq 0 ]; then
  ok "#2559: the coverage report never reds the gate"
else
  bad "#2559: the coverage report failed the gate (rc=$GATE_RC)"
fi

# ── 12. the resource contract: nothing was compiled, nothing was real ─────────
# NEGATION: pre-fix no harness existed, so nothing observed what the gate
# actually invoked. A harness that quietly reached a real cargo would turn
# the suite into the resource accident it is supposed to prevent.
if [ -s "$STUB_LOG" ]; then
  ok "resource: the gate's tool invocations were recorded ($(
    wc -l <"$STUB_LOG" | tr -d ' ') call(s))"
else
  bad "resource: no tool invocation was recorded — the assertion would be vacuous"
fi
STUB_OUTSIDE=0
while IFS= read -r line; do
  stub_path="$(printf '%s' "$line" | cut -f2)"
  case "$stub_path" in
    # `stub-no-jscpd/` holds symlinks to the same scripts, for the one case
    # that must run with no jscpd anywhere on PATH.
    "$STUB_DIR"/* | "$SCRATCH/stub-no-jscpd"/*) ;;
    *) STUB_OUTSIDE=$((STUB_OUTSIDE + 1)) ;;
  esac
done <"$STUB_LOG"
if [ "$STUB_OUTSIDE" -eq 0 ]; then
  ok "resource: every recorded invocation came from the scratch stub"
else
  bad "resource: $STUB_OUTSIDE invocation(s) did not come from the scratch stub"
fi
if [ -z "$(find "$TARGET_DIR" -mindepth 1 -print -quit 2>/dev/null)" ]; then
  ok "resource: the cargo target dir is still empty"
else
  bad "resource: the cargo target dir is not empty: $(find "$TARGET_DIR" -mindepth 1 | head -3)"
fi
FIXTURE_TARGETS=0
for d in "$SCRATCH"/fixtures/*; do
  [ -d "$d" ] || continue
  if [ -d "$d/target" ]; then FIXTURE_TARGETS=$((FIXTURE_TARGETS + 1)); fi
done
if [ "$FIXTURE_TARGETS" -eq 0 ]; then
  ok "resource: no fixture grew a target dir"
else
  bad "resource: $FIXTURE_TARGETS fixture(s) grew a target dir"
fi

# ── 13. A15: a DELETED script must not fail the syntax steps ───────────────────
# NEGATION: pre-fix the changed-scripts list came straight from the changed-file
# union, and `git diff --name-only` reports DELETED paths too. `bash -n` and
# the linter were then handed a file that is not on disk: retiring one script
# made the local gate fail twice with "No such file or directory" — a
# legitimate deletion reported as a defect, blocking the pre-push. Measured
# 2026-10-20. A path that is gone is not a script to lint.
#
# (A comment line that BEGINS with "shellcheck" is read as a directive, not as
# prose — this one used to start that way and CI rejected it with SC1072.)
D="$(make_fixture a15-deleted main real)"
printf '#!/usr/bin/env bash\ntrue\n' >"$D/scripts/keep.sh"
printf '#!/usr/bin/env bash\ntrue\n' >"$D/scripts/doomed.sh"
fixture_git "$D" add -A
fixture_git "$D" commit -q -m "fixture: two scripts, one of them about to be retired"
# The survivor is edited (so it is in the union AND on disk) and the other is
# deleted (so it is in the union and NOT on disk).
printf '# edited\n' >>"$D/scripts/keep.sh"
rm "$D/scripts/doomed.sh"
run_gate "$D" --base-ref HEAD --head-ref HEAD
if [ "$GATE_RC" -eq 0 ]; then
  ok "A15: retiring a script leaves the gate GREEN"
else
  bad "A15: retiring a script failed the gate (rc=$GATE_RC): $(tail -3 "$GATE_OUT")"
fi
has "$GATE_OUT" "PASS: bash syntax (changed scripts)" \
  "A15: the syntax step ran on the surviving script and passed"
has "$GATE_OUT" "PASS: shellcheck (changed scripts" \
  "A15: the shellcheck step ran on the surviving script and passed"
lacks "$GATE_OUT" "::error::FAIL: bash syntax" \
  "A15: the deleted script did not fail the syntax step"
lacks "$GATE_OUT" "::error::FAIL: shellcheck" \
  "A15: the deleted script did not fail the shellcheck step"

ELAPSED=$(( $(date +%s) - START_SECONDS ))
if [ "$ELAPSED" -lt 60 ]; then
  ok "resource: the harness finished in ${ELAPSED}s (ceiling: 60s)"
else
  bad "resource: the harness took ${ELAPSED}s (ceiling: 60s)"
fi

printf '\nstub log (%s):\n' "$STUB_LOG"
cat "$STUB_LOG"
printf 'cargo target dir: %s (%s file(s))\n' "$TARGET_DIR" \
  "$(find "$TARGET_DIR" -mindepth 1 2>/dev/null | wc -l | tr -d ' ')"

printf '\nharness: %d ok, %d fail, %d skip\n' "$PASS" "$FAIL" "$SKIP"
[ "$FAIL" -eq 0 ] || exit 1
echo "the lane decision fails closed: a broken classifier, an incomplete verdict, a stray code file and an unresolvable base all widen or fail the run"
