#!/usr/bin/env bash
# test_file_size_preflight.sh -- semantics harness for scripts/file_size_preflight.sh
#
# ODD odd/tasks/file-size-preflight.md, umbrella #2584, phase F1a.
#
# The guard asks "is this file legal". The preflight asks "what is it, and what
# will a cut cost". Before the preflight every answer to that was prose, and the
# prose was wrong every time it was measured (~490 vs 312, 4063 vs 4064).
#
# WHAT THIS HARNESS PROVES, offline (mktemp sandboxes; no cargo, no network):
#
#   1. the tool exists and --help names its three modes;
#   2. its corpus literal is BYTE-IDENTICAL to the find expression in
#      scripts/check_file_size.sh: one corpus text, one owner, the guard;
#   3. a missing path fails with a NAMED cause and rc=1, never an empty block;
#   4. a path OUTSIDE the corpus is reported (in_corpus=false) and rc=0;
#   5. every number carries value:unit:command, and its command REPRODUCES it;
#   6. production + inline_tests == total (the composition identity);
#   7. the inline-tests span is lexed: braces in strings, chars, comments and
#      lifetimes do not move it;
#   8. an UNBALANCED file reports the span as indeterminate, never invents it;
#   9. outside a git repository the churn says indeterminate, never 0;
#  10. --simulate arithmetic equals a REAL cut performed line by line;
#  11. --simulate rejects a non-grandfathered destination over hard_limit (3),
#      and a grandfathered one over its own baseline (3);
#  12. --simulate rejects a defective plan (4, named): inverted range, range
#      past EOF, overlapping ranges, destination == source;
#  13. the diff budget P carries unit, comparator and the command that
#      reproduces each sample figure; prs = ceil(2*moved / P) in code;
#  14. nothing is written into the measured tree;
#  15. the tool and this harness are named by .github/workflows/ci.yml, so the
#      meta-guard #2056 cannot leave them dormant, and the coverage .tsv gains
#      no row: a row per script would break #2559.
#  16. the suggested §5 classification is a MEASURED predicate: exception (the
#      baseline's exceptions map), tests-majority (inline > 50%), giant-function
#      (allow(too_many_lines)), needs-review otherwise.
#  17. --glue delivers §4.3's "plus headers": destination = header + moved +
#      glue, source = total - moved + glue; glue on a file the plan does not
#      touch is a defect.
#  18. the harness watches its own house: no early-exit grep after a pipe in
#      the four family scripts, the local-gate wiring is enforced on live code
#      lines only, and a broken python3 fails with a named cause.
#  19. the block keys are a CLOSED ENGLISH LIST: any key outside it (a new
#      Spanish key included) turns this red; --help's usage lines are bound
#      literally (round 8, the loose-word lesson).
#  20. --lex-span (F2, odd/tasks/file-size-split-tests.md): one SPAN line per
#      top-level braced mod -- start:end INCLUDING the attribute run, plus the
#      name; the lexer traps do not move the end; an unbalanced file is
#      INDETERMINATE fail-closed; a missing path is a NAMED rc=1; a file with
#      no braced mods prints nothing. The aggregate SPANS format that
#      --format/--simulate consume stays byte-identical.
#
# HERMETIC BY DESIGN. The tool is exercised as a COPY inside a sandbox tree (its
# ROOT is derived from its own location), so this harness never writes to the
# repository -- the same shape test_file_size.sh uses.
#
# NEGATION: --negation tampers the corpus expectation, so this harness fails on
# a correct tree. A harness that cannot fail proves nothing (#2056).
#
# Usage: bash scripts/test_file_size_preflight.sh [--negation]

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PRE="$REPO_ROOT/scripts/file_size_preflight.sh"
GUARD="$REPO_ROOT/scripts/check_file_size.sh"
BASELINE="$REPO_ROOT/scripts/quality-baselines.json"
CI_YML="$REPO_ROOT/.github/workflows/ci.yml"

NEGATION=0
for arg in "$@"; do
  case "$arg" in
    --negation) NEGATION=1 ;;
    -h|--help_preflight)
      printf '%s\n' "Usage: bash scripts/test_file_size_preflight.sh [--negation]"
      printf '%s\n' ""
      printf '%s\n' "  --negation  flips the corpus-parity expectation."
      printf '%s\n' "              A harness that cannot fail proves nothing (#2056)."
      exit 0 ;;
    *)
      printf 'unknown argument: %s\n' "$arg" >&2
      exit 2 ;;
  esac
done

if [ ! -f "$PRE" ]; then
  printf 'FAIL: missing %s -- F1a not written yet (expected RED)\n' "$PRE" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0

