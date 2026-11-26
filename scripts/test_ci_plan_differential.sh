#!/usr/bin/env bash
# scripts/test_ci_plan_differential.sh — differential golden test for the
# Rust `ci_plan` binary against `scripts/ci_plan.sh` (#2282 / phase 3).
#
# Both planners run against the same throwaway git repo and identical event
# env. The 15 `key=value` lines in GITHUB_OUTPUT, stderr and the exit code must
# match byte for byte. Covers PR/merge_group/push, bad/zero SHAs, non-diff and
# unknown events, classifier failure, and the crossplatform tri-state.
#
# Run: bash scripts/test_ci_plan_differential.sh
# The binary is built on demand (`cargo build -p webfang_benchmark --bin ci_plan`).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASH_PLAN="$ROOT/scripts/ci_plan.sh"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/debug"
BIN="$BIN_DIR/ci_plan"
CLASSIFIER="$ROOT/scripts/ci_path_classifier.sh"
CROSSPLATFORM="$ROOT/scripts/ci_crossplatform_scope.sh"

if [ ! -x "$BIN" ]; then
  cargo build --manifest-path "$ROOT/Cargo.toml" -p webfang_benchmark --bin ci_plan -q || exit 2
fi

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

R="$T/repo"
mkdir -p "$R"
git -C "$R" init -q
git -C "$R" config user.email ci-plan-test@example.invalid
git -C "$R" config user.name "CI Plan Test"
git -C "$R" config commit.gpgsign false
mkdir -p "$R/crates/webfang_core/src"
printf '[workspace]\nmembers = ["crates/webfang_core"]\n' > "$R/Cargo.toml"
printf '[package]\nname = "webfang_core"\nversion = "0.0.0"\n' > "$R/crates/webfang_core/Cargo.toml"
printf 'pub fn plain() {}\n' > "$R/crates/webfang_core/src/lib.rs"
printf '# web fang\n' > "$R/README.md"
git -C "$R" add -A
git -C "$R" commit -q -m "base"
BASE="$(git -C "$R" rev-parse HEAD)"

commit_case() {
  local branch="$1" file="$2" content="$3" message="$4"
  git -C "$R" switch -q -c "$branch" "$BASE"
  mkdir -p "$R/$(dirname "$file")"
  printf '%s\n' "$content" > "$R/$file"
  git -C "$R" add -A
  git -C "$R" commit -q -m "$message"
  git -C "$R" rev-parse HEAD
}

DOCS_HEAD="$(commit_case docs-case README.md "# docs only" "docs")"
RUST_HEAD="$(commit_case rust-case crates/webfang_core/src/plain.rs 'pub fn plain_two() {}' "plain rust")"

BAD_SHA="0000000000000000000000000000000000000000"
FAKE_SHA="not-a-sha-at-all"

pass=0
fail=0

compare() {
  local label="$1" event="$2" pr_base="$3" pr_head="$4" mg_base="$5" mg_head="$6" push_before="$7" push_after="$8" classifier_override="$9" crossplatform_override="${10}"
  : > "$T/bash.out"
  : > "$T/rust.out"
  : > "$T/bash.err"
  : > "$T/rust.err"
  (
    cd "$R" || exit 2
    env GITHUB_OUTPUT="$T/bash.out" EVENT_NAME="$event" \
      PR_BASE_SHA="$pr_base" PR_HEAD_SHA="$pr_head" \
      MERGE_GROUP_BASE_SHA="$mg_base" MERGE_GROUP_HEAD_SHA="$mg_head" \
      PUSH_BEFORE_SHA="$push_before" PUSH_AFTER_SHA="$push_after" \
      CI_PLAN_CLASSIFIER="${classifier_override:-$CLASSIFIER}" \
      CI_PLAN_CROSSPLATFORM="${crossplatform_override:-$CROSSPLATFORM}" \
      bash "$BASH_PLAN" 2>"$T/bash.err"
  )
  local bash_rc=$?
  (
    cd "$R" || exit 2
    env GITHUB_OUTPUT="$T/rust.out" EVENT_NAME="$event" \
      PR_BASE_SHA="$pr_base" PR_HEAD_SHA="$pr_head" \
      MERGE_GROUP_BASE_SHA="$mg_base" MERGE_GROUP_HEAD_SHA="$mg_head" \
      PUSH_BEFORE_SHA="$push_before" PUSH_AFTER_SHA="$push_after" \
      CI_PLAN_CLASSIFIER="${classifier_override:-$CLASSIFIER}" \
      CI_PLAN_CROSSPLATFORM="${crossplatform_override:-$CROSSPLATFORM}" \
      "$BIN" 2>"$T/rust.err"
  )
  local rust_rc=$?
  if [ "$bash_rc" -eq "$rust_rc" ] \
    && cmp -s "$T/bash.out" "$T/rust.out" \
    && cmp -s "$T/bash.err" "$T/rust.err"; then
    printf '  PASS  %s\n' "$label"
    pass=$((pass + 1))
  else
    printf '  FAIL  %s (rc bash=%s rust=%s)\n' "$label" "$bash_rc" "$rust_rc"
    diff -u "$T/bash.out" "$T/rust.out" | head -30
    diff -u "$T/bash.err" "$T/rust.err" | head -20
    fail=$((fail + 1))
  fi
}

