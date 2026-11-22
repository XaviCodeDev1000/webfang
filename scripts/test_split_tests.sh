#!/usr/bin/env bash
# test_split_tests.sh -- semantics harness for scripts/split_tests.sh
#
# ODD odd/tasks/file-size-split-tests.md, umbrella #2584, phase F2.
#
# The preflight asks "what is this file and what will a cut cost". The close-out
# asks "is the mission done". split_tests.sh does the mechanical step between
# them: re-parent one inline #[cfg(test)] mod into a child file with a multiset
# proof -- the test-item partition multiset(source_before) ==
# multiset(source_after) + multiset(dest) must hold exactly, so nothing is
# lost, nothing is invented, nothing is duplicated (policy 4.3 step 5, F2).
#
# WHAT THIS HARNESS PROVES, offline (mktemp sandboxes; no cargo, no network):
#
#   1. the tool exists, usage exit is 64, and --help names its interface
#      (--mod, --dest, --header-file) plus the preflight span dependency;
#   2. a green extraction: rc=0, the dest carries every moved test item exactly
#       once, the source carries none of them, the untouched sibling mod stays
#       byte-identical, the decl replaces the block, the emitted proof numbers
#       match an independent harness recount, and nothing outside source+dest
#       is written (hermeticity witness);
#   3. the decl shape follows the mission's verified manual extraction (#2950):
#       plain `mod tests;` when the dest IS the standard child file of the
#       declaring source, `#[path = "..."] mod tests;` otherwise, attributes
#       kept verbatim;
#   4. --header-file prepends verbatim import lines and adds no test items
#       (a header that adds items breaks the partition and is refused);
#   5. the dedent: the mod's inner indent is stripped once, relative indent
#       (nested mods, fn bodies) is preserved;
#   6. one defect per named cause (rc=4): mod not found, ambiguous name, a
#       target mod without cfg(test) (production mod), missing source,
#       dest == source, dest already exists, and an INDETERMINATE span is
#       refused fail-closed with the preflight's reason -- the tool never cuts
#       an unprovable block;
#   7. a broken python3 fails with a NAMED cause (rc=1), never a bare
#       traceback: the tool shells the span question out to the preflight
#       (ONE Rust lexer in the program, the preflight's; no second lexer);
#   8. the gate wiring lock lives here once the wiring lands in this same PR:
#       the .tsv repo-guards note, ci.yml and ci_fast_gate.sh name BOTH scripts
#       (#2056: a guard reachable from no workflow is a guard that does not
#       exist);
#   9. the family lint: no pipe into an early-exit grep -q in the pair
#       (issue #2925: the assertion fails despite holding);
#  10. the harness watches its own house: --negation flips one expectation so
#      the harness FAILS on a correct tree (#2056: a harness that cannot fail
#      proves nothing).
#
# HERMETIC: every scenario runs in a mktemp sandbox holding a COPY of the tool
# and of the preflight (the tool resolves its span owner as its sibling), with
# a synthetic rust fixture; only source+dest inside the sandbox are written.
#
# Usage: bash scripts/test_split_tests.sh [--negation]

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

NEGATION=0
while [ $# -gt 0 ]; do
  case "$1" in
    --negation) NEGATION=1 ;;
    -h | --help)
      sed -n '2,42p' "$0"
      exit 0
      ;;
    *)
      printf 'test_split_tests: unknown argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
  shift
done

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0
ERRF=""