ok() {
  printf '  ok   %-62s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-62s %s\n' "$1" "$2"
  if [ -n "${3:-}" ] && [ -f "$3" ]; then
    sed 's/^/        /' "$3" | head -20
  elif [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/        /' | head -20
  fi
  failed=$((failed + 1))
}

# expect_rc <desc> <expected> <real> [detail]
expect_rc() {
  if [ "$2" = "$3" ]; then ok "$1" "rc=$3"; else bad "$1" "expected rc=$2, got rc=$3" "${4:-}"; fi
}

# expect_eq <desc> <real> <expected>
expect_eq() {
  if [ "$2" = "$3" ]; then ok "$1" "$2"; else bad "$1" "got='$2' expected='$3'"; fi
}

# expect_match <desc> <text> <regex>  (an extended grep regex)
# grep -E -- "$3" > /dev/null, NOT -q: with set -o pipefail, -q's early exit
# on the first match SIGPIPEs the printf when the text is larger than the
# pipe buffer (the 161 KB ci.yml) and the pipeline fails DESPITE the match.
expect_match() {
  if printf '%s\n' "$2" | grep -E -- "$3" > /dev/null; then ok "$1" "$2"; else bad "$1" "no match /$3/ in: $2"; fi
}

SB=""
RC=0
OUT=""
ERRF=""

# --- sandbox -------------------------------------------------------------------

new_sandbox() {
  SB="$WORK/$1"
  mkdir -p "$SB/scripts" "$SB/crates/webfang_x/src"
  cp "$PRE" "$SB/scripts/file_size_preflight.sh"
  cp "$GUARD" "$SB/scripts/check_file_size.sh"
  cp "$BASELINE" "$SB/scripts/quality-baselines.json"
}

# write_baseline <path:count>... [exc:path ...] -- a grandfathered map;
# exc:path adds the entry to the exceptions map (the guard's parser tolerates it:
# it only validates grandfathered and hard_limit).
write_baseline() {
  python3 -I - "$SB/scripts/quality-baselines.json" "$@" <<'PY'
import json, sys
out, specs = sys.argv[1], sys.argv[2:]
gf, exc = {}, {}
for s in specs:
    if s.startswith("exc:"):
        exc[s[4:]] = "maintainer: cohesive declarative (fixture)"
    else:
        path, count = s.rsplit(":", 1)
        gf[path] = int(count)
ratchet = {"hard_limit": 500, "grandfathered": dict(sorted(gf.items()))}
if exc:
    ratchet["exceptions"] = dict(sorted(exc.items()))
json.dump({"file_size_rust": ratchet}, open(out, "w", encoding="utf-8"), indent=2)
PY
}

# write_code <file> <n>: n one-line functions => the file measures n
write_code() {
  local f="$1" n="$2" i
  mkdir -p "$(dirname "$f")"
  : > "$f"
  for i in $(seq 1 "$n"); do
    printf 'pub fn f%s() {}\n' "$i" >> "$f"
  done
}

# append_test_mod <file> <n>: #[cfg(test)] + mod tests { ... } => n+4 lines
append_test_mod() {
  local f="$1" n="$2" i
  {
    printf '#[cfg(test)]\n'
    printf 'mod tests {\n'
    printf '    use super::*;\n'
    for i in $(seq 1 "$n"); do
      printf '    fn t%s() { assert!(true); }\n' "$i"
    done
    printf '}\n'
  } >> "$f"
}

run_pre() { # runs the sandbox COPY; leaves stdout in OUT and rc in RC
  local errf="$WORK/err.$$.$RANDOM"
  OUT="$(cd "$SB" && bash scripts/file_size_preflight.sh "$@" 2>"$errf")"
  RC=$?
  ERRF="$errf"
}

# --- 1. the tool answers --help and names its modes -----------------------------
# NEGATION: a tool whose modes are undiscoverable is a tool only its author runs.

new_sandbox c1
write_code "$SB/crates/webfang_x/src/a.rs" 700
run_pre --help
expect_rc "1.1 --help exits 0" 0 "$RC" "$ERRF"
for mode in "--format" "--simulate" "--corpus-literal" "--budget"; do
  expect_match "1.2 --help names $mode" "$OUT" "$mode"
done
expect_match "1.3 --help names the unit 'lines' (wc -l)" "$OUT" "lines"

# --- 2. the corpus is the guard's corpus, byte for byte -------------------------
# NEGATION A preflight that re-derives the corpus drifts from the ratchet one
# commit later, and only the drift is ever noticed. One corpus, one owner.

new_sandbox c2
# the \$ROOT below is the guard's own literal text, not an expansion
# shellcheck disable=SC2016
GUARD_FIND="$(sed -n 's/^ *if ! \(find "\$ROOT\/crates" .*\) -print0 \\$/\1/p' "$GUARD" | head -1)"
# shellcheck disable=SC2016
GUARD_CORPUS="$(printf '%s' "$GUARD_FIND" | sed 's/find "\$ROOT\/crates"/find crates/')"
if [ -z "$GUARD_CORPUS" ]; then
  bad "2.0 the harness extracts the guard's find" "no match in $GUARD"
else
  ok "2.0 the harness extracts the guard's find" "$GUARD_CORPUS"
fi

run_pre --corpus-literal
EXPECT_CORPUS="$GUARD_CORPUS"
if [ "$NEGATION" = "1" ]; then
  EXPECT_CORPUS="$GUARD_CORPUS -maxdepth 1"
fi
expect_eq "2.1 --corpus-literal == the guard's find, byte for byte" "$OUT" "$EXPECT_CORPUS"

# 2.2 the THREE-WAY LOCK: AGENTS.md documents the corpus, the guard owns it,
# the preflight imports it. If the guard's find expression changes without
# AGENTS.md moving with it (or vice versa), this fails: a corpus the
# documentation does not describe is a corpus nobody can audit.
AGENTS_CORPUS="$(grep -o "find crates -name '[^']*' -not -path '[^']*'" "$REPO_ROOT/AGENTS.md" | head -1)"
if [ -z "$AGENTS_CORPUS" ]; then
  bad "2.2 the harness extracts the corpus documented in AGENTS.md" "no 'find crates' in $REPO_ROOT/AGENTS.md"
else
  expect_eq "2.2 the corpus is the one AGENTS.md documents (three-way lock)" "$OUT" "$AGENTS_CORPUS"
fi

# --- 3. a missing path fails with a NAMED cause --------------------------------
# NEGATION: an empty block for a typo reads as "healthy file".

new_sandbox c3
run_pre --format "crates/webfang_x/src/nope.rs"
expect_rc "3.1 a missing path fails (rc=1)" 1 "$RC" "$ERRF"
expect_match "3.2 the cause is named, not a bare traceback" "$(cat "$ERRF" 2>/dev/null)" "file not found|NOT_FOUND"

run_pre --format
expect_rc "3.3 --format without a path is a usage error (rc=64, EX_USAGE)" 64 "$RC" "$ERRF"
run_pre --help
expect_match "3.4 --help documents the exit-code map (64 = usage, EX_USAGE)" "$OUT" "^Exit codes: 0 ok.*64 usage \(EX_USAGE\)$"

# --- 4. a path outside the corpus is REPORTED, not measured --------------------
# NEGATION: crates/**/tests/* is outside the ratchet corpus; measuring it would
# claim the guard sees a file it never sees.

new_sandbox c4
write_code "$SB/crates/webfang_x/tests/x.rs" 900
run_pre --format "crates/webfang_x/tests/x.rs"
expect_rc "4.1 a file outside the corpus is not a failure (rc=0)" 0 "$RC" "$ERRF"
expect_match "4.2 ... and says so" "$OUT" "^in_corpus	(false|0)$"

# --- 5. the block: value, unit, command; production + tests == total -----------
new_sandbox c5
write_code "$SB/crates/webfang_x/src/b.rs" 600
append_test_mod "$SB/crates/webfang_x/src/b.rs" 96
TOTAL_B="$(wc -l < "$SB/crates/webfang_x/src/b.rs")"
run_pre --format "crates/webfang_x/src/b.rs"
expect_rc "5.1 --format exits 0 on a healthy file" 0 "$RC" "$ERRF"

field() { printf '%s\n' "$1" | awk -F'\t' -v k="$2" '$1==k{print $2; exit}'; }
cmd_of() { printf '%s\n' "$1" | awk -F'\t' -v k="$2" '$1==k{print $4; exit}'; }

T_SIM="$(field "$OUT" total)"
P_SIM="$(field "$OUT" production)"
I_SIM="$(field "$OUT" inline_tests)"
expect_eq "5.2 total is the file's wc -l" "$T_SIM" "$TOTAL_B"
expect_eq "5.3 inline_tests is the mod tests span (n+4)" "$I_SIM" "100"
expect_eq "5.4 production + inline_tests == total" "$((${P_SIM:-0} + ${I_SIM:-0}))" "$T_SIM"

# --- 6. every command in the block REPRODUCES its number ----------------------
# NEGATION: a command that does not reproduce its value is decoration, and
# decoration is exactly what the playbook replaced.

T_CMD="$(cmd_of "$OUT" total)"
I_CMD="$(cmd_of "$OUT" inline_tests)"
if [ -n "$T_CMD" ]; then
  REPRO="$(cd "$SB" && eval "$T_CMD" 2>/dev/null | tr -d ' ' | head -1)"
  expect_eq "6.1 the total command reproduces the total" "$REPRO" "$T_SIM"
else
  bad "6.1 the total command reproduces the total" "no command in column 4"
fi
if [ -n "$I_CMD" ]; then
  REPRO="$(cd "$SB" && eval "$I_CMD" 2>/dev/null | tr -d ' ' | head -1)"
  expect_eq "6.2 the inline-tests command reproduces the span" "$REPRO" "$I_SIM"
else
  bad "6.2 the inline-tests command reproduces the span" "no command in column 4"
fi

# --- 7. the span is LEXED, not brace-counted ----------------------------------
# The fixture is brace-balanced only for a lexer: a naive counter sees 4 extra
# '{' and 3 extra '}' (strings, a char literal, a comment, a lifespan).

new_sandbox c7
{
  printf 'pub fn head() {}\n'
  printf '#[cfg(test)]\n'
  printf 'mod tests {\n'
  printf '    fn a() { let s = "{"; assert_eq!(s, "{"); }\n'
  printf '    fn b() { let c = '"'"'{'"'"'; assert_eq!(c, '"'"'{'"'"'); }\n'
  printf '    fn c() { let r = r#"{"#; assert!(r.starts_with('"'"'{'"'"')); }\n'
  printf '    // mod tests { a comment that looks like a block\n'
  printf '    fn d() { /* } */ let x: &'"'"'static str = "}"; assert_eq!(x, "}"); }\n'
  printf '}\n'
} > "$SB/crates/webfang_x/src/c.rs"
C_TOTAL="$(wc -l < "$SB/crates/webfang_x/src/c.rs")"
run_pre --format "crates/webfang_x/src/c.rs"
expect_rc "7.1 the adversarial file is measured (rc=0)" 0 "$RC" "$ERRF"
expect_eq "7.2 the lexer finds the span at line 2 (span = total-2+1)" "$(field "$OUT" inline_tests)" "$((C_TOTAL - 1))"
expect_eq "7.3 production is line 1 only" "$(field "$OUT" production)" "1"

# 7.4-7.7 raw-string fixtures that FAIL against the pre-fix lexer (the 7
# fixture passed by accident: r#" never entered RAW and the STR fallback
# closed early). Every construct here broke a REAL corpus file (the 14
# indeterminados of the first full-corpus contrast).
new_sandbox c7b
{
  printf 'pub fn head() {}\n'
  printf 'pub fn with_raw() {\n'
  printf '    let j = r#"{\n'
  printf '        "name": "x",\n'
  printf '        \\"nested\\": 1\n'
  printf '    }"#;\n'
  printf '    assert!(j.contains("x"));\n'
  printf '}\n'
  printf '#[cfg(test)]\n'
  printf 'mod tests {\n'
  printf '    #[test]\n'
  printf '    fn t1() { assert!(true); }\n'
  printf '}\n'
} > "$SB/crates/webfang_x/src/c7b.rs"
run_pre --format "crates/webfang_x/src/c7b.rs"
expect_rc "7.4 a multiline r# JSON string is measured (rc=0)" 0 "$RC" "$ERRF"
expect_eq "7.4b the span is determinate at the mod (9..13 = 5)" "$(field "$OUT" inline_tests)" "5"

new_sandbox c7c
{
  printf 'pub fn h() {}\n'
  printf '#[cfg(test)]\n'
  printf 'mod tests {\n'
  printf '    fn a() { let s = r##"inner "# not a close"##; assert_eq!(s, "x"); }\n'
  printf '}\n'
} > "$SB/crates/webfang_x/src/c7c.rs"
run_pre --format "crates/webfang_x/src/c7c.rs"
expect_eq "7.5 an r## raw keeps its span determinate despite an inner quote-hash" "$(field "$OUT" inline_tests)" "4"

new_sandbox c7d
{
  printf 'pub fn h() {}\n'
  printf '#[cfg(test)]\n'
  printf 'mod tests {\n'
  printf '    fn a() { let s = br#"say "hi""#; assert!(s.contains("hi")); }\n'
  printf '}\n'
} > "$SB/crates/webfang_x/src/c7d.rs"
run_pre --format "crates/webfang_x/src/c7d.rs"
expect_eq "7.6 a br# raw with an inner quote keeps its span determinate" "$(field "$OUT" inline_tests)" "4"

new_sandbox c7e
{
  printf 'pub fn h() {}\n'
  printf '#[cfg(test)]\n'
  printf 'mod tests {\n'
  printf '    fn a() { assert!(true); }\n'
  printf '}\n'
  printf '#[test]\n'
  printf 'fn bare_outside() { assert!(true); }\n'
} > "$SB/crates/webfang_x/src/c7e.rs"
run_pre --format "crates/webfang_x/src/c7e.rs"
expect_match "7.7 a bare #[test] outside the span goes indeterminate, never a wrong number" "$OUT" "^inline_tests	indeterminate"
expect_match "7.7b ... and classifies as needs-review, never tests-majority" "$OUT" "^classification	needs-review"

# --- 8. an UNBALANCED file says so; it does not invent a span ------------------
# NEGATION: a truncated file (missing the closing brace after a crash) would
# yield "production = 0" from a naive counter -- a plausible, wrong number.

new_sandbox c8
{
  printf 'pub fn head() {}\n'
  printf '#[cfg(test)]\n'
  printf 'mod tests {\n'
  printf '    fn a() { assert!(true); }\n'
} > "$SB/crates/webfang_x/src/d.rs"
run_pre --format "crates/webfang_x/src/d.rs"
expect_rc "8.1 an unbalanced file is still reportable (rc=0)" 0 "$RC" "$ERRF"
expect_match "8.2 the span is declared indeterminate" "$OUT" "^inline_tests	indeterminate"
expect_match "8.3 ... with the cause, not a number" "$OUT" "^inline_tests_note	[^	]+"

# --- 9. churn outside a git repository is INDETERMINATE, never 0 ---------------
# NEGATION: "0 commits" and "cannot know" are different facts, and the whole
# playbook exists because the two were written as the same one.

new_sandbox c9
write_code "$SB/crates/webfang_x/src/e.rs" 520
run_pre --format "crates/webfang_x/src/e.rs"
expect_match "9.1 outside git the churn is indeterminate" "$OUT" "^churn_commits	indeterminate"

# --- 10. --simulate arithmetic equals a REAL cut -------------------------------
# The harness performs the cut: extract L11-40 to src/z.rs with a 2-line header.

new_sandbox c10
write_code "$SB/crates/webfang_x/src/f.rs" 600
SOURCE="$SB/crates/webfang_x/src/f.rs"
DEST="$SB/crates/webfang_x/src/z.rs"
FROM=11
TO=40
HEADER=2
MOVED=$((TO - FROM + 1))
run_pre --simulate "src/z.rs:$FROM-$TO:header=$HEADER" --format "crates/webfang_x/src/f.rs"
expect_rc "10.1 --simulate exits 0 on a legal plan" 0 "$RC" "$ERRF"

sim_total() { printf '%s\n' "$1" | awk -F'\t' -v p="$2" '$1=="plan" && $2==p{print $3; exit}'; }

# real cut, same semantics: destination = header + moved lines; source loses
# them. The header is REAL lines (a split adds a module header), so head
# -n "$HEADER" /dev/null -- which produces none -- cannot stand in for it.
{
  for ((h = 0; h < HEADER; h++)); do printf '// header line\n'; done
  sed -n "${FROM},${TO}p" "$SOURCE"
} > "$DEST"
sed -i "${FROM},${TO}d" "$SOURCE"
REAL_SRC="$(wc -l < "$SOURCE")"
REAL_DEST="$(wc -l < "$DEST")"
EXPECT_SRC=$((600 - MOVED))
expect_eq "10.2 the harness cut what it said it cut" "$REAL_DEST" "$((HEADER + MOVED))"
expect_eq "10.3 simulated source size == real cut source size" "$(sim_total "$OUT" src/f.rs)" "$REAL_SRC"
expect_eq "10.4 simulated destination size == real cut destination size" "$(sim_total "$OUT" src/z.rs)" "$REAL_DEST"
if [ "$EXPECT_SRC" != "$REAL_SRC" ]; then
  bad "10.5 the cut is the one the plan describes" "expected $EXPECT_SRC, real $REAL_SRC"
else
  ok "10.5 the cut is the one the plan describes" "$REAL_SRC"
fi

# --- 11. --simulate enforces the ratchet's own rule ----------------------------
# (a) a non-grandfathered destination over hard_limit; (b) a grandfathered
# source that would exceed its own baseline after the cut.

new_sandbox c11
write_code "$SB/crates/webfang_x/src/g.rs" 900
write_baseline "crates/webfang_x/src/g.rs:900"
run_pre --simulate "src/big.rs:1-600" --format "crates/webfang_x/src/g.rs"
expect_rc "11.1 a destination over hard_limit is rejected (rc=3)" 3 "$RC" "$ERRF"
expect_match "11.2 ... naming the destination and the limit" "$OUT$(cat "$ERRF" 2>/dev/null)" "hard_limit|500"

new_sandbox c11b
write_code "$SB/crates/webfang_x/src/g2.rs" 600
write_code "$SB/crates/webfang_x/src/d2.rs" 100
write_baseline "crates/webfang_x/src/d2.rs:150"
run_pre --simulate "src/d2.rs:1-100" --format "crates/webfang_x/src/g2.rs"
expect_rc "11.4 a grandfathered DESTINATION over its own cap is rejected (rc=3)" 3 "$RC" "$ERRF"
expect_match "11.5 ... naming its own baseline cap" "$OUT$(cat "$ERRF" 2>/dev/null)" "its baseline \(150\)"

new_sandbox c12
write_code "$SB/crates/webfang_x/src/h.rs" 540
write_baseline "crates/webfang_x/src/h.rs:540"
# moves 10 lines but the destination adds 20 header lines: h stays at 530 <= 540, legal
run_pre --simulate "src/h2.rs:1-10:header=20" --format "crates/webfang_x/src/h.rs"
expect_rc "11.3 a grandfathered source that SHRINKS stays legal (rc=0)" 0 "$RC" "$ERRF"

# --- 12. a defective plan is refused, with the cause ---------------------------
new_sandbox c13
write_code "$SB/crates/webfang_x/src/i.rs" 300
run_pre --simulate "src/x.rs:40-11" --format "crates/webfang_x/src/i.rs"
expect_rc "12.1 an inverted range is refused (rc=4)" 4 "$RC" "$ERRF"
run_pre --simulate "src/x.rs:280-400" --format "crates/webfang_x/src/i.rs"
expect_rc "12.2 a range past EOF is refused (rc=4)" 4 "$RC" "$ERRF"
run_pre --simulate "src/x.rs:10-20" "src/y.rs:15-25" --format "crates/webfang_x/src/i.rs"
expect_rc "12.3 overlapping ranges are refused (rc=4)" 4 "$RC" "$ERRF"
run_pre --simulate "src/i.rs:10-20" --format "crates/webfang_x/src/i.rs"
expect_rc "12.4 destination == source is refused (rc=4)" 4 "$RC" "$ERRF"
new_sandbox c14
write_code "$SB/crates/webfang_x/src/j.rs" 500
run_pre --simulate "src/x.rs:10-20" --format "crates/webfang_x/src/j.rs"
expect_rc "12.5 extracting from a file AT hard_limit is refused (rc=3)" 3 "$RC" "$ERRF"
expect_match "12.6 ... pointing at the consolidation (--remove), not a floor" "$(cat "$ERRF" 2>/dev/null)" "remove|<= *hard_limit|consolid"

# --- 13. the budget P: unit, comparator, and a command per sample --------------
# NEGATION: P restated in prose is the 2000-then-3000 defect this phase exists
# to end. Here it lives in code, with the command that reproduces each figure.

new_sandbox c15
write_code "$SB/crates/webfang_x/src/k.rs" 500
run_pre --budget
expect_rc "13.1 --budget exits 0" 0 "$RC" "$ERRF"
expect_match "13.2 P carries its unit (add+del)" "$OUT" "3000	(add\+del)|3000.*add\+del"
expect_match "13.3 P states its comparator (strict >)" "$OUT" "strict.*>"
for sample in 2246#2901 3018#2896 3933#2856 5101#2893; do
  n="${sample%%#*}"
  pr="${sample##*#}"
  expect_match "13.4 sample $n (#$pr) carries its reproducing command" "$OUT" "gh pr view $pr .*--json"
done
expect_match "13.5 the PR-count formula is computed, not restated" "$OUT" "prs	[0-9]+	PRs	"

new_sandbox c16
write_code "$SB/crates/webfang_x/src/l.rs" 900
run_pre --simulate "src/l1.rs:1-3000" --format "crates/webfang_x/src/l.rs"
PRS="$(printf '%s\n' "$OUT" | awk -F'\t' '$1=="prs"{print $2; exit}')"
# moved = 900 lines (all of them); P = 3000 -> ceil(2*900/3000) = 1
expect_eq "13.6 prs = ceil(2 * moved / P) computed in code" "${PRS:-?}" "1"

# --- 14. nothing is written into the measured tree ----------------------------
new_sandbox c17
write_code "$SB/crates/webfang_x/src/m.rs" 640
BEFORE="$(cd "$SB" && find . -type f | sort | xargs sha256sum 2>/dev/null | sha256sum)"
run_pre --format "crates/webfang_x/src/m.rs"
run_pre --simulate "src/m1.rs:1-100" --format "crates/webfang_x/src/m.rs"
run_pre --budget
AFTER="$(cd "$SB" && find . -type f | sort | xargs sha256sum 2>/dev/null | sha256sum)"
expect_eq "14.1 the sandbox tree is byte-identical after every mode" "$AFTER" "$BEFORE"

# --- 15. the gate wiring: ci.yml names BOTH, the .tsv gains no row ------------
# #2056 refuses an unwired check_*.sh; this tool is not a check_*, so nothing
# else asserts it is reachable. The .tsv is a table of JOBS (#2559): a row per
# script would make test_ci_gate_structure.sh fail on an unknown job.

expect_match "15.1 ci.yml runs the preflight harness" "$(cat "$CI_YML" 2>/dev/null)" "test_file_size_preflight\.sh"
expect_match "15.2 ci.yml runs the preflight itself" "$(cat "$CI_YML" 2>/dev/null)" "file_size_preflight\.sh$"
TSV_EXTRA="$(awk -F'\t' '$1 !~ /^#/ && $1 != "" {print $1}' "$REPO_ROOT/scripts/ci_fast_gate_coverage.tsv" \
  | grep -E 'file_size_preflight|test_file_size_preflight' || true)"
expect_eq "15.3 the coverage .tsv declares no row for a non-job" "$TSV_EXTRA" ""

# --- 16. the suggested §5 classification is a MEASURED predicate ---------------
# NEGATION: a classification invented from vibes is the prose defect again.
# The mechanical §5 signals get fixtures; the semantic ones (glued modules,
# cohesive declarative, high risk, architectural) are maintainer calls per
# §5's own Signals column, so a file without mechanical signals says "needs-review".

new_sandbox c18
{
  for i in $(seq 1 100); do printf 'pub fn p%s() {}\n' "$i"; done
  printf '#[cfg(test)]\nmod tests {\n    use super::*;\n'
  for i in $(seq 1 300); do printf '    fn t%s() { assert!(true); }\n' "$i"; done
  printf '}\n'
} > "$SB/crates/webfang_x/src/n.rs"
run_pre --format "crates/webfang_x/src/n.rs"
expect_match "16.1 inline > 50% suggests tests-majority" "$OUT" "^classification	tests-majority"

new_sandbox c19
{
  printf '#[allow(clippy::too_many_lines)]\n'
  printf 'pub fn big() {\n'
  for i in $(seq 1 500); do printf '    let _%s = %s;\n' "$i" "$i"; done
  printf '}\n'
} > "$SB/crates/webfang_x/src/o.rs"
run_pre --format "crates/webfang_x/src/o.rs"
expect_match "16.2 an allow(too_many_lines) suggests giant-function" "$OUT" "^classification	giant-function"

new_sandbox c20
write_code "$SB/crates/webfang_x/src/q.rs" 600
run_pre --format "crates/webfang_x/src/q.rs"
expect_match "16.3 production-heavy without signals says needs-review" "$OUT" "^classification	needs-review"
expect_match "16.4 ... naming the semantic §5 signals" "$OUT" "^classification_note	"

new_sandbox c21
write_code "$SB/crates/webfang_x/src/r.rs" 512
write_baseline "crates/webfang_x/src/r.rs:512" "exc:crates/webfang_x/src/r.rs"
run_pre --format "crates/webfang_x/src/r.rs"
expect_match "16.5 a declared exception says exception" "$OUT" "^classification	exception"

# --- 17. --glue: §4.3's "plus headers" completes the move model ---------------
# NEGATION: a simulator that cannot model the glue FORCES criterion drift —
# the v3 criterion "Reproduce server.rs 2012→360" got amended to a weaker
# form before the tool grew this parameter. With glue the verbatim criterion
# is reachable: glue is the non-moved lines a real split adds to a file
# (import blocks, re-exports), destination or source.

new_sandbox c22
write_code "$SB/crates/webfang_x/src/s1.rs" 600
run_pre --simulate "src/z1.rs:11-40:header=2" --glue "src/z1.rs=5" --format "crates/webfang_x/src/s1.rs"
expect_rc "17.1 a legal plan with glue exits 0" 0 "$RC" "$ERRF"
expect_eq "17.2 dest = header + moved + glue" "$(sim_total "$OUT" src/z1.rs)" "37"
expect_eq "17.3 the source is untouched by a destination's glue" "$(sim_total "$OUT" src/s1.rs)" "570"

new_sandbox c23
write_code "$SB/crates/webfang_x/src/s2.rs" 600
run_pre --simulate "src/z2.rs:11-40" --glue "src/s2.rs=29" --format "crates/webfang_x/src/s2.rs"
expect_rc "17.4 glue on the SOURCE exits 0" 0 "$RC" "$ERRF"
expect_eq "17.5 source gets its glue (the server.rs 2012→360 shape)" "$(sim_total "$OUT" src/s2.rs)" "599"

new_sandbox c24
write_code "$SB/crates/webfang_x/src/s3.rs" 600
run_pre --simulate "src/z3.rs:11-40" --glue "src/untouched.rs=5" --format "crates/webfang_x/src/s3.rs"
expect_rc "17.6 glue on a file the plan does not touch is a defect (rc=4)" 4 "$RC" "$ERRF"
expect_match "17.6b ... naming the untouched destination" "$OUT$(cat "$ERRF" 2>/dev/null)" "src/untouched.rs"
run_pre --simulate "src/z3.rs:11-40" --glue "src/z3.rs=-3" --format "crates/webfang_x/src/s3.rs"
expect_rc "17.7 negative glue is a defect (rc=4)" 4 "$RC" "$ERRF"
run_pre --simulate "src/z3.rs:11-40" --glue "src/z3.rs=abc" --format "crates/webfang_x/src/s3.rs"
expect_rc "17.9 non-integer glue is a defect (rc=4)" 4 "$RC" "$ERRF"
run_pre --simulate "src/z3.rs:11-40" --glue "src/z3.rs=" --format "crates/webfang_x/src/s3.rs"
expect_rc "17.10 empty glue value is a defect (rc=4)" 4 "$RC" "$ERRF"
run_pre --simulate "src/z3.rs:11-40" --glue "src/z3.rs=5" --glue "src/z3.rs=7" --format "crates/webfang_x/src/s3.rs"
expect_rc "17.11 duplicate glue for the same file is refused, not silently summed (rc=4)" 4 "$RC" "$ERRF"
run_pre --simulate "src/z3.rs:11-40" --glue "src/z3.rs=0" --format "crates/webfang_x/src/s3.rs"
expect_rc "17.14 zero glue is legal and changes nothing (rc=0)" 0 "$RC" "$ERRF"
expect_eq "17.14b dest = header + moved with zero glue" "$(sim_total "$OUT" src/z3.rs)" "30"
run_pre --simulate "src/z3.rs:11-40" --glue "src/z3.rs=-5" --format "crates/webfang_x/src/s3.rs"
expect_rc "17.15 negative glue (-5) is a defect (rc=4)" 4 "$RC" "$ERRF"

new_sandbox c26
write_code "$SB/crates/webfang_x/src/s5.rs" 600
run_pre --simulate "src/z5.rs:1-450:header=40" --glue "src/z5.rs=20" --format "crates/webfang_x/src/s5.rs"
expect_rc "17.12 glue counts toward the destination cap: 40+450+20 = 510 > 500 (rc=3)" 3 "$RC" "$ERRF"
expect_match "17.12b ... naming the limit" "$OUT$(cat "$ERRF" 2>/dev/null)" "hard_limit"

# 17.13 the FROZEN server.rs criterion fixture (owner round 3: no git show in
# the harness — a shallow clone has no history). The data below is frozen
# from the real split #2830 (commit 74ba3cb3); the producing commands were:
#   git show 74ba3cb3^:crates/webfang_mcp/src/mcp_server/server.rs | wc -l   # 2012
#   git show 74ba3cb3 -- crates/webfang_mcp/src/mcp_server/server.rs          # the 8 deleted runs (1681 lines)
#   git show 74ba3cb3 --numstat -- crates/webfang_mcp/src/mcp_server/server.rs # +29 glue lines
# The content is uniform: the arithmetic only needs the line COUNTS.
new_sandbox c27
write_code "$SB/crates/webfang_mcp/src/mcp_server/server.rs" 2012
run_pre --simulate "child_1.rs:9-9" "child_2.rs:11-13" "child_3.rs:15-16" "child_4.rs:19-22" \
  "child_5.rs:45-252" "child_6.rs:447-478" "child_7.rs:480-967" "child_8.rs:1070-2012" \
  --glue "mcp_server/server.rs=29" --format "crates/webfang_mcp/src/mcp_server/server.rs"
expect_eq "17.13 frozen #2830 data: source = 2012 - 1681 + 29 = 360" "$(sim_total "$OUT" mcp_server/server.rs)" "360"
expect_rc "17.13b the aggregate 943-line child is still refused (negative test, rc=3)" 3 "$RC" "$ERRF"

# --- 18. the harness watches its own house -------------------------------------
# 18.1 the lesson of §8 (issue #2925): the file-size family carries no pipe
# into an early-exit grep. 18.2 the local-gate wiring is ENFORCED, not just
# declared: ci_fast_gate.sh must name both scripts (a mutation test proved
# that nothing else goes red when it is removed). 18.3 a missing python3
# fails with a NAMED cause, never a bare traceback.

# the dangerous form: a pipe into grep whose option cluster contains -q
# (early exit). Plain -E consumes the whole pipe and is safe. Comments are
# stripped first: prose that MENTIONS the pattern is not a pipeline.
LINT_PAT='\|[[:space:]]*grep[[:space:]]+(-[A-Za-z]*q[A-Za-z]*|--quiet)'
for f in check_file_size.sh test_file_size.sh file_size_preflight.sh test_file_size_preflight.sh; do
  LINT_HITS="$(sed 's/#.*$//' "$REPO_ROOT/scripts/$f" | grep -cE "$LINT_PAT")" || LINT_HITS=0
  expect_eq "18.1 no early-exit grep after a pipe in $f" "${LINT_HITS:-0}" "0"
done

# count only LIVE code lines: a commented-out wiring line must stop counting
# (the mutation test proved that text-level counting misses the commented form)
GATE_HITS="$(sed 's/[[:space:]]*#.*$//' "$REPO_ROOT/scripts/ci_fast_gate.sh" | grep -c 'file_size_preflight.sh')" || GATE_HITS=0
HARNESS_HITS="$(sed 's/[[:space:]]*#.*$//' "$REPO_ROOT/scripts/ci_fast_gate.sh" | grep -c 'test_file_size_preflight.sh')" || HARNESS_HITS=0
if [ "${GATE_HITS:-0}" -ge 1 ] && [ "${HARNESS_HITS:-0}" -ge 1 ]; then
  ok "18.2 ci_fast_gate.sh runs the preflight and its harness (local wiring enforced)"
else
  bad "18.2 ci_fast_gate.sh runs the preflight and its harness" "preflight=$GATE_HITS harness=$HARNESS_HITS (the local wiring was removed or commented out and nothing notices)"
fi

new_sandbox c25
write_code "$SB/crates/webfang_x/src/s4.rs" 600
mkdir -p "$SB/fakebin"
printf '#!/usr/bin/env bash\nexit 127\n' > "$SB/fakebin/python3"
chmod +x "$SB/fakebin/python3"
OUT="$(cd "$SB" && PATH="$SB/fakebin:$PATH" bash scripts/file_size_preflight.sh --format "crates/webfang_x/src/s4.rs" 2>"$WORK/err18")"
RC=$?
ERRF="$WORK/err18"
expect_rc "18.3 a broken python3 fails named (rc=1)" 1 "$RC" "$ERRF"
expect_match "18.4 ... naming python3, never a bare traceback" "$(cat "$ERRF" 2>/dev/null)" "python3"

# --- 19. the block keys are a CLOSED ENGLISH LIST -----------------------------
# §8 lesson (round 8): the accent+stopword detector cannot see a loose Spanish
# word with no accent — it missed "clasificacion", an emitted key, while the
# five renamed tokens were being audited. So the contract is not DETECTED, it
# is ENUMERATED: every key the tool may emit must be in this list; any new key
# (Spanish or not) turns this red until the list is amended by a reviewed
# commit. The union is fed by REAL outputs of fixtures in every branch: budget,
# a classified file, the indeterminate-span branch, the no-git branch and a
# simulate plan.
CLOSED_KEYS='^(allow|blast_radius|blast_radius_note|churn_commits|churn_commits_note|classification|classification_note|functions|in_corpus|inline_tests|inline_tests_note|sample|overlap_prs|overlap_prs_note|path|plan|budget_diff|production|prs|total)$'
new_sandbox c26
write_code "$SB/crates/webfang_x/src/s5.rs" 600
for i in $(seq 1 30); do echo "fn f$i() {"; done > "$SB/crates/webfang_x/src/u5.rs"
run_pre --budget
BUDGET_OUT="$OUT"
run_pre --format "crates/webfang_x/src/s5.rs"
FORMAT_OUT="$OUT"
run_pre --format "crates/webfang_x/src/u5.rs"
UNBAL_OUT="$OUT"
run_pre --simulate "src/z2.rs:11-40:header=2" --glue "src/z2.rs=5" --format "crates/webfang_x/src/s5.rs"
SIM_OUT="$OUT"
KEYS="$(printf '%s\n' "$BUDGET_OUT" "$FORMAT_OUT" "$UNBAL_OUT" "$SIM_OUT" | awk -F'\t' 'NF>=2 && $1 ~ /^[a-z_]+$/ {print $1}' | sort -u)"
BAD_KEYS="$(printf '%s\n' "$KEYS" | grep -cvE "$CLOSED_KEYS")" || BAD_KEYS=0
expect_eq "19.1 every emitted block key is in the closed English list" "${BAD_KEYS:-0}" "0"
expect_match "19.2 the union is fed and exercises the renamed keys" "$KEYS" "(classification|functions|sample|budget_diff)"
run_pre --help
expect_match "19.3 --help opens with the English usage line" "$OUT" "^Usage: bash scripts/file_size_preflight.sh \[mode\]$"
expect_match "19.4 ... and the English no-arguments mode line" "$OUT" "^\s*\(no arguments\)\s+the budget block P \(what ci.yml runs: --budget\)$"
expect_match "19.5 budget_diff carries P with unit and comparator" "$BUDGET_OUT" "^budget_diff	3000	\(add\+del\)	warning at diff strictly > 3000"
expect_match "19.6 a sample row carries its gh command" "$BUDGET_OUT" "^sample	2246	\(add\+del\)	gh pr view 2901"
expect_match "19.7 the reserved functions row is emitted" "$FORMAT_OUT" "^functions		-	reserved"
# 19.8: the closed list also covers the SOURCE, not only the fixtures' output: a key added in
# a branch no fixture exercises (an emit* first argument) turns this red too.
SOURCE_KEYS="$(grep -oE 'emit[0-9]? +"[a-z_]+"' "$PRE" | sed -E 's/emit[0-9]? +"([a-z_]+)"/\1/' | sort -u)"
BAD_SOURCE_KEYS="$(printf '%s\n' "$SOURCE_KEYS" | grep -cvE "$CLOSED_KEYS")" || BAD_SOURCE_KEYS=0
expect_eq "19.8 every emit* first argument in the source is in the closed list" "${BAD_SOURCE_KEYS:-0}" "0"
expect_match "19.9 the source key set is fed and matches the emitted set" "$SOURCE_KEYS" "(classification|functions|sample|budget_diff)"
expect_eq "19.10 the source key set equals the emitted key set" "$(printf '%s\n' "$SOURCE_KEYS" | sort)" "$(printf '%s\n' "$KEYS" | sort)"

new_sandbox c27
for i in $(seq 1 4); do echo "fn f$i() {"; done > "$SB/crates/webfang_x/src/u6.rs"
run_pre --format "crates/webfang_x/src/u6.rs"
expect_match "19.11 an unclosed-brace file names the brace-depth cause in English" "$OUT" "^inline_tests_note	the file ends in an unclosed state \(brace depth [0-9]+\)"
new_sandbox c28
printf 'fn f() {
}
}
' > "$SB/crates/webfang_x/src/u7.rs"
run_pre --format "crates/webfang_x/src/u7.rs"
expect_match "19.12 an extra-brace file names the opener cause in English" "$OUT" "^inline_tests_note	closing brace without an opener"

