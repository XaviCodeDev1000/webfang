#!/usr/bin/env bash
#
# test_mutation_ssrf_scope.sh — meta-guard for the mutation scope of the SSRF
# surface (#3105, Guard 2).
#
# WHAT IT ASSERTS. A property of the CONFIGURATION, not of a test run: the two
# modules that decide an SSRF verdict are inside `.cargo/mutants.toml`'s
# `examine_globs`, the SINGLE source of the mutation scope (`.github/workflows/
# mutants.yml` keeps no second copy). It deliberately does NOT prove that a
# mutant gets caught — whether a surviving mutant is a real defect is what the
# mutation run answers, and no config assertion can stand in for it.
#
#   crates/webfang_core/src/domain/ssrf_guard.rs          redirect_policy, secure_client
#   crates/webfang_core/src/infrastructure/ssrf.rs        ValidatingResolver
#
# WHY THIS EXISTS. Before this guard, `examine_globs` covered
# `infrastructure/downloader/**` and `infrastructure/network/**` but neither
# module above, so the SSRF surface was green BY OMISSION: those mutants were
# outside the examination set in the nightly whole-crate run as well, not merely
# diff-scoped by the required PR job.
#
# PARSING — order-, whitespace- and comment-independent: the array is read by a
# state machine that follows the `examine_globs =` key and the closing bracket,
# strips `#` comments before extracting quoted strings, and never refers to a
# line number or an entry position. Every entry in the file is either a literal
# path or `dir/**`, the two shapes where bash's `[[ path == pattern ]]` gives the
# same verdict as cargo-mutants. Removing an entry, renaming a module so no glob
# matches it, or listing a required module in `exclude_globs` (the exclusion
# wins) turns it red NAMING the module that fell out of scope.
#
# Usage:
#   bash scripts/tests/test_mutation_ssrf_scope.sh            # the repo
#   bash scripts/tests/test_mutation_ssrf_scope.sh --root DIR # a fixture tree

set -uo pipefail

SELF="${BASH_SOURCE[0]}"
DEFAULT_ROOT="$(cd "$(dirname "$SELF")/../.." && pwd)"

TARGET_ROOT=""
EXPLICIT_ROOT=0
while [[ $# -gt 0 ]]; do
  case "$1" in
  --root)
    if [[ -z "${2:-}" ]]; then
      echo "::error::--root requires a directory" >&2
      exit 2
    fi
    TARGET_ROOT="$2"
    EXPLICIT_ROOT=1
    shift 2
    ;;
  *)
    echo "unknown argument: $1" >&2
    exit 2
    ;;
  esac
done
[[ -n "$TARGET_ROOT" ]] || TARGET_ROOT="$DEFAULT_ROOT"

# The SSRF decision surface. `wreq_downloader.rs` APPLIES the guard, it is not
# where the verdict is decided, and `infrastructure/downloader/**` already has it.
REQUIRED_SSRF_MODULES=(
  "crates/webfang_core/src/domain/ssrf_guard.rs"
  "crates/webfang_core/src/infrastructure/ssrf.rs"
)

pass=0
fail=0
ok() { echo "  ok   $*"; pass=$((pass + 1)); }
bad() {
  echo "  FAIL $*" >&2
  fail=$((fail + 1))
}

# array_body <field> <toml>: the raw lines inside `<field> = [ ... ]` (flat TOML,
# as the file's own header says).
array_body() {
  awk -v field="$1" '
    $0 ~ "^[[:space:]]*" field "[[:space:]]*=" { inside = 1; next }
    inside && $0 ~ "^[[:space:]]*\\]" { inside = 0; next }
    inside { print }
  ' "$2"
}

# array_entries <field> <toml>: comments are stripped FIRST, so a quoted string
# inside a comment can never be mistaken for a scope entry.
array_entries() {
  array_body "$1" "$2" | sed -E 's/#.*$//' | grep -oE '"[^"]*"' | tr -d '"' || true
}

# matches_any <path> <glob>...: bash `==` pattern-matches the unquoted expansion.
matches_any() {
  local path="$1"
  shift
  local pat
  for pat in "$@"; do
    # shellcheck disable=SC2053 # the RHS is a glob on purpose; see the header
    if [[ $path == $pat ]]; then
      return 0
    fi
  done
  return 1
}

