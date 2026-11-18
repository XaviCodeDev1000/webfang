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
#   9. outside a git repository the churn says indeterminado, never 0;
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
#      baseline's exceptions map), tests-mayoria (inline > 50%), funcion-gigante
#      (allow(too_many_lines)), revisar otherwise.
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
      printf '%s\n' "Uso: bash scripts/test_file_size_preflight.sh [--negation]"
      printf '%s\n' ""
      printf '%s\n' "  --negation  invierte la expectativa de paridad del corpus."
      printf '%s\n' "              Un harness que no puede fallar no prueba nada (#2056)."
      exit 0 ;;
    *)
      printf 'argumento desconocido: %s\n' "$arg" >&2
      exit 2 ;;
  esac
done

if [ ! -f "$PRE" ]; then
  printf 'FAIL: falta %s -- F1a todavia no esta escrito (RED esperado)\n' "$PRE" >&2
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

# expect_rc <desc> <esperado> <obtenido> [detalle]
expect_rc() {
  if [ "$2" = "$3" ]; then ok "$1" "rc=$3"; else bad "$1" "esperado rc=$2, obtenido rc=$3" "${4:-}"; fi
}

# expect_eq <desc> <obtenido> <esperado>
expect_eq() {
  if [ "$2" = "$3" ]; then ok "$1" "$2"; else bad "$1" "obtenido='$2' esperado='$3'"; fi
}