ok() {
  printf '  ok   %-62s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-62s %s\n' "$1" "$2"
  if [ -n "${3:-}" ] && [ -f "$3" ]; then
    sed 's/^/      /' "$3" | head -20
  elif [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/      /' | head -20
  fi
  failed=$((failed + 1))
}

expect_rc() { # <desc> <expected> <real> [detail-file-or-text]
  if [ "$2" -eq "$3" ]; then
    ok "$1" "rc=$3"
  else
    bad "$1" "expected rc=$2, got rc=$3" "${4:-${ERRF:-}}"
  fi
}

expect_eq() { # <desc> <real> <expected>
  if [ "$2" = "$3" ]; then
    ok "$1" "$2"
  else
    bad "$1" "expected '$3', got '$2'"
  fi
}

expect_match() { # <desc> <text> <regex>  -- full consumption, NEVER grep -q
  if printf '%s\n' "$2" | grep -E -- "$3" > /dev/null; then
    ok "$1"
  else
    bad "$1" "no match: $3" "$2"
  fi
}

expect_no_match() { # <desc> <text> <regex>
  if printf '%s\n' "$2" | grep -E -- "$3" > /dev/null; then
    bad "$1" "unexpected match: $3"
  else
    ok "$1"
  fi
}

expect_count() { # <desc> <file> <regex> <expected-count>
  local got
  got="$(grep -cE -- "$3" "$2" 2>/dev/null)" || got=0
  expect_eq "$1" "${got:-0}" "$4"
}

# --- fixtures --------------------------------------------------------------------
# One production braced mod (a second span, name-selectable), one cfg(test)
# tests mod whose content forces the lexer through every trap (strings with
# braces, a raw string, a char literal, a lifetime, comments), four test items
# inside it (three at top level of the mod, one nested), and a sibling
# other_tests mod that must stay untouched. Independent recount: before=5,
# after=1, dest=4.

write_fixture() { # <path>
  mkdir -p "$(dirname "$1")"
  cat > "$1" <<'FIXTURE'
//! A synthetic crate body for the split-tests harness.

pub struct Widget {
    pub name: String,
}

pub fn build(name: &str) -> Widget {
    Widget {
        name: name.to_string(),
    }
}

pub mod prod {
    pub fn helper() -> usize {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::{build, Widget};

    // a line comment with braces: mod fake { } must not move the span
    /* a block comment with { braces } inside */
    const PATTERN: &str = "mod tests { not really }";
    const RAW: &str = r#"{ "braces": "inside a raw string }" }"#;
    const CLOSE: char = '}';

    #[test]
    fn builds_a_widget() {
        let w = build("x");
        assert_eq!(w.name, "x");
    }

    #[test]
    #[should_panic]
    fn panics_on_a_close_brace() {
        panic!("}");
    }

    #[tokio::test]
    async fn async_counts_helpers() {
        let n = prod::helper();
        assert_eq!(n, 3);
    }

    mod nested {
        #[test]
        fn nested_counts_twice() {
            assert_eq!(1 + 1, 2);
        }
    }
}

#[cfg(test)]
mod other_tests {
    #[test]
    fn stays_put() {
        assert_eq!(2 + 2, 4);
    }
}
FIXTURE
}

new_sandbox() { # <name> -- tool + span owner as COPIES (the tool resolves the
  #             preflight as its sibling), plus the fixture tree
  SB="$WORK/$1"
  rm -rf "$SB"
  mkdir -p "$SB/bin"
  cp "$SCRIPT_DIR/split_tests.sh" "$SB/bin/split_tests.sh"
  cp "$SCRIPT_DIR/file_size_preflight.sh" "$SB/bin/file_size_preflight.sh"
  TOOL="$SB/bin/split_tests.sh"
  SRC="$SB/src/lib.rs"
  write_fixture "$SRC"
  printf 'witness\n' > "$SB/witness.txt"
}

run_split() { # args... -- captures stdout+stderr
  OUT="$(bash "$TOOL" "$@" 2>"$SB/err.txt")"
  RC=$?
  ERRF="$SB/err.txt"
  ERR_ALL="$OUT$(cat "$SB/err.txt" 2>/dev/null)"
}

count_name() { # <file> <fn-name> -- occurrences of the test fn, any form
  # grep -c prints the count (0 included); only an ERROR (rc=2, e.g. a missing
  # file) leaves $got empty, and then the count is 0 by definition.
  local got
  got="$(grep -cE "(^|[^a-zA-Z0-9_])fn $2[[:space:]]*(\(|<)" "$1" 2>/dev/null)" || got="0"
  printf '%s' "${got:-0}"
}

# --- 1. usage and help ------------------------------------------------------------
new_sandbox usage
run_split
expect_rc "1.1 bare invocation is usage (rc=64)" 64 "$RC" "$ERRF"
OUT_HELP="$(bash "$TOOL" --help 2>&1)"
RC_HELP=$?
expect_rc "1.2 --help exits 0" 0 "$RC_HELP"
expect_match "1.3 --help names --mod" "$OUT_HELP" '\-\-mod'
expect_match "1.4 --help names --dest" "$OUT_HELP" '\-\-dest'
expect_match "1.5 --help names --header-file" "$OUT_HELP" '\-\-header-file'
expect_match "1.6 --help names the preflight span owner" "$OUT_HELP" 'file_size_preflight'

# --- 2. the green extraction (plain decl branch) -----------------------------------
# Dest IS the standard child file of src/lib.rs (src/lib/tests.rs), so the decl
# must be a bare `mod tests;` -- the #2950 hub-children shape.
new_sandbox green
DEST="$SB/src/lib/tests.rs"
run_split "$SRC" --mod tests --dest "$DEST"
expect_rc "2.1 green extraction exits 0" 0 "$RC" "$ERRF"
if [ "$RC" -eq 0 ]; then
  expect_match "2.2 emission names the extracted mod and span" "$ERR_ALL" 'extracted.*tests'
  expect_match "2.3 emission carries the dest" "$ERR_ALL" "dest.*src/lib/tests.rs"
  expect_match "2.4 proof: before=5 after=1 dest=4" "$ERR_ALL" 'before=5 after=1 dest=4'
  expect_eq "2.5 dest exists" "yes" "$([ -f "$DEST" ] && printf yes || printf no)"
  expect_eq "2.6 moved item once in dest: builds_a_widget" "1" "$(count_name "$DEST" builds_a_widget)"
  expect_eq "2.7 moved item once in dest: panics_on_a_close_brace" "1" "$(count_name "$DEST" panics_on_a_close_brace)"
  expect_eq "2.8 moved item once in dest: async_counts_helpers" "1" "$(count_name "$DEST" async_counts_helpers)"
  expect_eq "2.9 moved item once in dest: nested_counts_twice" "1" "$(count_name "$DEST" nested_counts_twice)"
  expect_eq "2.10 the sibling mod's item did not travel: stays_put" "0" "$(count_name "$DEST" stays_put)"
  expect_count "2.11 source after: the moved names are gone (builds_a_widget)" "$SRC" 'fn builds_a_widget' 0
  expect_count "2.12 source after: the moved names are gone (nested_counts_twice)" "$SRC" 'fn nested_counts_twice' 0
  expect_count "2.13 source after: the sibling item remains (stays_put)" "$SRC" 'fn stays_put' 1
  expect_count "2.14 source after: the decl is a bare mod tests;" "$SRC" '^mod tests;$' 1
  expect_count "2.15 source after: no #[path] on the plain branch" "$SRC" '#\[path' 0
  expect_count "2.16 source after: cfg(test) kept on the decl" "$SRC" '^#\[cfg\(test\)\]$' 2
  expect_count "2.17 source after: the prod mod is untouched" "$SRC" 'pub mod prod' 1
  expect_eq "2.18 hermeticity: the witness is untouched" "same" "$(cmp -s "$SB/witness.txt" <(printf 'witness\n') && printf same || printf diff)"
  expect_eq "2.19 hermeticity: src/ holds only lib.rs and the new child dir" "2" "$(ls -A "$SB/src" | wc -l)"
  expect_match "2.20 emission keys are the closed English list" "$ERR_ALL" '(^|	)(extracted|dest|decl|proof)(	|$)'
fi

# --- 3. the #[path] decl branch -----------------------------------------------------
# Dest OUTSIDE the standard child dir: the #2950 seam-children shape.
new_sandbox path_branch
DEST2="$SB/src/tests_moved.rs"
run_split "$SRC" --mod tests --dest "$DEST2"
expect_rc "3.1 extraction with a non-standard dest exits 0" 0 "$RC" "$ERRF"
if [ "$RC" -eq 0 ]; then
  expect_count "3.2 the decl carries #[path = \"tests_moved.rs\"]" "$SRC" '^#\[path = "tests_moved\.rs"\]$' 1
  expect_count "3.3 the decl line follows the path attr" "$SRC" '^mod tests;$' 1
  expect_count "3.4 cfg(test) kept verbatim above the path attr" "$SRC" '^#\[cfg\(test\)\]$' 2
fi

# --- 4. the header file --------------------------------------------------------------
new_sandbox header
DEST4="$SB/src/lib/tests.rs"
printf 'use super::super::outer_helper;\n' > "$SB/header.txt"
run_split "$SRC" --mod tests --dest "$DEST4" --header-file "$SB/header.txt"
expect_rc "4.1 extraction with a header exits 0" 0 "$RC" "$ERRF"
if [ "$RC" -eq 0 ]; then
  expect_count "4.2 the dest starts with the verbatim header" "$DEST4" '^use super::super::outer_helper;$' 1
  expect_match "4.3 the header added no test items (partition still 5=1+4)" "$ERR_ALL" 'before=5 after=1 dest=4'
fi

# a header that ADDS a test item breaks the partition and is refused (rc=4):
new_sandbox header_bad
DEST4B="$SB/src/lib/tests.rs"
cat > "$SB/header_bad.txt" <<'HDR'
use super::super::outer_helper;

#[test]
fn phantom_header_test() {}
HDR
run_split "$SRC" --mod tests --dest "$DEST4B" --header-file "$SB/header_bad.txt"
expect_rc "4.4 a header that adds test items is refused (rc=4)" 4 "$RC" "$ERRF"
expect_match "4.5 ... with a NAMED cause" "$ERR_ALL" 'header|phantom|partition'

# --- 5. the dedent --------------------------------------------------------------------
new_sandbox dedent
DEST5="$SB/src/lib/tests.rs"
run_split "$SRC" --mod tests --dest "$DEST5"
if [ "$RC" -eq 0 ]; then
  expect_count "5.1 mod-level items land at column 0" "$DEST5" '^#\[test\]$' 2
  expect_count "5.1b the tokio item lands at column 0 too" "$DEST5" '^#\[tokio::test\]$' 1
  expect_count "5.2 fn signatures land at column 0" "$DEST5" '^fn builds_a_widget\(\)' 1
  expect_count "5.3 fn bodies keep their relative indent" "$DEST5" '^    let w = build' 1
  expect_count "5.4 the nested mod keeps its relative indent" "$DEST5" '^mod nested \{$' 1
  expect_count "5.5 the nested mod's items sit one level deeper" "$DEST5" '^    fn nested_counts_twice' 1
fi

# --- 6. one defect per named cause (rc=4) ----------------------------------------------
new_sandbox defects
DEST6="$SB/src/lib/tests.rs"

run_split "$SRC" --mod nope --dest "$DEST6"
expect_rc "6.1 an unknown mod is a defect" 4 "$RC" "$ERRF"
expect_match "6.2 ... naming the missing mod" "$ERR_ALL" 'nope|not found'

run_split "$SRC" --mod prod --dest "$DEST6"
expect_rc "6.3 a production mod (no cfg(test)) is a defect" 4 "$RC" "$ERRF"
expect_match "6.4 ... naming the cfg(test) requirement" "$ERR_ALL" 'cfg\(test\)|test mod'

run_split "$SRC" --mod tests --dest "$SRC"
expect_rc "6.5 dest == source is a defect" 4 "$RC" "$ERRF"

run_split "$SRC" --mod tests --dest "$DEST6"
run_split "$SRC" --mod tests --dest "$DEST6"
expect_rc "6.6 an existing dest is a defect (no clobber)" 4 "$RC" "$ERRF"

new_sandbox defects_missing
TOOL="$SB/bin/split_tests.sh"
run_split "$SB/src/gone.rs" --mod tests --dest "$SB/src/lib/tests.rs"
expect_rc "6.7 a missing source is a defect" 4 "$RC" "$ERRF"
expect_match "6.8 ... naming the missing file" "$ERR_ALL" 'gone\.rs|not found|no such'

new_sandbox defects_unbalanced
TOOL="$SB/bin/split_tests.sh"
write_fixture "$SRC"
# unbalance the file: drop the LAST line (the closing brace of other_tests)
sed -i '$d' "$SRC"
run_split "$SRC" --mod tests --dest "$SB/src/lib/tests.rs"
expect_rc "6.9 an INDETERMINATE span is refused fail-closed" 4 "$RC" "$ERRF"
expect_match "6.10 ... carrying the preflight's reason" "$ERR_ALL" 'INDETERMINATE|indeterminate|unclosed|unbalanced'

new_sandbox defects_ambiguous
TOOL="$SB/bin/split_tests.sh"
write_fixture "$SRC"
cat >> "$SRC" <<'DUP'

#[cfg(test)]
mod tests {
    #[test]
    fn duplicate_name_in_a_second_mod() {}
}
DUP
run_split "$SRC" --mod tests --dest "$SB/src/lib/tests.rs"
expect_rc "6.11 an ambiguous mod name is a defect" 4 "$RC" "$ERRF"
expect_match "6.12 ... naming the ambiguity" "$ERR_ALL" 'ambiguous|two|multiple'

# --- 7. a broken python3 fails with a NAMED cause ---------------------------------------
new_sandbox broken_py
TOOL="$SB/bin/split_tests.sh"
STUB="$SB/stub"
mkdir -p "$STUB"
printf '#!/usr/bin/env bash\nexit 7\n' > "$STUB/python3"
chmod +x "$STUB/python3"
OUT="$(PATH="$STUB:$PATH" bash "$TOOL" "$SRC" --mod tests --dest "$SB/src/lib/tests.rs" 2>"$SB/err.txt")"
RC=$?
ERRF="$SB/err.txt"
ERR_ALL="$OUT$(cat "$SB/err.txt" 2>/dev/null)"
expect_rc "7.1 the tool fails (rc=1), never invents the span" 1 "$RC" "$ERRF"
expect_match "7.2 ... with a NAMED python3 cause" "$ERR_ALL" 'python3'
expect_no_match "7.3 ... and no bare traceback" "$ERR_ALL" 'Traceback'

# --- 8. the gate wiring lock (added with the wiring in this same PR) --------------------
# TODO(F2-wiring): the .tsv repo-guards note, ci.yml and ci_fast_gate.sh must
# name BOTH scripts (live code lines only); the case lands with the wiring
# commit so every commit in this branch stays green.

# --- 9. the family lint ------------------------------------------------------------------
LINT_PAT='\|[[:space:]]*grep[[:space:]]+(-[A-Za-z]*q[A-Za-z]*|--quiet)'
for f in split_tests.sh test_split_tests.sh; do
  LINT_HITS="$(sed 's/#.*$//' "$SCRIPT_DIR/$f" | grep -cE "$LINT_PAT")" || LINT_HITS=0
  expect_eq "9.1 no early-exit grep after a pipe in $f" "${LINT_HITS:-0}" "0"
done

# --- 10. the negation flip (#2056) -------------------------------------------------------
if [ "$NEGATION" -eq 1 ]; then
  # the flip: on the correct tree the dest DOES carry the item, so this
  # inverted expectation must fail the negated run.
  new_sandbox negated
  DESTN="$SB/src/lib/tests.rs"
  run_split "$SRC" --mod tests --dest "$DESTN"
  if [ "$RC" -eq 0 ] && [ "$(count_name "$DESTN" builds_a_widget)" = "1" ]; then
    bad "10.0n negation flip" "the correct tree passed the INVERTED expectation -- the harness cannot fail (#2056)"
  else
    ok "10.0n negation flip" "the inverted expectation failed as required"
  fi
else
  NEG_OUT="$(bash "$0" --negation 2>"$WORK/err.neg")"
  NEG_RC=$?
  expect_rc "10.1 --negation makes the harness fail (it can fail, #2056)" 1 "$NEG_RC" "$WORK/err.neg"
  if printf '%s\n' "$NEG_OUT" | grep -E '10\.0n negation flip' > /dev/null; then
    ok "10.2 the negated run reports the flipped case"
  else
    bad "10.2 the negated run reports the flipped case" "the negation output does not name the flip"
  fi
fi

# --- summary ------------------------------------------------------------------------------
printf '\n'
printf '  %s passed, %s failed\n' "$passed" "$failed"
if [ "$failed" -eq 0 ]; then
  exit 0
fi
exit 1
