#!/usr/bin/env bash
# ============================================================================
# AGENTS DOC — size + structure guard for AGENTS.md (#2839)
# ============================================================================
# AGENTS.md is the always-loaded router. It regrew 740 -> 862 lines after the
# 2026-10-21 routerization because agents append runbook prose on every commit;
# #2834 pruned it back under the owner-approved budget. Engram obs #368:
# "Sin un gate que falle, un doc largo no se auto-achica." This is that gate.
#
# Three structural classes, all machine-checkable (no prose reading):
#   1. SIZE BUDGET: lines <= agents_doc.max_lines AND bytes <= max_bytes,
#      read from scripts/quality-baselines.json. The budget lives in the JSON,
#      not here, so an agent cannot silently raise it to pass. DESCENDING-only
#      via --update; raising is a manual maintainer-reviewed JSON edit
#      (the file_size_rust ratchet philosophy, #516).
#   2. FENCE BALANCE: the count of ^``` fence lines is even (the file renders).
#   3. ZERO LINE-NUMBER CITATIONS: `path.ext:NNN` rots on every edit above it;
#      cite the symbol or the path. Extension-anchored regex avoids false
#      positives on #NNN, vX.Y.Z, 127.0.0.1:8080, :443. The per-line marker
#      <!-- line-cite:allow --> exempts an intentional edge case.
#
# Fail-closed: a missing AGENTS.md or a missing/unreadable/invalid baseline is
# a red gate, never a blind pass.
#
# Usage: scripts/check_agents_doc.sh
#        scripts/check_agents_doc.sh --update max_lines=N [max_bytes=M]
# Exit: 0 clean · 1 budget/structure violation, refused update, or unreadable input
# ============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

err() { printf '::error::%s\n' "$*" >&2; exit 1; }

DOC="AGENTS.md"
BASELINE="scripts/quality-baselines.json"

MODE="check"
UPDATE_ARGS=()
for arg in "$@"; do
  case "$arg" in
    --update) MODE="update" ;;
    *=*) UPDATE_ARGS+=("$arg") ;;
    *) err "argumento desconocido: '$arg' (uso: --update max_lines=N [max_bytes=M])." ;;
  esac
done

# One embedded python block owns every read and write of the baseline, like
# check_file_size.sh: statuses are machine-readable, the caller turns any
# non-OK into a red gate.
baseline_tool() {
  python3 - "$@" <<'PY'
import json
import sys


def emit(status, value=""):
    print("%s\t%s" % (status, value))


def load(path):
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
    if not isinstance(data, dict) or "agents_doc" not in data:
        return "MISSING_KEY", "agents_doc", None
    budget = data["agents_doc"]
    if not isinstance(budget, dict):
        return "NOT_AN_OBJECT", "agents_doc", None
    for key in ("max_lines", "max_bytes"):
        if key not in budget:
            return "MISSING_FIELD", "agents_doc.%s" % key, None
        value = budget[key]
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            return "NOT_POSITIVE_INT", "%s=%r" % (key, value), None
    return "OK", "%d\t%d" % (budget["max_lines"], budget["max_bytes"]), budget


def dump(path, data, raw):
    ensure_ascii = all(ord(ch) < 128 for ch in raw)
    with open(path, "w") as fh:
        fh.write(json.dumps(data, indent=2, ensure_ascii=ensure_ascii))


op, path = sys.argv[1], sys.argv[2]
status, value, raw, data = load(path)
if status != "OK":
    emit(status, value)
    sys.exit(0)
vstatus, vvalue, budget = validate(data)
if vstatus != "OK":
    emit(vstatus, vvalue)
    sys.exit(0)

if op == "read":
    emit("OK", vvalue)
elif op == "update":
    refusals = []
    applied = 0
    with open(sys.argv[3]) as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line:
                continue
            key, _, rest = line.partition("\t")
            if key not in ("max_lines", "max_bytes"):
                refusals.append(
                    "clave desconocida (solo max_lines/max_bytes): %s" % key)
                continue
            try:
                new = int(rest)
            except ValueError:
                refusals.append("valor no entero: %s=%s" % (key, rest))
                continue
            if new <= 0:
                refusals.append("valor no positivo: %s=%d" % (key, new))
            elif new > budget[key]:
                refusals.append(
                    "subir un valor no es descendente (%s: %d > %d); es una "
                    "edición manual del JSON revisada por maintainer"
                    % (key, new, budget[key]))
            elif new < budget[key]:
                budget[key] = new
                applied += 1
    if refusals:
        emit("REFUSED", " | ".join(refusals))
        sys.exit(0)
    dump(path, data, raw)
    emit("OK", str(applied))
else:
    emit("UNKNOWN_OP", op)
PY
}

