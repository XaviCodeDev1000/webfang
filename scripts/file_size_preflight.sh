#!/usr/bin/env bash
# file_size_preflight.sh -- the measurement/decision tool of the file-size
# policy v3 (umbrella #2584, phase F1a; ODD odd/tasks/file-size-preflight.md).
#
# The ratchet guard (check_file_size.sh) asks "is this file LEGAL". The
# preflight asks "what IS it, and what will a cut COST". Both answers used to
# live in prose, and the prose was wrong every time it was measured
# (crawl_loop "~490" vs 312, engine.rs 4063 vs 4064): until F1a no new split
# opens without this tool (policy 4.3).
#
# WHAT IT EMITS (--format, TAB separated, one fact per line): every number
# carries value, unit and the command that reproduces it ON THE SAME LINE
# (policy 6: "todo número lleva su unidad y el comando que lo produce en la
# misma línea"). Indeterminable facts say "indeterminado" and carry a note:
# "0 commits" and "cannot know" are different facts. The inline-tests span is
# LEXED (strings, chars, comments and lifetimes do not move it), never
# brace-counted; an unbalanced file reports its span as indeterminate instead
# of inventing one.
#
# DIFF_BUDGET (P) -- the constant of policy 6: an ADVISORY diff warning,
# strict comparator, never blocks, moves DOWN only by amendment. A political
# election (not a derived value): the four diffs measured by the 2026-11-17
# review, cited on eea59eaa:
#   2246 (add+del) #2901 · 3018 #2896 · 3933 #2856 · 5101 #2893
# The PR-count formula is policy 6: prs = ceil(2 * moved / P).
#
# Exit codes: 0 legal · 1 tool failure (named cause) · 2 usage ·
# 3 ratchet violation (--simulate) · 4 defective plan (--simulate, named).
#
# HERMETIC: nothing is ever written into the measured tree (all scratch lives
# in a mktemp dir). The corpus is NOT re-implemented: the find expression is
# extracted from check_file_size.sh at runtime; one corpus, one owner.
#
# The baseline is read through ONE embedded python3 reader (the guard's own
# pattern: named statuses, fail-closed). exceptions is a CLASSIFICATION signal
# only; it never exonerates a file from the limit (policy 5 and 7).

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE="$ROOT/scripts/quality-baselines.json"
GUARD="$ROOT/scripts/check_file_size.sh"

# Policy 6: see the header comment. Strictly `>` (like the guard), advisory.
DIFF_BUDGET=3000

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

declare -A GF=()      # ROOT-relative path -> grandfathered cap
declare -A EXC=()     # ROOT-relative path -> declared exception reason
declare -A PLAN_MOVED=()
declare -A PLAN_FROM=()
declare -A PLAN_TO=()
declare -A PLAN_HEADER=()
declare -A PLAN_CLIPPED=()

usage() {
  cat <<'USAGE'
Uso: bash scripts/file_size_preflight.sh [modo]

  (sin argumentos)      el bloque de presupuesto P (lo que corre ci.yml: --budget)
  --format <ruta>       el bloque de medición de un archivo (ruta repo-relativa, como la usa el guard)
  --simulate <plan>...  con --format <fuente>: calcula el corte; plan <dest>:<FROM>-<TO>[:header=<N>]
  --glue <archivo>=<N>  con --simulate: líneas NO movidas que el split añade a ese archivo
                        (imports, re-exports; el archivo es la fuente o un destino del plan)
  --corpus-literal      la expresión find del guard, byte a byte
  --help                esta ayuda

Modos: --format --simulate --corpus-literal --budget; las unidades son lineas.
USAGE
}

die_tool() {   # rc=1: the tool cannot answer, with a NAMED cause
  printf 'error: %s\n' "$1" >&2
  exit 1
}

usage_error() {  # rc=2
  printf 'error: %s\n' "$1" >&2
  usage >&2
  exit 2
}