# expect_match <desc> <texto> <regex>  (regex extendida de grep)
# grep -E -- "$3" > /dev/null, NOT -q: with set -o pipefail, -q's early exit
# on the first match SIGPIPEs the printf when the text is larger than the
# pipe buffer (the 161 KB ci.yml) and the pipeline fails DESPITE the match.
expect_match() {
  if printf '%s\n' "$2" | grep -E -- "$3" > /dev/null; then ok "$1" "$2"; else bad "$1" "sin match /$3/ en: $2"; fi
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

# write_baseline <ruta:conteo>... [exc:ruta ...] -- UN mapa grandfathered;
# exc:ruta añade la entrada al mapa exceptions (el parser del guard la tolera:
# solo valida grandfathered y hard_limit).
write_baseline() {
  python3 -I - "$SB/scripts/quality-baselines.json" "$@" <<'PY'
import json, sys
out, specs = sys.argv[1], sys.argv[2:]
gf, exc = {}, {}
for s in specs:
    if s.startswith("exc:"):
        exc[s[4:]] = "mantainer: declarativo cohesivo (fixture)"
    else:
        path, count = s.rsplit(":", 1)
        gf[path] = int(count)
ratchet = {"hard_limit": 500, "grandfathered": dict(sorted(gf.items()))}
if exc:
    ratchet["exceptions"] = dict(sorted(exc.items()))
json.dump({"file_size_rust": ratchet}, open(out, "w", encoding="utf-8"), indent=2)
PY
}

# write_code <archivo> <n>: n funciones de una linea => el archivo mide n
write_code() {
  local f="$1" n="$2" i
  mkdir -p "$(dirname "$f")"
  : > "$f"
  for i in $(seq 1 "$n"); do
    printf 'pub fn f%s() {}\n' "$i" >> "$f"
  done
}

# append_test_mod <archivo> <n>: #[cfg(test)] + mod tests { ... } => n+4 lineas
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

run_pre() { # corre la COPIA del sandbox; deja stdout en OUT, rc en RC
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
expect_match "1.3 --help names the unit 'lineas' (wc -l)" "$OUT" "lineas"

# --- 2. the corpus is the guard's corpus, byte for byte -------------------------
# NEGATION A preflight that re-derives the corpus drifts from the ratchet one
# commit later, and only the drift is ever noticed. One corpus, one owner.

new_sandbox c2
GUARD_FIND="$(sed -n 's/^ *if ! \(find "\$ROOT\/crates" .*\) -print0 \\$/\1/p' "$GUARD" | head -1)"
GUARD_CORPUS="$(printf '%s' "$GUARD_FIND" | sed 's/find "\$ROOT\/crates"/find crates/')"
if [ -z "$GUARD_CORPUS" ]; then
  bad "2.0 el harness extrae el find del guard" "no match en $GUARD"
else
  ok "2.0 el harness extrae el find del guard" "$GUARD_CORPUS"
fi

run_pre --corpus-literal
EXPECT_CORPUS="$GUARD_CORPUS"
if [ "$NEGATION" = "1" ]; then
  EXPECT_CORPUS="$GUARD_CORPUS -maxdepth 1"
fi
expect_eq "2.1 --corpus-literal == el find del guard, byte a byte" "$OUT" "$EXPECT_CORPUS"

# --- 3. a missing path fails with a NAMED cause --------------------------------
# NEGATION: an empty block for a typo reads as "healthy file".

new_sandbox c3
run_pre --format "crates/webfang_x/src/nope.rs"
expect_rc "3.1 a missing path fails (rc=1)" 1 "$RC" "$ERRF"
expect_match "3.2 the cause is named, not a bare traceback" "$(cat "$ERRF" 2>/dev/null)" "NOT_FOUND|no existe|no se encontro"

run_pre --format
expect_rc "3.3 --format without a path is a usage error (rc=2)" 2 "$RC" "$ERRF"

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
  bad "6.1 the total command reproduces the total" "sin comando en la columna 4"
fi
if [ -n "$I_CMD" ]; then
  REPRO="$(cd "$SB" && eval "$I_CMD" 2>/dev/null | tr -d ' ' | head -1)"
  expect_eq "6.2 the inline-tests command reproduces the span" "$REPRO" "$I_SIM"
else
  bad "6.2 the inline-tests command reproduces the span" "sin comando en la columna 4"
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
expect_match "8.2 the span is declared indeterminate" "$OUT" "^inline_tests	indeterminado"
expect_match "8.3 ... with the cause, not a number" "$OUT" "^inline_tests_note	[^	]+"

# --- 9. churn outside a git repository is INDETERMINATE, never 0 ---------------
# NEGATION: "0 commits" and "cannot know" are different facts, and the whole
# playbook exists because the two were written as the same one.

new_sandbox c9
write_code "$SB/crates/webfang_x/src/e.rs" 520
run_pre --format "crates/webfang_x/src/e.rs"
expect_match "9.1 outside git the churn is indeterminate" "$OUT" "^churn_commits	indeterminado"

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
  bad "10.5 the cut is the one the plan describes" "esperado $EXPECT_SRC, real $REAL_SRC"
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

new_sandbox c12
write_code "$SB/crates/webfang_x/src/h.rs" 540
write_baseline "crates/webfang_x/src/h.rs:540"
# mueve 10 lineas pero el destino suma 20 de header: h queda en 530 <= 540, legal
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
expect_match "13.3 P states its comparator (strict >)" "$OUT" "(strict|estricto).*>"
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
# The mechanical §5 signals get fixtures; the semantic ones (módulos pegados,
# declarativo cohesivo, alto riesgo, arquitectónico) are maintainer calls per
# §5's own Señal column, so a file without mechanical signals says "revisar".

new_sandbox c18
{
  for i in $(seq 1 100); do printf 'pub fn p%s() {}\n' "$i"; done
  printf '#[cfg(test)]\nmod tests {\n    use super::*;\n'
  for i in $(seq 1 300); do printf '    fn t%s() { assert!(true); }\n' "$i"; done
  printf '}\n'
} > "$SB/crates/webfang_x/src/n.rs"
run_pre --format "crates/webfang_x/src/n.rs"
expect_match "16.1 inline > 50% suggests tests-mayoria" "$OUT" "^clasificacion	tests-mayoria"

new_sandbox c19
{
  printf '#[allow(clippy::too_many_lines)]\n'
  printf 'pub fn big() {\n'
  for i in $(seq 1 500); do printf '    let _%s = %s;\n' "$i" "$i"; done
  printf '}\n'
} > "$SB/crates/webfang_x/src/o.rs"
run_pre --format "crates/webfang_x/src/o.rs"
expect_match "16.2 an allow(too_many_lines) suggests funcion-gigante" "$OUT" "^clasificacion	funcion-gigante"

new_sandbox c20
write_code "$SB/crates/webfang_x/src/q.rs" 600
run_pre --format "crates/webfang_x/src/q.rs"
expect_match "16.3 production-heavy without signals says revisar" "$OUT" "^clasificacion	revisar"
expect_match "16.4 ... naming the semantic §5 signals" "$OUT" "^clasificacion_note	"

new_sandbox c21
write_code "$SB/crates/webfang_x/src/r.rs" 512
write_baseline "crates/webfang_x/src/r.rs:512" "exc:crates/webfang_x/src/r.rs"
run_pre --format "crates/webfang_x/src/r.rs"
expect_match "16.5 a declared exception says exception" "$OUT" "^clasificacion	exception"

# --- summary ------------------------------------------------------------------

echo "----------------------------------------"
echo "test_file_size_preflight: passed=$passed failed=$failed"
if [ "$failed" -gt 0 ]; then
  exit 1
fi
exit 0