# 19.13/19.14 (issue #2928): the plan rows carry their arithmetic NOTE as the
# 5th column. emit4 accepts only 4 args, so both notes (destination
# "header N + M moved + glue G", source "T lines - M moved + glue G") never
# reached the output; emit5 delivers them. The note is prose in the row, not a
# key of the closed list — but it IS observable output this harness pins.
expect_match "19.13 the destination plan row carries its note (header+moved+glue)" "$SIM_OUT" "^plan	src/z2\.rs	37	lines	header 2 \+ 30 moved \+ glue 5$"
expect_match "19.14 the source plan row carries its note (total-moved)" "$SIM_OUT" "^plan	src/s5\.rs	570	lines	600 lines - 30 moved$"

# --- 20. --lex-span: the span question's single owner (F2) ----------------------
# split_tests.sh consumes this mode; the aggregate format stays untouched.
new_sandbox c30
cat > "$SB/crates/webfang_x/src/lexprobe.rs" <<'PROBE'
pub mod prod {
    pub fn helper() -> usize {
        3
    }
}

#[cfg(test)]
mod tests {
    const RAW: &str = r#"{ "braces": "inside raw }" }"#;
    const CLOSE: char = '}';
    // mod fake { }
    #[test]
    fn one() {
        assert_eq!(1, 1);
    }
}
PROBE
run_pre --lex-span "crates/webfang_x/src/lexprobe.rs"
expect_rc "20.1 --lex-span exits 0" 0 "$RC" "$ERRF"
expect_match "20.2 the production span is named (prod 1:5)" "$OUT" "^SPAN	1:5	prod$"
expect_match "20.3 the test span is named and the traps do not move its end (tests 7:16)" "$OUT" "^SPAN	7:16	tests$"
ATTR_LINE="$(printf '%s\n' "$OUT" | awk -F'\t' '$3=="tests" {split($2, r, ":"); print r[1]}')"
expect_eq "20.4 the span starts at the attribute run, not the mod line" "$ATTR_LINE" "7"
expect_match "20.5 both spans on one invocation" "$OUT" "^SPAN	1:5	prod$"