emit2() { printf '%s\t%s\n' "$1" "$2"; }
emit4() { printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4"; }

# --- the corpus lives in the guard ---------------------------------------------
#
# extract_corpus: pulls the find expression out of check_file_size.sh VERBATIM
# (the same line the harness extracts), so the two tools can never drift. A
# guard whose line changed shape is a tool failure, never a re-implementation.
extract_corpus() {
  [ -f "$GUARD" ] || die_tool "no encuentro el guard $GUARD: el corpus del preflight vive en el guard y no se re-implementa"
  # the sed programs below keep \$ROOT literal ON PURPOSE: it is the guard's
  # own text being extracted, not an expansion.
  # shellcheck disable=SC2016
  EXEC_FIND="$(sed -n 's/^ *if ! \(find "\$ROOT\/crates" .*-print0\) \\$/\1/p' "$GUARD" | head -1)"
  [ -n "${EXEC_FIND:-}" ] || die_tool "no pude extraer la expresión find de $GUARD (cambió de forma): el corpus no se re-implementa, actualizá la extracción junto con el guard"
  # shellcheck disable=SC2016
  CORPUS_LITERAL="$(printf '%s' "$EXEC_FIND" | sed 's/find "\$ROOT\/crates"/find crates/; s/ -print0$//')"
}

CORPUS_READY=0

# ensure_corpus: runs the guard's find ONCE, NUL-safe into scratch. A find
# that fails is a tool failure (fail-closed), never "not in corpus".
ensure_corpus() {
  [ "$CORPUS_READY" -eq 1 ] && return 0
  eval "$EXEC_FIND" > "$WORK/corpus.nul" 2>/dev/null || die_tool "find falló sobre crates/: el corpus no se puede medir (fail-closed)"
  CORPUS_READY=1
}

# corpus_contains <rel>: 0 if <rel> is in the guard's corpus.
corpus_contains() {
  ensure_corpus
  local f
  while IFS= read -r -d '' f; do
    [ "${f#"$ROOT"/}" = "$1" ] && return 0
  done < "$WORK/corpus.nul"
  return 1
}

# --- the single baseline reader -------------------------------------------------
#
# One embedded python3 reader (the guard's own pattern: one reader, named
# statuses, fail-closed) fills WORK/gf.tsv ("path<TAB>count") and WORK/exc.tsv
# ("path<TAB>reason").
baseline_read() {
  local raw status
  local py3_rc=0
  raw="$(python3 -I - "$BASELINE" "$WORK/gf.tsv" "$WORK/exc.tsv" <<'PY'
import json
import sys


def emit(status, value=""):
    print("%s\t%s" % (status, value))
    sys.exit(0)


path, og, oe = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    with open(path) as fh:
        data = json.load(fh)
except FileNotFoundError:
    emit("MISSING_FILE", path)
except OSError as exc:
    emit("UNREADABLE", str(exc))
except ValueError as exc:
    emit("BAD_JSON", str(exc))
if not isinstance(data, dict) or "file_size_rust" not in data:
    emit("MISSING_KEY", "file_size_rust")
ratchet = data["file_size_rust"]
if not isinstance(ratchet, dict):
    emit("NOT_AN_OBJECT", "file_size_rust")
if "hard_limit" not in ratchet:
    emit("MISSING_HARD_LIMIT", "file_size_rust.hard_limit")
limit = ratchet["hard_limit"]
if isinstance(limit, bool) or not isinstance(limit, int):
    emit("HARD_LIMIT_NOT_INT", repr(limit))
gf = ratchet.get("grandfathered")
if not isinstance(gf, dict):
    emit("GRANDFATHERED_NOT_OBJECT", "file_size_rust.grandfathered")
for key, value in gf.items():
    if isinstance(value, bool) or not isinstance(value, int):
        emit("ENTRY_NOT_INT", "%s=%r" % (key, value))
exc = ratchet.get("exceptions", {})
if not isinstance(exc, dict):
    emit("EXCEPTIONS_NOT_OBJECT", "file_size_rust.exceptions")
with open(og, "w") as out:
    for key in sorted(gf):
        out.write("%s\t%s\n" % (key, gf[key]))
with open(oe, "w") as out:
    for key in sorted(exc):
        out.write("%s\t%s\n" % (key, exc[key]))
emit("OK", str(limit))
PY
)" || py3_rc=$?
  if [ "$py3_rc" -ne 0 ] && [ -z "$raw" ]; then
    die_tool "python3 falló silenciosamente (rc=$py3_rc): el preflight no inventa una medición sin su lector (fail-closed)"
  fi
  status="${raw%%$'\t'*}"
  case "$status" in
    OK) BASELINE_LIMIT="${raw#*$'\t'}" ;;
    MISSING_FILE) die_tool "no existe el baseline $BASELINE" ;;
    UNREADABLE) die_tool "no se puede leer el baseline $BASELINE: ${raw#*$'\t'}" ;;
    BAD_JSON) die_tool "el baseline $BASELINE no es JSON válido: ${raw#*$'\t'}" ;;
    *) die_tool "el baseline $BASELINE no se puede leer ($status): ${raw#*$'\t'}" ;;
  esac
}