# quoted_comment <toml>: true when a comment inside examine_globs carries a double
# quote. Only the comment TEXT is inspected; the entry owns its own quotes.
quoted_comment() {
  array_body examine_globs "$1" | awk '
    index($0, "#") > 0 {
      text = $0
      sub(/^[^#]*#/, "", text)
      if (index(text, "\"") > 0) found = 1
    }
    END { exit(found ? 0 : 1) }
  '
}

check_scope() { # root
  local root="$1"
  local toml="$root/.cargo/mutants.toml"
  local module

  if [[ ! -r "$toml" ]]; then
    bad "cannot read $toml — the mutation scope is unknown, never assume it is fine"
    return
  fi

  local -a examine=()
  local -a exclude=()
  local entry
  while IFS= read -r entry; do
    [[ -n "$entry" ]] && examine+=("$entry")
  done < <(array_entries examine_globs "$toml")
  while IFS= read -r entry; do
    [[ -n "$entry" ]] && exclude+=("$entry")
  done < <(array_entries exclude_globs "$toml")

  if [[ "${#examine[@]}" -eq 0 ]]; then
    bad "examine_globs is empty or unparsable in $toml — the guard would assert nothing"
    return
  fi

  # The workflow derives its hot-path regex from the quoted strings of the array,
  # so a comment carrying a double quote silently changes that regex.
  if quoted_comment "$toml"; then
    bad "a comment inside examine_globs carries a double quote — the workflow derives its regex from this array ($toml)"
  else
    ok "no quoted string hides in an examine_globs comment"
  fi

  for module in "${REQUIRED_SSRF_MODULES[@]}"; do
    if ! matches_any "$module" "${examine[@]}"; then
      bad "SSRF module fell out of the mutation scope: $module is matched by no examine_globs entry ($toml)"
    elif [[ ! -f "$root/$module" ]]; then
      # Matched on paper but gone from the tree: the module was renamed and the
      # glob now guards a path nothing mutates.
      bad "SSRF module fell out of the mutation scope: $module is matched by examine_globs but no longer exists in $root (renamed?)"
    elif [[ "${#exclude[@]}" -gt 0 ]] && matches_any "$module" "${exclude[@]}"; then
      bad "SSRF module fell out of the mutation scope: $module is excluded by exclude_globs ($toml)"
    else
      ok "$module is inside examine_globs"
    fi
  done
}

echo "A: the tree's mutation scope covers the SSRF surface ($TARGET_ROOT)"
check_scope "$TARGET_ROOT"

if [ "$EXPLICIT_ROOT" -eq 1 ]; then
  if [ "$fail" -ne 0 ]; then
    echo "test_mutation_ssrf_scope: FAIL ($fail failed, $pass passed)"
    exit 1
  fi
  echo "test_mutation_ssrf_scope: PASS ($pass assertions)"
  exit 0
fi

# B: the fixture-driven directions. Each fixture is a synthetic tree holding one
# mutants.toml and the two module paths, and the harness re-invokes itself with
# --root, which keeps ONE implementation of the assertions.
echo "B: the meta-guard is sensitive in all directions"

FIXTURES="$(mktemp -d)"
trap 'rm -rf "$FIXTURES"' EXIT

# make_tree <dir> <examine-body> <exclude-body> <modules-to-create...>
make_tree() {
  local dir="$1" examine_body="$2" exclude_body="$3"
  shift 3
  mkdir -p "$dir/.cargo"
  {
    echo "examine_globs = ["
    printf '%s\n' "$examine_body"
    echo "]"
    echo ""
    echo "exclude_globs = ["
    printf '%s\n' "$exclude_body"
    echo "]"
  } >"$dir/.cargo/mutants.toml"
  local module
  for module in "$@"; do
    mkdir -p "$dir/$(dirname "$module")"
    : >"$dir/$module"
  done
}

INTACT_EXAMINE='"crates/webfang_core/src/domain/ssrf_guard.rs"
  "crates/webfang_core/src/infrastructure/ssrf.rs"'
INTACT_EXCLUDE='"**/mod.rs"'

OUT=""
RC=0
run_fixture() { # dir
  OUT="$(bash "$SELF" --root "$1" 2>&1)"
  RC=$?
}

# The intact tree first: without this row B1-B4 could be red because the harness
# is broken for an unrelated reason, which proves nothing about the guard.
make_tree "$FIXTURES/intact" "$INTACT_EXAMINE" "$INTACT_EXCLUDE" "${REQUIRED_SSRF_MODULES[@]}"
run_fixture "$FIXTURES/intact"
if [ "$RC" -eq 0 ]; then
  ok "the intact fixture tree is green"
else
  bad "the intact fixture tree is NOT green, so the rows below prove nothing (exit $RC)"
  printf '%s\n' "$OUT" >&2
fi

# B1: an entry removed -> red, naming the module it no longer covers.
make_tree "$FIXTURES/entry-removed" \
  '"crates/webfang_core/src/domain/ssrf_guard.rs"' \
  "$INTACT_EXCLUDE" \
  "${REQUIRED_SSRF_MODULES[@]}"
run_fixture "$FIXTURES/entry-removed"
if [ "$RC" -ne 0 ] && [[ "$OUT" == *"ssrf.rs"* ]] && [[ "$OUT" == *"fell out of the mutation scope"* ]]; then
  ok "a removed glob fails the guard and names infrastructure/ssrf.rs"
else
  bad "a removed glob did NOT fail the guard naming the module (exit $RC)"
  printf '%s\n' "$OUT" >&2
fi

# B2: a module renamed to a path no longer matched -> red. The file that exists
# is the renamed one; the glob still points at the old path.
make_tree "$FIXTURES/module-renamed" \
  "$INTACT_EXAMINE" \
  "$INTACT_EXCLUDE" \
  "crates/webfang_core/src/domain/ssrf_guard.rs" \
  "crates/webfang_core/src/infrastructure/ssrf/mod.rs"
run_fixture "$FIXTURES/module-renamed"
if [ "$RC" -ne 0 ] && [[ "$OUT" == *"no longer exists"* ]] && [[ "$OUT" == *"infrastructure/ssrf.rs"* ]]; then
  ok "a renamed module fails the guard and names infrastructure/ssrf.rs"
else
  bad "a renamed module did NOT fail the guard naming the module (exit $RC)"
  printf '%s\n' "$OUT" >&2
fi

# B3: a module added to exclude_globs -> red. Matched by both lists means the
# exclusion wins and the file is still never mutated, so it must not pass.
make_tree "$FIXTURES/module-excluded" \
  "$INTACT_EXAMINE" \
  '"**/mod.rs"
  "crates/webfang_core/src/infrastructure/ssrf.rs"' \
  "${REQUIRED_SSRF_MODULES[@]}"
run_fixture "$FIXTURES/module-excluded"
if [ "$RC" -ne 0 ] && [[ "$OUT" == *"excluded by exclude_globs"* ]]; then
  ok "a module added to exclude_globs fails the guard"
else
  bad "a module added to exclude_globs did NOT fail the guard (exit $RC)"
  printf '%s\n' "$OUT" >&2
fi

# B4: reordered, reindented and commented — must stay GREEN. The parser follows
# the key and the closing bracket, so a pure formatting change is not a scope
# change.
make_tree "$FIXTURES/reformatted" \
  '# scope decision (2026-11): the SSRF decision surface
	"crates/webfang_core/src/infrastructure/ssrf.rs",   # ValidatingResolver
	    "crates/webfang_core/src/domain/ssrf_guard.rs",  # redirect_policy
  # trailing comment inside the array' \
  '"**/mod.rs"' \
  "${REQUIRED_SSRF_MODULES[@]}"
run_fixture "$FIXTURES/reformatted"
if [ "$RC" -eq 0 ]; then
  ok "reordered, reindented and commented entries stay in scope"
else
  bad "a pure reformatting of the array failed the guard (exit $RC)"
  printf '%s\n' "$OUT" >&2
fi

# C: the measured cost of the extension. `--list` parses the sources, it compiles
# nothing. CAREFUL: on cargo-mutants 27.1.0 `-f/--file` is a glob UNIONED with
# `examine_globs`, not an intersection, so the count is taken per module path and
# never from the line total (the whole scope). Reported, never asserted.
echo "C: mutant count for the SSRF surface"
if command -v cargo-mutants >/dev/null 2>&1; then
  for module in "${REQUIRED_SSRF_MODULES[@]}"; do
    if LIST_OUT="$(cd "$TARGET_ROOT" && cargo mutants --list --file "$module" 2>&1)"; then
      COUNT="$(printf '%s\n' "$LIST_OUT" | grep -c "^$module:" || true)"
      echo "  info cargo mutants --list --file $module -> ${COUNT:-0} mutant(s) in that module"
    else
      echo "  info cargo mutants --list --file $module -> FAILED (count not measured)"
      printf '%s\n' "$LIST_OUT" | tail -5
    fi
  done
else
  echo "  info cargo-mutants is not installed here — the mutant count was NOT measured"
fi

if [ "$fail" -ne 0 ]; then
  echo "test_mutation_ssrf_scope: FAIL ($fail failed, $pass passed)"
  exit 1
fi
echo "test_mutation_ssrf_scope: PASS ($pass assertions)"
exit 0

# NEGATION: removing either entry from the real examine_globs turns section A
# red and names the module it stopped covering.
# NEGATION: renaming either module to a path examine_globs does not match turns
# it red ("no longer exists" — matched on paper, gone from the tree).
# NEGATION: listing a required module in exclude_globs turns it red, because the
# exclusion wins over the examination.
# NEGATION: an empty or unreadable examine_globs turns it red instead of passing
# on an empty pattern set.