printf 'pub mod only_decl;\n' > "$SB/crates/webfang_x/src/nodecls.rs"
run_pre --lex-span "crates/webfang_x/src/nodecls.rs"
expect_rc "20.6 a file without braced mods exits 0 with no spans" 0 "$RC" "$ERRF"
expect_eq "20.7 ... and prints nothing" "$OUT" ""

printf 'pub mod broken {\n' > "$SB/crates/webfang_x/src/unbal.rs"
run_pre --lex-span "crates/webfang_x/src/unbal.rs"
expect_match "20.8 an unbalanced file is INDETERMINATE fail-closed" "$OUT" "^INDETERMINATE	the file ends in an unclosed state"

run_pre --lex-span "crates/webfang_x/src/gone.rs"
expect_rc "20.9 a missing path is a NAMED tool failure (rc=1)" 1 "$RC" "$ERRF"
expect_match "20.10 ... naming the missing file" "$(cat "$ERRF" 2>/dev/null)" "gone\.rs"

# the aggregate consumer is untouched: --format still lexes spans into the
# same awk-range form (a test-mod-only fixture, the corpus's common shape)
write_code "$SB/crates/webfang_x/src/aggr.rs" 2
append_test_mod "$SB/crates/webfang_x/src/aggr.rs" 2
run_pre --format "crates/webfang_x/src/aggr.rs"
expect_match "20.11 --format's aggregate span still answers in its lexed form" "$OUT" "^inline_tests	[0-9]+	lines	awk"

# --- summary ------------------------------------------------------------------

echo "----------------------------------------"
echo "test_file_size_preflight: passed=$passed failed=$failed"
if [ "$failed" -gt 0 ]; then
  exit 1
fi
exit 0