load_baseline_maps() {
  local p c r
  while IFS=$'\t' read -r p c; do
    [ -n "$p" ] && GF["$p"]="$c"
  done < "$WORK/gf.tsv"
  while IFS=$'\t' read -r p r; do
    [ -n "$p" ] && EXC["$p"]="$r"
  done < "$WORK/exc.tsv"
}

# --- the inline-tests span lexer -------------------------------------------------
#
# lex_span <abs-path>: prints "SPANS<TAB>s:e s:e ...<TAB>total" (1-based,
# inclusive) or "INDETERMINATE<TAB><reason>". The span starts at the
# #[cfg(test)] attribute line and closes at the mod's closing brace. Strings,
# byte strings, raw strings, comments and lifetimes never move it.
lex_span() {
  python3 -I - "$1" <<'PY'
import re
import sys

path = sys.argv[1]
try:
    with open(path, encoding="utf-8", errors="replace") as fh:
        text = fh.read()
except OSError as exc:
    print("INDETERMINATE\t%s" % exc)
    sys.exit(0)

lines = text.split("\n")
if lines and lines[-1] == "":
    lines.pop()

CODE, LC, BC, STR, RAW = range(5)
STATE_NAMES = {
    BC: "comentario de bloque", STR: "string", RAW: "raw string",
}
state = CODE
raw_hashes = 0
block_depth = 0
depth = 0
reason = None

row_code = []
row_start = []
row_end = []

for raw_line in lines:
    start = depth
    buf = []
    i = 0
    L = raw_line
    m = len(L)
    while i < m:
        c = L[i]
        if state == CODE:
            if c == "/" and i + 1 < m and L[i + 1] == "/":
                state = LC
                buf.append(" " * (m - i))
                i = m
                continue
            if c == "/" and i + 1 < m and L[i + 1] == "*":
                state = BC
                block_depth = 1
                buf.append("  ")
                i += 2
                continue
            if c == '"':
                state = STR
                buf.append(" ")
                i += 1
                continue
            if c in ("r", "b"):
                k = i
                is_raw = False
                if L[k] == "b":
                    k += 1
                if k < m and L[k] == "r":
                    k += 1
                    is_raw = True
                if is_raw and k < m:
                    # r"..." (0 hashes) or r#*"..."#* : the hash run sits
                    # BETWEEN the r and the quote (r#"{ ... }"# — the JSON-in-
                    # raw-string shape). No escapes inside; the closing mark
                    # is '"' plus the same run. Multi-line: RAW persists.
                    hh = 0
                    while k + hh < m and L[k + hh] == "#":
                        hh += 1
                    if k + hh < m and L[k + hh] == '"':
                        state = RAW
                        raw_hashes = hh
                        buf.append(" " * (k + hh + 1 - i))
                        i = k + hh + 1
                        continue
                elif k < m and L[k] == '"':
                    state = STR  # b"..." : escapes apply
                    buf.append(" " * (k + 1 - i))
                    i = k + 1
                    continue
                buf.append(c)
                i += 1
                continue
            if c == "'":
                # char literals are consumed INLINE (they never span lines, so
                # no state is entered); anything that is not a complete literal
                # is a lifetime, and the quote itself is not a brace.
                if i + 1 < m and L[i + 1] == "\\":
                    if i + 3 < m and L[i + 3] == "'" and L[i + 2] != "u":
                        buf.append("    ")
                        i += 4
                        continue
                    if i + 3 < m and L[i + 2] == "u" and L[i + 3] == "{":
                        close = L.find("}", i + 4)
                        if close != -1 and close + 1 < m and L[close + 1] == "'":
                            buf.append(" " * (close + 2 - i))
                            i = close + 2
                            continue
                    i += 1  # a lifetime, not a char literal
                    buf.append(" ")
                    continue
                if i + 2 < m and L[i + 2] == "'":
                    buf.append("   ")
                    i += 3
                    continue
                i += 1  # a lifetime: the quote itself is not a brace
                buf.append(" ")
                continue
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth < 0:
                    reason = "llave de cierre sin apertura"
            buf.append(c)
            i += 1
        elif state == LC:
            buf.append(" " * (m - i))
            i = m
            continue
        elif state == BC:
            j = i
            while j < m:
                if L[j] == "/" and j + 1 < m and L[j + 1] == "*":
                    block_depth += 1
                    j += 2
                elif L[j] == "*" and j + 1 < m and L[j + 1] == "/":
                    block_depth -= 1
                    j += 2
                    if block_depth == 0:
                        state = CODE
                        break
                else:
                    j += 1
            buf.append(" " * (min(j, m) - i))
            i = min(j, m)
            continue
        elif state == STR:
            j = i
            while j < m:
                if L[j] == "\\":
                    j += 2
                    continue
                if L[j] == '"':
                    j += 1
                    state = CODE
                    break
                j += 1
            buf.append(" " * (min(j, m) - i))
            i = min(j, m)
            continue
        elif state == RAW:
            j = i
            while j < m:
                if L[j] == '"':
                    h = 0
                    while j + 1 + h < m and L[j + 1 + h] == "#":
                        h += 1
                    if h == raw_hashes:
                        j = j + 1 + h
                        state = CODE
                        break
                j += 1
            buf.append(" " * (min(j, m) - i))
            i = min(j, m)
            continue
    row_code.append("".join(buf))
    row_start.append(start)
    row_end.append(depth)
    if state == LC:  # a line comment ends at the newline
        state = CODE

if depth != 0 or state in (BC, STR, RAW) or reason is not None:
    reason = reason or "el archivo termina en un estado no cerrado (%s)" % STATE_NAMES.get(
        state, "profundidad de llaves %d" % depth
    )
    print("INDETERMINATE\t%s" % reason)
    sys.exit(0)

mod_re = re.compile(r"^\s*(pub(\s*\([a-zA-Z ]+\))?\s+)?mod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{\s*$")
attr_re = re.compile(r"^\s*#\[.*\]\s*$")
spans = []
for idx, code in enumerate(row_code):
    if row_start[idx] != 0 or row_end[idx] != 1:
        continue
    if not mod_re.match(code):
        continue
    end = None
    for j in range(idx + 1, len(row_code)):
        if row_end[j] == 0:
            end = j
            break
    if end is None:
        print("INDETERMINATE\tel mod de la línea %d abre llaves que nunca se cierran" % (idx + 1))
        sys.exit(0)
    a = idx
    while a - 1 >= 0 and attr_re.match(row_code[a - 1]):
        a -= 1
    spans.append((a, end))

if not spans:
    print("SPANS\t\t0")
    sys.exit(0)

# a bare #[test]/#[cfg(test)] item OUTSIDE the mod spans (depth 0) means the
# inline-test figure would UNDERCOUNT: the span covers what it covers and the
# rest is tests too. A determinate-but-wrong number is worse than none: the
# span goes indeterminate with the first offender's line.
outside_attr_re = re.compile(r"^\s*(#\[\s*cfg\s*\(\s*test|\#\[test\])")
last_end = spans[-1][1]
for idx in range(last_end + 1, len(row_code)):
    if row_start[idx] != 0:
        continue
    if outside_attr_re.match(row_code[idx]):
        print(
            "INDETERMINATE\tla línea %d tiene un test #[test]/#[cfg(test)] FUERA de los mod spans (terminan en %d): el total de tests inline sería un subconteo"
            % (idx + 1, last_end + 1)
        )
        sys.exit(0)

pairs = " ".join("%d:%d" % (s + 1, e + 1) for (s, e) in spans)
total = sum(e - s + 1 for (s, e) in spans)
print("SPANS\t%s\t%d" % (pairs, total))
PY
}

