#!/usr/bin/env bash
# Ratchet gate for file size (the AGENTS.md "~500 lines" rule).
#
# AGENTS.md: "No new file should exceed ~500 lines, and if you touch one that
# already does, split it." Nothing enforced it: on main@3fcdd431 113 of 332
# non-test .rs files measure above 500 lines (largest: engine.rs at 4063). A
# hard gate would be red on day one, so the enforcement is a DESCENDING
# ratchet with a grandfathered map, the same pattern as the duplication
# ratchet (check_duplication.sh + quality-baselines.json, issue #516):
#
#   - a file NOT in the baseline fails above the hard limit (500 exactly
#     passes; the comparison is strictly `wc -l > hard_limit`);
#   - a file IN the baseline fails if it grows by a single line, fails once it
#     measures <= hard_limit (removing the entry consolidates the win), and
#     passes with a notice when it shrank but stays above the limit;
#   - a baseline entry whose file is not in the measured corpus (deleted, or
#     moved out of crates/**/src — e.g. into tests/) FAILS with the removal
#     command: a silent orphan would let the ratchet rot.
#
# The corpus is the AGENTS.md find expression, VERBATIM, so the script and the
# documented rule cannot diverge:
#
#   find crates -name '*.rs' -not -path '*/tests/*'
#
# (it also covers crates/webfang_core/build.rs, which measures below the
# limit; a `*/src/*` scoping would drop it — measured 2026-11-01.)
#
# The measurement is NUL-safe end to end (find -print0, sort -z) and counts
# with `wc -l < file` PER FILE, so no batch `total` line can ever be parsed as
# a path and filenames with spaces survive. The corpus is sorted so --generate
# output is deterministic. A corpus that cannot be measured (missing crates/,
# failing find, unreadable file, zero .rs files) FAILS CLOSED — zero files is
# never "all green".
#
# All JSON access goes through ONE embedded python reader/writer so the file's
# shape cannot drift between modes: 2-space indent, no trailing newline,
# top-level key order preserved, `grandfathered` keys sorted. Reads fail
# CLOSED with named causes (the same status\tvalue protocol as
# check_duplication.sh): a baseline that cannot be read is a red gate, never
# a green one.
#
# Usage:
#   bash scripts/check_file_size.sh                  # the gate (default)
#   bash scripts/check_file_size.sh --generate       # ONE-SHOT: build the
#                         # grandfathered map from the current tree (refuses
#                         # when the key already exists; regenerating means
#                         # deleting the key by hand, a reviewed edit)
#   bash scripts/check_file_size.sh --update path=550 [path=count ...]
#   bash scripts/check_file_size.sh --remove path [path ...]
#                         # DESCENDING-ONLY edits: --update lowers values and
#                         # --remove deletes entries; raising or adding an
#                         # entry is a manual JSON edit (a maintainer decision
#                         # visible in review, exactly like the duplication
#                         # ratchet). --update refuses counts <= hard_limit:
#                         # that path ends in --remove, not in a floor entry.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE_FILE="$ROOT/scripts/quality-baselines.json"
# The limit --generate WRITES. Every other mode reads it from the baseline:
# the JSON is the source of truth, never the script.
HARD_LIMIT_GENERATE=500

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

usage() {
  sed -n '42,55p' "$0"
}

die() {
  echo "::error::$1"
  exit 1
}