read_budget() {
  local raw status value
  raw="$(baseline_tool read "$BASELINE")"
  status="${raw%%$'\t'*}"
  value="${raw#*$'\t'}"
  case "$status" in
    OK)
      MAX_LINES="${value%%$'\t'*}"
      MAX_BYTES="${value#*$'\t'}"
      ;;
    MISSING_FILE) err "no se encontró el baseline $BASELINE: sin presupuesto no hay gate (fail-closed)." ;;
    *) err "no se puede leer el presupuesto agents_doc de $BASELINE ($status): $value" ;;
  esac
}

if [[ "$MODE" == "update" ]]; then
  [[ ${#UPDATE_ARGS[@]} -gt 0 ]] || err "--update requiere al menos un par clave=valor (max_lines=N max_bytes=M)."
  OPS="$(mktemp)"
  trap 'rm -f "$OPS"' EXIT
  for arg in "${UPDATE_ARGS[@]}"; do
    printf '%s\t%s\n' "${arg%%=*}" "${arg#*=}" >> "$OPS"
  done
  RAW="$(baseline_tool update "$BASELINE" "$OPS")"
  STATUS="${RAW%%$'\t'*}"
  VALUE="${RAW#*$'\t'}"
  case "$STATUS" in
    OK) printf 'OK: baseline agents_doc actualizado (%s cambio(s) descendente(s)): %s\n' "$VALUE" "$BASELINE" ;;
    REFUSED) err "update rechazado: $VALUE" ;;
    *) err "no se puede editar el baseline ($STATUS): $VALUE" ;;
  esac
  exit 0
fi

[[ -f $DOC ]] || err "no se encontró $DOC en $ROOT: el gate no puede verificar nada (fail-closed)."
read_budget

FAILED=0
fail() { printf '::error::%s\n' "$*" >&2; FAILED=$((FAILED + 1)); }

# 1. Size budget.
LINES="$(wc -l < "$DOC")"
BYTES="$(wc -c < "$DOC")"
if (( LINES > MAX_LINES )); then
  fail "$DOC mide $LINES líneas y el presupuesto es $MAX_LINES. Recortá runbook hacia Engram (obs #370: el conocimiento vive allí); subir el presupuesto es una decisión de maintainer, no un --update."
fi
if (( BYTES > MAX_BYTES )); then
  fail "$DOC mide $BYTES bytes y el presupuesto es $MAX_BYTES. Recortá runbook hacia Engram; subir el presupuesto es una decisión de maintainer, no un --update."
fi

# 2. Fence balance: an odd count means an unclosed code block.
FENCES="$(grep -c '^```' "$DOC" || true)"
if (( FENCES % 2 != 0 )); then
  fail "$DOC tiene $FENCES líneas de fence (\`\`\`): número impar, hay un bloque de código sin cerrar."
fi

# 3. Line-number citations. Extension-anchored so #2307, v8.0.0,
# 127.0.0.1:8080 and :443 never match. The allow-marker exempts its own line.
CITE_RE='[A-Za-z0-9_./-]+\.(rs|sh|bash|toml|md|json|ya?ml|txt|tsv):[0-9]+'
while IFS= read -r hit; do
  [[ -z $hit ]] && continue
  line="${hit%%:*}"
  text="${hit#*:}"
  if [[ $text == *'<!-- line-cite:allow -->'* ]]; then
    continue
  fi
  fail "cita con número de línea en $DOC:$line — citá el símbolo o la ruta, no la línea (se invalida con cada edición encima). Marcador de excepción: <!-- line-cite:allow -->"
done < <(grep -nE "$CITE_RE" "$DOC" || true)

if (( FAILED > 0 )); then
  printf '::error::check_agents_doc: %d violación(es) de tamaño/estructura en %s.\n' "$FAILED" "$DOC" >&2
  exit 1
fi

printf 'OK: check_agents_doc: %s dentro del presupuesto y la estructura (%s líneas / %s bytes; presupuesto %s / %s; %s fences pares; sin citas de línea).\n' \
  "$DOC" "$LINES" "$BYTES" "$MAX_LINES" "$MAX_BYTES" "$FENCES"