# range_cmd <rel> <s-e ...> -- a command reproducing the summed line count of
# 1-based inclusive ranges; the single-range form stays plain awk.
range_cmd() {
  local rel="$1"
  shift
  if [ "$#" -eq 0 ]; then
    printf "awk 'NR>=1 && NR<=0' '%s' | wc -l" "$rel"
    return 0
  fi
  local parts=() seg
  for seg in "$@"; do
    parts+=("awk 'NR>=${seg%%-*} && NR<=${seg##*-}' '$rel'")
  done
  if [ "${#parts[@]}" -eq 1 ]; then
    printf '%s | wc -l' "${parts[0]}"
  else
    printf '( %s ) | wc -l' "$(printf '%s; ' "${parts[@]}" | sed 's/; $//')"
  fi
}

# --- argument parsing ------------------------------------------------------------

MODE="budget"
FORMAT_PATH=""
PLAN_ARGS=()
GLUE_ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --format)
      [ $# -ge 2 ] || usage_error "--format requiere la ruta a medir"
      FORMAT_PATH="$2"
      # --format is the SOURCE for --simulate; it must not clobber the mode
      [ "$MODE" = "simulate" ] || MODE="format"
      shift 2
      ;;
    --simulate)
      MODE="simulate"
      shift
      while [ $# -gt 0 ] && [[ "$1" != -* ]]; do
        PLAN_ARGS+=("$1")
        shift
      done
      [ "${#PLAN_ARGS[@]}" -gt 0 ] || usage_error "--simulate requiere al menos un plan <dest>:<FROM>-<TO>[:header=<N>]"
      ;;
    --glue)
      [ $# -ge 2 ] || usage_error "--glue requiere <archivo>=<N> (líneas no movidas que el split añade a ese archivo)"
      GLUE_ARGS+=("$2")
      shift 2
      ;;
    --corpus-literal)
      MODE="corpus"
      shift
      ;;
    --budget)
      MODE="budget"
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage_error "argumento desconocido: '$1'"
      ;;
  esac