# --- the single JSON reader/writer --------------------------------------------
#
# One embedded python block owns every read and every write of the baseline,
# so the file's shape (indent, key order, trailing newline, ASCII escaping)
# is decided in exactly one place. Operations:
#
#   read    <baseline> <entries-out>   validate; write "path\tcount" lines
#                                      (sorted); emit OK\t<hard_limit>
#   generate <baseline> <offenders-tsv> <limit>
#                                      one-shot: add file_size_rust from the
#                                      measured offenders; refuses when the
#                                      key already exists
#   update  <baseline> <pairs-tsv>     descending-only lowers; ALL pairs are
#                                      validated before anything is written
#   remove  <baseline> <paths-tsv>     delete entries; validated the same way
#
# Every failure emits a NAMED status instead of a value; the caller decides
# the exit. The status\tvalue protocol matches check_duplication.sh.
baseline_tool() {
  python3 - "$@" <<'PY'
import json
import sys


def emit(status, value=""):
    print("%s\t%s" % (status, value))


def load(path):
    """Returns (status, value, raw, data). raw/data are None unless read."""
    try:
        with open(path) as fh:
            raw = fh.read()
    except FileNotFoundError:
        return "MISSING_FILE", path, None, None
    except OSError as exc:
        return "UNREADABLE", str(exc), None, None
    try:
        data = json.loads(raw)
    except ValueError as exc:
        return "BAD_JSON", str(exc), raw, None
    return "OK", raw, raw, data


def validate(data):
    """Returns (status, value, ratchet). status != OK -> ratchet is None."""
    if not isinstance(data, dict) or "file_size_rust" not in data:
        return "MISSING_KEY", "file_size_rust", None
    ratchet = data["file_size_rust"]
    if not isinstance(ratchet, dict):
        return "NOT_AN_OBJECT", "file_size_rust", None
    if "hard_limit" not in ratchet:
        return "MISSING_HARD_LIMIT", "file_size_rust.hard_limit", None
    limit = ratchet["hard_limit"]
    if isinstance(limit, bool) or not isinstance(limit, int):
        return "HARD_LIMIT_NOT_INT", repr(limit), None
    grandfathered = ratchet.get("grandfathered")
    if not isinstance(grandfathered, dict):
        return "GRANDFATHERED_NOT_OBJECT", "file_size_rust.grandfathered", None
    for key, value in grandfathered.items():
        if isinstance(value, bool) or not isinstance(value, int):
            return "ENTRY_NOT_INT", "%s=%r" % (key, value), None
    return "OK", str(limit), ratchet


def dump(path, data, raw):
    """Writes the file preserving its existing shape.

    2-space indent, no trailing newline, top-level key order preserved (dict
    insertion order), and the same ASCII escaping the file already uses.
    """
    ensure_ascii = all(ord(ch) < 128 for ch in raw)
    text = json.dumps(data, indent=2, ensure_ascii=ensure_ascii)
    with open(path, "w") as fh:
        fh.write(text)


op = sys.argv[1]
path = sys.argv[2]

if op == "read":
    status, value, _, data = load(path)
    if status != "OK":
        emit(status, value)
        sys.exit(0)
    status, limit, ratchet = validate(data)
    if status != "OK":
        emit(status, limit)
        sys.exit(0)
    with open(sys.argv[3], "w") as out:
        for key in sorted(ratchet["grandfathered"]):
            out.write("%s\t%s\n" % (key, ratchet["grandfathered"][key]))
    emit("OK", limit)

elif op == "generate":
    status, value, raw, data = load(path)
    if status == "OK":
        vstatus, _, _ = validate(data)
        if vstatus == "OK":
            emit("KEY_EXISTS", "file_size_rust")
            sys.exit(0)
        if vstatus != "MISSING_KEY":
            emit(vstatus, "pre-existing file_size_rust is invalid")
            sys.exit(0)
        # vstatus == MISSING_KEY: the one-shot case — the key does not exist
        # yet, which is exactly what --generate is for. Proceed below.
    else:
        emit(status, value)
        sys.exit(0)
    offenders = {}
    with open(sys.argv[3]) as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line:
                continue
            count, _, rel = line.partition("\t")
            offenders[rel] = int(count)
    data["file_size_rust"] = {
        "hard_limit": int(sys.argv[4]),
        "grandfathered": {k: offenders[k] for k in sorted(offenders)},
    }
    dump(path, data, raw)
    emit("OK", str(len(offenders)))

elif op in ("update", "remove"):
    status, value, raw, data = load(path)
    if status != "OK":
        emit(status, value)
        sys.exit(0)
    vstatus, vvalue, ratchet = validate(data)
    if vstatus != "OK":
        emit(vstatus, vvalue)
        sys.exit(0)
    grandfathered = ratchet["grandfathered"]
    refusals = []
    applied = 0
    with open(sys.argv[3]) as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line:
                continue
            key, _, rest = line.partition("\t")
            if op == "update":
                new = int(rest)
                if key not in grandfathered:
                    refusals.append(
                        "agregar una entrada es una edición manual del JSON "
                        "revisada por maintainer, no --update: %s" % key
                    )
                elif new <= ratchet["hard_limit"]:
                    refusals.append(
                        "count <= hard_limit no se guarda en el baseline "
                        "(usá --remove para consolidar la victoria): %s=%d"
                        % (key, new)
                    )
                elif new > grandfathered[key]:
                    refusals.append(
                        "subir un valor no es descendente "
                        "(%s: %d > %d); es una edición manual del JSON "
                        "revisada por maintainer" % (key, new, grandfathered[key])
                    )
                elif new == grandfathered[key]:
                    pass  # no-op: nothing to lower
                else:
                    grandfathered[key] = new
                    applied += 1
            else:  # remove
                if key not in grandfathered:
                    refusals.append(
                        "la entrada no existe en el baseline: %s" % key
                    )
                else:
                    del grandfathered[key]
                    applied += 1
    if refusals:
        emit("REFUSED", " | ".join(refusals))
        sys.exit(0)
    dump(path, data, raw)
    emit("OK", str(applied))

else:
    emit("UNKNOWN_OP", op)
    sys.exit(0)
PY
}