echo "ci_plan Rust differential"

# Path-aware events (docs-only vs code).
compare "PR docs-only" pull_request "$BASE" "$DOCS_HEAD" "" "" "" "" "" ""
compare "PR code" pull_request "$BASE" "$RUST_HEAD" "" "" "" "" "" ""
compare "merge_group docs-only" merge_group "" "" "$BASE" "$DOCS_HEAD" "" "" "" ""
compare "merge_group code" merge_group "" "" "$BASE" "$RUST_HEAD" "" "" "" ""
compare "push docs-only" push "" "" "" "" "$BASE" "$DOCS_HEAD" "" ""
compare "push code" push "" "" "" "" "$BASE" "$RUST_HEAD" "" ""

# Fail-closed fallbacks.
compare "push zero before" push "" "" "" "" "$BAD_SHA" "$RUST_HEAD" "" ""
compare "push missing after" push "" "" "" "" "$BASE" "" "" ""
compare "PR missing base" pull_request "" "$DOCS_HEAD" "" "" "" "" "" ""
compare "merge_group missing head" merge_group "" "" "$BASE" "" "" "" "" ""
compare "bad head sha" pull_request "$BASE" "$FAKE_SHA" "" "" "" "" "" ""
compare "bad base sha" pull_request "$FAKE_SHA" "$RUST_HEAD" "" "" "" "" "" ""
compare "schedule" schedule "" "" "" "" "" "" "" ""
compare "workflow_dispatch" workflow_dispatch "" "" "" "" "" "" "" ""
compare "unknown event" unknown_event "" "" "" "" "" "" "" ""

# Crossplatform tri-state, only exercised when run_code_jobs is true.
printf '#!/usr/bin/env bash\necho true\n' > "$T/cp-true.sh"
printf '#!/usr/bin/env bash\necho false\n' > "$T/cp-false.sh"
printf '#!/usr/bin/env bash\necho garbage\n' > "$T/cp-garbage.sh"
printf '#!/usr/bin/env bash\necho "boom" >&2\nexit 1\n' > "$T/cp-fail.sh"
compare "crossplatform true" pull_request "$BASE" "$RUST_HEAD" "" "" "" "" "" "$T/cp-true.sh"
compare "crossplatform false" pull_request "$BASE" "$RUST_HEAD" "" "" "" "" "" "$T/cp-false.sh"
compare "crossplatform garbage fails closed" pull_request "$BASE" "$RUST_HEAD" "" "" "" "" "" "$T/cp-garbage.sh"
compare "crossplatform failure fails closed" pull_request "$BASE" "$RUST_HEAD" "" "" "" "" "" "$T/cp-fail.sh"

# Classifier failure.
printf '#!/usr/bin/env bash\necho "classifier exploded" >&2\nexit 1\n' > "$T/fail-classifier.sh"
compare "classifier failure" pull_request "$BASE" "$DOCS_HEAD" "" "" "" "" "$T/fail-classifier.sh" ""

# S5/B2: `${KV[k]:-default}` treats an empty classifier value as unset.
printf '#!/usr/bin/env bash\nprintf "docs_only=\\nci_only=\\ncode_changed=\\nall=\\naffected=\\nrun_code_jobs=false\\nneeds_ai=\\nneeds_mcp=\\nneeds_mutation_hotpath=\\nsnapshot_changed=\\n"\n' > "$T/empty-values-classifier.sh"
compare "classifier empty values use defaults" pull_request "$BASE" "$DOCS_HEAD" "" "" "" "" "$T/empty-values-classifier.sh" ""

# S5/B3: sed preserves a missing final newline in the detector's stderr.
printf '#!/usr/bin/env bash\nprintf "line1\\nline2" >&2\nexit 1\n' > "$T/no-final-newline-classifier.sh"
compare "classifier stderr without trailing newline" pull_request "$BASE" "$DOCS_HEAD" "" "" "" "" "$T/no-final-newline-classifier.sh" ""

# CLI surface: help and bad argument (stdout/stderr contract).
for flag in --help -h; do
  : > "$T/bash.out"
  : > "$T/rust.out"
  : > "$T/bash.err"
  : > "$T/rust.err"
  bash "$BASH_PLAN" "$flag" > "$T/bash.out" 2> "$T/bash.err"
  bash_rc=$?
  "$BIN" "$flag" > "$T/rust.out" 2> "$T/rust.err"
  rust_rc=$?
  if [ "$bash_rc" -eq "$rust_rc" ] && cmp -s "$T/bash.out" "$T/rust.out" \
    && cmp -s "$T/bash.err" "$T/rust.err"; then
    printf '  PASS  %s\n' "$flag"
    pass=$((pass + 1))
  else
    printf '  FAIL  %s (rc bash=%s rust=%s)\n' "$flag" "$bash_rc" "$rust_rc"
    diff -u "$T/bash.out" "$T/rust.out" | head -20
    fail=$((fail + 1))
  fi
done

echo
if [ "$fail" -eq 0 ]; then
  echo "ci_plan differential: $pass passed, 0 failed"
  exit 0
fi
echo "ci_plan differential: $pass passed, $fail failed"
exit 1