done

case "$MODE" in

  corpus)
    extract_corpus
    printf '%s\n' "$CORPUS_LITERAL"
    ;;

  budget)
    emit4 "presupuesto_diff" "$DIFF_BUDGET" "(add+del)" "aviso en diff estricto > $DIFF_BUDGET (add+del); no bloquea; solo cambia a la baja"
    emit4 "muestra" "2246" "(add+del)" "gh pr view 2901 --json additions,deletions -q '.additions+.deletions'"
    emit4 "muestra" "3018" "(add+del)" "gh pr view 2896 --json additions,deletions -q '.additions+.deletions'"
    emit4 "muestra" "3933" "(add+del)" "gh pr view 2856 --json additions,deletions -q '.additions+.deletions'"
    emit4 "muestra" "5101" "(add+del)" "gh pr view 2893 --json additions,deletions -q '.additions+.deletions'"
    emit4 "prs" "0" "PRs" "ceil(2*moved/$DIFF_BUDGET); moved=0 sin plan"
    ;;

  format | simulate)
    [ -n "$FORMAT_PATH" ] || usage_error "el modo necesita --format <ruta> (la fuente de la medición y de los rangos)"
    command -v python3 >/dev/null 2>&1 || die_tool "python3 no está disponible: el lector del baseline y el lexer del preflight lo necesitan (instalalo o corré en una estación con python3)"
    extract_corpus
    baseline_read
    load_baseline_maps

    # resolve the path ROOT-relative (the guard's vocabulary); an absolute
    # path under ROOT is accepted as-is.
    FORMAT_PATH="${FORMAT_PATH#"$ROOT"/}"
    ABSPATH="$ROOT/$FORMAT_PATH"
    [ -f "$ABSPATH" ] || die_tool "el archivo no existe: $FORMAT_PATH (¿es la ruta repo-relativa que usa el guard?)"
    [ -r "$ABSPATH" ] || die_tool "el archivo no se puede leer: $FORMAT_PATH"
    REL="$FORMAT_PATH"

    if corpus_contains "$REL"; then
      IN_CORPUS=1
    else
      IN_CORPUS=0
    fi

    TOTAL="$(wc -l < "$ABSPATH")"
    emit2 "path" "$REL"
    if [ "$IN_CORPUS" -eq 1 ]; then
      emit2 "in_corpus" "true"
    else
      emit2 "in_corpus" "false"
    fi
    emit4 "total" "$TOTAL" "lineas" "wc -l < '$REL'"

    LEXED="$(lex_span "$ABSPATH")"
    LEX_TAG="${LEXED%%$'\t'*}"
    LEX_REST="${LEXED#*$'\t'}"
    PAIRS="${LEX_REST%%$'\t'*}"
    SPAN_TOTAL="${LEX_REST##*$'\t'}"
    INLINE_N=""
    if [ "$LEX_TAG" = "INDETERMINATE" ]; then
      emit4 "production" "indeterminado" "-" "-"
      emit4 "inline_tests" "indeterminado" "-" "-"
      emit2 "inline_tests_note" "$PAIRS: el span no se inventa, medí el archivo reparado"
    else
      SPAN_RANGES=()
      GAP_RANGES=()
      PREV=1
      if [ -n "$PAIRS" ]; then
        for pair in $PAIRS; do
          S="${pair%%:*}"
          E="${pair##*:}"
          SPAN_RANGES+=("$S-$E")
          [ "$S" -gt "$PREV" ] && GAP_RANGES+=("$PREV-$((S - 1))")
          PREV=$((E + 1))
        done
      fi
      [ "$PREV" -le "$TOTAL" ] && GAP_RANGES+=("$PREV-$TOTAL")
      INLINE_N="$SPAN_TOTAL"
      PROD_N=$((TOTAL - INLINE_N))
      if [ "${#SPAN_RANGES[@]}" -gt 0 ]; then
        emit4 "production" "$PROD_N" "lineas" "$(range_cmd "$REL" "${GAP_RANGES[@]}")"
        emit4 "inline_tests" "$INLINE_N" "lineas" "$(range_cmd "$REL" "${SPAN_RANGES[@]}")"
      else
        emit4 "production" "$PROD_N" "lineas" "wc -l < '$REL'"
        emit4 "inline_tests" "0" "lineas" "$(range_cmd "$REL")"
      fi
    fi

    CHURN="$(git -C "$ROOT" rev-list --count HEAD -- "$REL" 2>/dev/null)"
    if [ -n "$CHURN" ]; then
      emit4 "churn_commits" "$CHURN" "commits" "git -C '$ROOT' rev-list --count HEAD -- '$REL'"
    else
      emit4 "churn_commits" "indeterminado" "-" "-"
      emit2 "churn_commits_note" "no es un repo git o no tiene HEAD: 0 y no-puedo-saber son hechos distintos"
    fi

    ALLOW_N="$(grep -c 'allow(clippy::too_many_lines)' "$ABSPATH" 2>/dev/null)" || true
    ALLOW_N="${ALLOW_N:-0}"
    emit4 "allow" "$ALLOW_N" "attrs" "grep -c 'allow(clippy::too_many_lines)' '$REL'"

    emit4 "funciones" "" "-" "reservada (F1a): requiere clippy/rustc, no disponible en el preflight offline"
    emit4 "blast_radius" "indeterminado" "-" "-"
    emit2 "blast_radius_note" "codegraph explore / codedb callers sobre '$REL' (herramientas de inteligencia; el preflight es offline)"
    emit4 "overlap_prs" "indeterminado" "-" "-"
    emit2 "overlap_prs_note" "gh pr list --state open y scripts/ci_pr_overlap.sh <N1> <N2>; el preflight es offline"

    # the suggested §5 classification: MEASURED predicates only. The semantic
    # §5 signals (módulos pegados, declarativo cohesivo, alto riesgo,
    # arquitectónico) are maintainer calls per §5's own Señal column.
    if [ "$IN_CORPUS" -eq 0 ]; then
      emit2 "clasificacion" "na"
      emit2 "clasificacion_note" "fuera del corpus del ratchet (el find del guard); sin clasificación §5"
    elif [ -n "${EXC["$REL"]:-}" ]; then
      emit2 "clasificacion" "exception"
      emit2 "clasificacion_note" "declarada en el mapa exceptions del baseline; no exonera del límite ni del net-zero"
    elif [ -n "$INLINE_N" ] && [ "$((2 * INLINE_N))" -gt "$TOTAL" ]; then
      emit2 "clasificacion" "tests-mayoria"
    elif [ "$ALLOW_N" -gt 0 ]; then
      emit2 "clasificacion" "funcion-gigante"
    else
      emit2 "clasificacion" "revisar"
      emit2 "clasificacion_note" "señales §5 no mecánicas (módulos pegados, declarativo cohesivo, alto riesgo, arquitectónico): decisión de revisión"
    fi

    [ "$MODE" = "format" ] && exit 0

    # --- simulate: plan arithmetic first, then the verdict -------------------
    # Policy 4.3: "--simulate toma el plan (destino ← rangos) y calcula el
    # tamaño resultante de cada archivo con wc -l total, más cabeceras."
    PARENT="$(dirname "$ABSPATH")"
    BASE_DIR="$(dirname "$PARENT")"
    [ "$BASE_DIR" = "$PARENT" ] && BASE_DIR="$PARENT"  # a root-level file
    SRC_KEY="${ABSPATH#"$BASE_DIR"/}"

    PLAN_ORDER=()
    DEFECTS=()
    VIOLATIONS=()
    NOTICES=()

    for plan in "${PLAN_ARGS[@]}"; do
      if [[ ! "$plan" =~ ^([^:]+):([0-9]+)-([0-9]+)(:header=([0-9]+))?$ ]]; then
        DEFECTS+=("plan malformado: '$plan' (formato <dest>:<FROM>-<TO>[:header=<N>])")
        continue
      fi
      DEST="${BASH_REMATCH[1]}"
      FROM="${BASH_REMATCH[2]}"
      TO="${BASH_REMATCH[3]}"
      HEADER="${BASH_REMATCH[5]:-0}"
      if [ "$FROM" -eq 0 ] || [ "$FROM" -gt "$TO" ]; then
        DEFECTS+=("rango invertido en '$plan': las líneas son 1-based y FROM ≤ TO")
        continue
      fi
      if [ "$DEST" = "$SRC_KEY" ]; then
        DEFECTS+=("destino == fuente en '$plan': mover líneas de un archivo a sí mismo no es un corte")
        continue
      fi
      # overlap on the RAW ranges, pairwise against the accepted plans
      overlap=0
      for other in "${PLAN_ORDER[@]}"; do
        if [ "$FROM" -le "${PLAN_TO[$other]}" ] && [ "${PLAN_FROM[$other]}" -le "$TO" ]; then
          DEFECTS+=("rangos solapados: '$plan' contra el plan de '$other' (mismo archivo fuente)")
          overlap=1
          break
        fi
      done
      [ "$overlap" -eq 1 ] && continue
      PLAN_FROM["$DEST"]="$FROM"
      PLAN_TO["$DEST"]="$TO"
      PLAN_HEADER["$DEST"]="$HEADER"
      PLAN_ORDER+=("$DEST")
      # prs arithmetic uses the range CLAMPED to EOF (harness 13.6: a range
      # past EOF is a defect AND prs counts the lines that do exist)
      MOVED=$((TO - FROM + 1))
      if [ "$TO" -gt "$TOTAL" ]; then
        MOVED=$((TOTAL - FROM + 1))
        [ "$MOVED" -lt 0 ] && MOVED=0
        PLAN_CLIPPED["$DEST"]=1
        DEFECTS+=("rango pasado EOF en '$plan': la fuente mide $TOTAL líneas")
      fi
      PLAN_MOVED["$DEST"]="$MOVED"
    done

    # --- glue (§4.3 "más cabeceras"): the non-moved lines a real split adds --
    # (import blocks, re-exports). It may target a plan destination or the
    # source itself — the server.rs split added 29 lines of glue to the
    # source, which is exactly the delta a pure-move model cannot see.
    declare -A GLUE=()
    for gspec in "${GLUE_ARGS[@]}"; do
      if [[ ! "$gspec" =~ ^([^=]+)=([0-9]+)$ ]]; then
        DEFECTS+=("glue malformado: '$gspec' (formato <archivo>=<N>, N ≥ 0)")
        continue
      fi
      GKEY="${BASH_REMATCH[1]}"
      GVAL="${BASH_REMATCH[2]}"
      if [ "$GKEY" != "$SRC_KEY" ] && [[ ! " ${PLAN_ORDER[*]} " == *" $GKEY "* ]]; then
        DEFECTS+=("glue sobre un archivo que el plan no toca: '$GKEY' (el plan mueve hacia ${#PLAN_ORDER[@]} destino(s) y la fuente '$SRC_KEY')")
        continue
      fi
      GLUE["$GKEY"]="$GVAL"
    done

    # plan rows: per destination, then the source once
    TOTAL_MOVED=0
    for dest in "${PLAN_ORDER[@]}"; do
      FROM="${PLAN_FROM[$dest]}"
      MOVED="${PLAN_MOVED[$dest]}"
      HEADER="${PLAN_HEADER[$dest]}"
      TOTAL_MOVED=$((TOTAL_MOVED + MOVED))
      DEST_ABS="$BASE_DIR/$dest"
      DEST_REL="${DEST_ABS#"$ROOT"/}"
      if [ -f "$DEST_ABS" ]; then
        DEST_CUR="$(wc -l < "$DEST_ABS")"
      else
        DEST_CUR=0
      fi
      DEST_FINAL=$((DEST_CUR + HEADER + MOVED + ${GLUE["$dest"]:-0}))
      NOTE="cabecera $HEADER + $MOVED movidas"
      [ -n "${PLAN_CLIPPED[$dest]:-}" ] && NOTE="$NOTE (recortado a EOF)"
      [ -n "${GLUE["$dest"]:-}" ] && NOTE="$NOTE + glue ${GLUE[$dest]}"
      if [ -n "${GF["$DEST_REL"]:-}" ]; then
        CAP="${GF["$DEST_REL"]}"
        CAP_DESC="su baseline ($CAP)"
      else
        CAP="$BASELINE_LIMIT"
        CAP_DESC="hard_limit ($CAP)"
      fi
      emit4 "plan" "$dest" "$DEST_FINAL" "lineas" "$NOTE"
      if [ "$DEST_FINAL" -gt "$CAP" ]; then
        if [ -n "${GF["$DEST_REL"]:-}" ]; then
          VIOLATIONS+=("el destino '$dest' quedaría en $DEST_FINAL > $CAP_DESC: grandfathered no puede crecer")
        else
          VIOLATIONS+=("el destino '$dest' quedaría en $DEST_FINAL > $CAP_DESC y no está grandfathered: dividí el plan en extracciones más chicas")
        fi
      fi
    done

    SRC_FINAL=$((TOTAL - TOTAL_MOVED + ${GLUE["$SRC_KEY"]:-0}))
    emit4 "plan" "$SRC_KEY" "$SRC_FINAL" "lineas" "$TOTAL líneas - $TOTAL_MOVED movidas${GLUE["$SRC_KEY"]:+ + glue ${GLUE[$SRC_KEY]}}"

    if [ "${#PLAN_ORDER[@]}" -gt 0 ]; then
      PRS=$(((2 * TOTAL_MOVED + DIFF_BUDGET - 1) / DIFF_BUDGET))
      emit4 "prs" "$PRS" "PRs" "ceil(2*$TOTAL_MOVED/$DIFF_BUDGET)"
    fi

    # ratchet verdicts on the source
    if [ "$TOTAL" -le "$BASELINE_LIMIT" ] && [ -z "${GF["$REL"]:-}" ]; then
      VIOLATIONS+=("no se preflighta: '$REL' mide <= hard_limit ($BASELINE_LIMIT); si está grandfathered a ≤ límite consolidá con --remove, dividirlo es aritmética forzada")
    fi
    if [ -n "${GF["$REL"]:-}" ] && [ "$SRC_FINAL" -le "$BASELINE_LIMIT" ]; then
      NOTICES+=("la fuente quedaría en $SRC_FINAL <= hard_limit ($BASELINE_LIMIT): consolidá la victoria con bash scripts/check_file_size.sh --remove '$REL' en el mismo PR")
    fi

    for notice in "${NOTICES[@]}"; do
      printf 'aviso: %s\n' "$notice" >&2
    done
    for defect in "${DEFECTS[@]}"; do
      printf 'defecto: %s\n' "$defect" >&2
    done
    for violation in "${VIOLATIONS[@]}"; do
      printf 'violación: %s\n' "$violation" >&2
    done
    if [ "${#DEFECTS[@]}" -gt 0 ]; then
      exit 4
    fi
    if [ "${#VIOLATIONS[@]}" -gt 0 ]; then
      exit 3
    fi
    exit 0
    ;;
esac