# --- the measurement -----------------------------------------------------------
#
# measure_corpus <out-file>: writes "count<TAB>relpath" lines for every .rs in
# the AGENTS.md corpus. On failure writes NOTHING to <out-file> and prints a
# NAMED cause ("STATUS" or "STATUS<TAB>detail"); the caller turns it into a
# red gate. NUL-safe traversal, per-file wc (no `total` line exists), sorted
# for deterministic --generate output.
measure_corpus() {
  local out="$1" f rel n
  if [[ ! -d "$ROOT/crates" ]]; then
    echo "NO_CRATES_DIR"
    return 1
  fi
  # The find expression is AGENTS.md's, TEXTUAL (see header).
  if ! find "$ROOT/crates" -name '*.rs' -not -path '*/tests/*' -print0 \
    > "$WORKDIR/files.nul"; then
    echo "FIND_FAILED"
    return 1
  fi
  sort -z "$WORKDIR/files.nul" > "$WORKDIR/files.sorted.nul"
  : > "$out"
  while IFS= read -r -d '' f; do
    rel="${f#"$ROOT"/}"
    if ! n="$(wc -l < "$f" 2>/dev/null)"; then
      echo "UNREADABLE_FILE	$rel"
      return 1
    fi
    printf '%s\t%s\n' "$((n))" "$rel" >> "$out"
  done < "$WORKDIR/files.sorted.nul"
  if [[ ! -s "$out" ]]; then
    echo "EMPTY_CORPUS"
    return 1
  fi
  return 0
}

run_measurement() {
  local measured_file="$1" err status detail
  if ! err="$(measure_corpus "$measured_file")"; then
    status="${err%%$'\t'*}"
    detail="${err#*$'\t'}"
    case "$status" in
      NO_CRATES_DIR)
        die "No existe $ROOT/crates: el corpus no se puede medir (fail-closed)."
        ;;
      FIND_FAILED)
        die "find falló sobre crates/: la medición no es confiable (fail-closed)."
        ;;
      UNREADABLE_FILE)
        die "No se puede contar $detail: la medición no es completa (fail-closed)."
        ;;
      EMPTY_CORPUS)
        die "El corpus midió 0 archivos .rs: no hay medición sobre la que decidir (fail-closed)."
        ;;
      *)
        die "Medición fallida ($status)."
        ;;
    esac
  fi
}

# --- mode dispatch ---------------------------------------------------------------

MODE="check"
UPDATE_ARGS=()
REMOVE_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --generate) MODE="generate" ;;
    # --update/--remove consume ALL remaining args (variadic lists; no further
    # flags after them, as documented in usage).
    --update) MODE="update"; shift; UPDATE_ARGS=("$@"); break ;;
    --remove) MODE="remove"; shift; REMOVE_ARGS=("$@"); break ;;
    -h | --help) usage; exit 0 ;;
    *) die "Argumento desconocido '$1' (ver --help)." ;;
  esac
  shift
done

case "$MODE" in
  generate)
    MEASURED="$WORKDIR/measured.tsv"
    run_measurement "$MEASURED"
    OFFENDERS="$WORKDIR/offenders.tsv"
    awk -F'\t' -v lim="$HARD_LIMIT_GENERATE" '$1 + 0 > lim + 0' "$MEASURED" > "$OFFENDERS"
    RAW="$(baseline_tool generate "$BASELINE_FILE" "$OFFENDERS" "$HARD_LIMIT_GENERATE")"
    STATUS="${RAW%%$'\t'*}"
    VALUE="${RAW#*$'\t'}"
    case "$STATUS" in
      OK)
        echo "Baseline escrito: $BASELINE_FILE"
        echo "  hard_limit: $HARD_LIMIT_GENERATE"
        echo "  entradas grandfathered: $VALUE"
        ;;
      KEY_EXISTS)
        die "--generate es de un solo uso: 'file_size_rust' ya existe en $BASELINE_FILE. Borrá la clave manualmente (es una edición revisada) si querés regenerar."
        ;;
      MISSING_FILE)
        die "Missing baseline file $BASELINE_FILE"
        ;;
      *)
        die "El baseline no se puede escribir ($STATUS): $VALUE"
        ;;
    esac
    ;;

  update | remove)
    if [[ "$MODE" == "update" ]]; then
      [[ ${#UPDATE_ARGS[@]} -gt 0 ]] || die "--update requiere al menos un par path=count."
      OPS="$WORKDIR/ops.tsv"
      : > "$OPS"
      for arg in "${UPDATE_ARGS[@]}"; do
        [[ "$arg" == *=* ]] || die "--update espera path=count, recibí '$arg'."
        pair_path="${arg%%=*}"
        pair_count="${arg#*=}"
        [[ "$pair_count" =~ ^[0-9]+$ ]] || die "El conteo de '$arg' no es un número entero."
        printf '%s\t%s\n' "$pair_path" "$pair_count" >> "$OPS"
      done
    else
      [[ ${#REMOVE_ARGS[@]} -gt 0 ]] || die "--remove requiere al menos un path."
      OPS="$WORKDIR/ops.tsv"
      : > "$OPS"
      for arg in "${REMOVE_ARGS[@]}"; do
        printf '%s\n' "$arg" >> "$OPS"
      done
    fi
    RAW="$(baseline_tool "$MODE" "$BASELINE_FILE" "$OPS")"
    STATUS="${RAW%%$'\t'*}"
    VALUE="${RAW#*$'\t'}"
    case "$STATUS" in
      OK) echo "OK: baseline actualizado ($VALUE cambio(s)): $BASELINE_FILE" ;;
      REFUSED) die "Refusado (nada fue escrito): $VALUE" ;;
      *)
        die "El baseline no se puede editar ($STATUS): $VALUE"
        ;;
    esac
    ;;

  check)
    # The baseline first (a baseline that cannot be read is a red gate), then
    # the measurement, then every rule with its named violation.
    ENTRIES="$WORKDIR/baseline.entries"
    RAW="$(baseline_tool read "$BASELINE_FILE" "$ENTRIES")"
    STATUS="${RAW%%$'\t'*}"
    VALUE="${RAW#*$'\t'}"
    case "$STATUS" in
      OK) HARD_LIMIT="$VALUE" ;;
      MISSING_FILE) die "Missing baseline file $BASELINE_FILE" ;;
      UNREADABLE) die "No se puede leer el baseline $BASELINE_FILE: $VALUE" ;;
      BAD_JSON) die "El baseline $BASELINE_FILE no es JSON válido: $VALUE" ;;
      MISSING_KEY) die "Falta la clave 'file_size_rust' en $BASELINE_FILE" ;;
      NOT_AN_OBJECT) die "'file_size_rust' no es un objeto en $BASELINE_FILE" ;;
      MISSING_HARD_LIMIT) die "Falta 'file_size_rust.hard_limit' en $BASELINE_FILE" ;;
      HARD_LIMIT_NOT_INT) die "'file_size_rust.hard_limit' no es un entero en $BASELINE_FILE: $VALUE" ;;
      GRANDFATHERED_NOT_OBJECT) die "'file_size_rust.grandfathered' no es un objeto en $BASELINE_FILE" ;;
      ENTRY_NOT_INT) die "Entrada no entera en 'file_size_rust.grandfathered': $VALUE" ;;
      *) die "No se puede leer el baseline ($STATUS)" ;;
    esac

    declare -A GF=()
    while IFS=$'\t' read -r entry_path entry_count; do
      GF["$entry_path"]="$entry_count"
    done < "$ENTRIES"

    MEASURED="$WORKDIR/measured.tsv"
    run_measurement "$MEASURED"
    declare -A MEASURED_MAP=()
    while IFS=$'\t' read -r m_count m_path; do
      MEASURED_MAP["$m_path"]="$m_count"
    done < "$MEASURED"

    VIOLATIONS=()
    NOTICES=()

    # Orphans first: an entry whose file left the measured corpus (deleted or
    # moved out of crates/**/src) would otherwise rot in the JSON forever.
    for gf_path in "${!GF[@]}"; do
      if [[ -z "${MEASURED_MAP["$gf_path"]+x}" ]]; then
        VIOLATIONS+=("La entrada del baseline '$gf_path' ya no está en el corpus medido (borrado o movido fuera de crates/**/src). Quitá esta entrada: bash scripts/check_file_size.sh --remove '$gf_path'")
      fi
    done

    for m_path in "${!MEASURED_MAP[@]}"; do
      m_lines="${MEASURED_MAP["$m_path"]}"
      if [[ -n "${GF["$m_path"]+x}" ]]; then
        gf_cap="${GF["$m_path"]}"
        if (( m_lines > gf_cap )); then
          VIOLATIONS+=("Archivo grandfathered CRECIÓ: $m_path ($m_lines > $gf_cap). No puede crecer ni una línea: dividí el archivo o revertí el cambio.")
        elif (( m_lines <= HARD_LIMIT )); then
          VIOLATIONS+=("Archivo grandfathered ahora mide <= $HARD_LIMIT líneas: $m_path ($m_lines). Consolidá la victoria quitándolo del baseline: bash scripts/check_file_size.sh --remove '$m_path'")
        elif (( m_lines < gf_cap )); then
          NOTICES+=("Se puede bajar el baseline: bash scripts/check_file_size.sh --update '$m_path=$m_lines' (de $gf_cap a $m_lines)")
        fi
      elif (( m_lines > HARD_LIMIT )); then
        VIOLATIONS+=("Archivo por encima del límite ($HARD_LIMIT líneas): $m_path ($m_lines). No está en el baseline: dividí el archivo (agregar un ofensor histórico es una edición manual del JSON revisada por maintainer).")
      fi
    done

    echo "Corpus medido: ${#MEASURED_MAP[@]} archivos .rs (límite duro: $HARD_LIMIT, grandfathered: ${#GF[@]})."
    for notice in "${NOTICES[@]}"; do
      echo "::notice::$notice"
    done
    if (( ${#VIOLATIONS[@]} > 0 )); then
      for violation in "${VIOLATIONS[@]}"; do
        echo "::error::$violation"
      done
      echo "Ratchet de tamaño de archivo: ROJO (${#VIOLATIONS[@]} violación(es))."
      exit 1
    fi
    echo "OK: el ratchet de tamaño de archivo se sostiene."
    ;;
esac
