#!/usr/bin/env bash
# ============================================================================
# DATA-FLOW DOC GUARD — docs/src/data-flow.md (#2241)
# ============================================================================
# Qué protege: `docs/src/data-flow.md` es la DIVULGACIÓN de flujo de datos —
# qué sale de la máquina, hacia qué destinos, con qué payload y qué se retiene
# localmente. El issue #2241 encontró que ese contrato entero vivía sólo en
# código: `data-flow` tenía 0 hits bajo docs/src/. Un doc de divulgación que
# el código desmiente es peor que ningún doc, así que el doc lleva un
# manifiesto embebido que este guard audita CONTRA EL ÁRBOL, no contra su
# propia prosa. Mismo modelo que el guard del doc normativo de providers
# (#2136): documento con literales + fuente de verdad + check fail-closed.
#
# Checks (TODOS fail-closed):
#   C1 estructura  doc y manifiesto presentes, entradas parseables, literales
#                  no vacíos (≥4 chars), rutas resueltas y confinadas a la raíz
#   C2 contains    el doc NOMBRA el literal en alguna línea (payload, destinos,
#                  mecanismos de apagado, retención)
#   C3 code        el archivo citado existe y contiene el símbolo — si el
#                  código se mueve o renombra, la afirmación del doc es drift
#                  y el guard se pone rojo (obliga a actualizar el doc)
#   C4 summary     el doc está listado en docs/src/SUMMARY.md (libro mdBook)
#
# Deliberadamente SIN números de línea: la superficie de edición más frecuente
# de este doc es el propio doc, y un manifiesto con líneas se rompería en
# cada edición cosmética. `contains` pega en cualquier línea del doc; `code`
# pega en cualquier línea del archivo citado.
#
# AISLAMIENTO DE RUTAS: toda ruta que el guard LEE queda confinada
# léxicamente a la raíz del repo (un `..` no puede salir de la raíz; una ruta
# que escapa no se lee siquiera). Mismo contrato que check_ai_providers_doc.sh
# (S5): una cita como `../../etc/passwd` resolvería a un archivo real FUERA
# del árbol y el guard imprimiría su contenido en el log de CI.
#
# El manifiesto vive DENTRO del doc entre `<!-- claims:begin` y
# `claims:end -->` (sin cerrar el comentario en la primera línea) para que el
# bloque entero sea UN comentario HTML de CommonMark y no se renderice en el
# libro mdBook.
#
# Uso: scripts/check_data_flow_doc.sh [--root DIR]
# Salida: 0 limpio · 1 drift (fail-closed)
# ============================================================================
set -uo pipefail

DOC_REL="docs/src/data-flow.md"
SUMMARY_REL="docs/src/SUMMARY.md"
CORE_SRC_REL="crates/webfang_core/src"

usage() {
  cat >&2 <<'EOF'
Uso: scripts/check_data_flow_doc.sh [--root DIR]

  --root DIR   raíz del repo (default: la raíz derivada de BASH_SOURCE)

Salida: 0 limpio · 1 drift (fail-closed)
EOF
}

ROOT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --root)
      if [[ $# -lt 2 ]]; then
        printf '::error::falta el valor de --root\n' >&2
        exit 1
      fi
      ROOT="$2"
      shift 2
      ;;
    --root=*)
      ROOT="${1#--root=}"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf '::error file=%s,line=1::argumento desconocido: %s\n' "$DOC_REL" "$1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$ROOT" ]]; then
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
if [[ ! -d "$ROOT" ]]; then
  printf '::error file=%s,line=1::la raíz %s no existe\n' "$DOC_REL" "$ROOT" >&2
  exit 1
fi
ROOT="$(cd "$ROOT" && pwd)"

FINDINGS=0
report() { # file line message
  printf '::error file=%s,line=%s::%s\n' "$1" "$2" "$3" >&2
  FINDINGS=$((FINDINGS + 1))
}

# ─── aislamiento y resolución de rutas ───────────────────────────────────────
# `confina <rel> <base>` normaliza `rel` contra `base` y devuelve 1 si escapa.
# Aislamiento LEXICAL: no depende de que la ruta exista; una ruta que escapa
# no se mira siquiera.
confina() { # rel base → 0 si queda confinada; imprime nada
  local rel="$1" base="$2" seg
  local -a stack=() segs=()
  IFS='/' read -r -a segs <<<"$base"
  for seg in ${segs[@]+"${segs[@]}"}; do
    case "$seg" in ''|'.') continue ;;
      '..') [[ ${#stack[@]} -gt 0 ]] || return 1; stack=("${stack[@]:0:${#stack[@]}-1}") ;;
      *) stack+=("$seg") ;;
    esac
  done
  IFS='/' read -r -a segs <<<"$rel"
  for seg in ${segs[@]+"${segs[@]}"}; do
    case "$seg" in ''|'.') continue ;;
      '..') [[ ${#stack[@]} -gt 0 ]] || return 1; stack=("${stack[@]:0:${#stack[@]}-1}") ;;
      *) stack+=("$seg") ;;
    esac
  done
  [[ ${#stack[@]} -gt 0 ]] || return 1
  return 0
}

# resolve_path <path>: imprime la ruta relativa al repo; 1 si no resuelve;
# 2 si escapa de la raíz. (a) contra la raíz, (b) contra crates/webfang_core/src/,
# (c) por basename único bajo crates/. Nunca se adivina: ambiguo → drift.
resolve_path() { # path
  local p="${1#./}" rel base count matches
  if ! confina "$p" ""; then
    return 2
  fi
  if [[ -f "$ROOT/$p" ]]; then
    printf '%s\n' "$p"
    return 0
  fi
  if ! confina "$p" "$CORE_SRC_REL"; then
    return 2
  fi
  if [[ -f "$ROOT/$CORE_SRC_REL/$p" ]]; then
    printf '%s/%s\n' "$CORE_SRC_REL" "$p"
    return 0
  fi
  base="${p##*/}"
  matches="$(find "$ROOT/crates" -type f -name "$base" 2>/dev/null | sort)"
  count="$(printf '%s' "$matches" | grep -c . || true)"
  if [[ "$count" == "1" ]]; then
    rel="${matches#"$ROOT"/}"
    if confina "$rel" ""; then
      printf '%s\n' "$rel"
      return 0
    fi
    return 2
  fi
  return 1
}

trim() { # string → sin whitespace en los extremos
  local t="$1"
  t="${t#"${t%%[![:space:]]*}"}"
  t="${t%"${t##*[![:space:]]}"}"
  printf '%s' "$t"
}

# ─── C1: estructura ──────────────────────────────────────────────────────────
DOC="$ROOT/$DOC_REL"
if [[ ! -f "$DOC" ]]; then
  report "$DOC_REL" 1 "no existe el doc de divulgación $DOC_REL bajo la raíz $ROOT"
  printf '::error::data-flow doc guard: 1 hallazgo\n' >&2
  exit 1
fi
mapfile -t DOC_LINES <"$DOC"

begin_idx=-1
end_idx=-1
for i in "${!DOC_LINES[@]}"; do
  t="${DOC_LINES[$i]//[[:space:]]/}"
  if [[ "$t" == "<!--claims:begin" && $begin_idx -lt 0 ]]; then
    begin_idx=$i
  elif [[ "$t" == "claims:end-->" && $end_idx -lt 0 ]]; then
    end_idx=$i
  fi
done
if [[ $begin_idx -lt 0 || $end_idx -lt 0 || $end_idx -le $begin_idx ]]; then
  report "$DOC_REL" 1 "C1: no existe el manifiesto (<!-- claims:begin … claims:end -->)"
  printf '::error::data-flow doc guard: 1 hallazgo\n' >&2
  exit 1
fi

CLAIMS_FENCE_RE='^```claims[[:space:]]*$'
claims_start=-1
for ((i = begin_idx + 1; i < end_idx; i++)); do
  if [[ "${DOC_LINES[$i]}" =~ $CLAIMS_FENCE_RE ]]; then
    claims_start=$i
    break
  fi
done
if [[ $claims_start -lt 0 ]]; then
  report "$DOC_REL" $((begin_idx + 1)) "C1: el manifiesto no contiene un fence con info-string 'claims'"
  printf '::error::data-flow doc guard: 1 hallazgo\n' >&2
  exit 1
fi

# ─── parsing del manifiesto ──────────────────────────────────────────────────
# contains | <literal>            el doc nombra el literal en alguna línea
# code     | <ruta> | <símbolo>   el archivo citado contiene el símbolo
N_CONTAINS=()
N_CODE_FILE=()
N_CODE_SYM=()
N_AT=()
i=$((claims_start + 1))
FENCE_CLOSE_RE='^```[[:space:]]*$'
while [[ $i -lt $end_idx ]]; do
  raw="${DOC_LINES[$i]}"
  if [[ "$raw" =~ $FENCE_CLOSE_RE ]]; then
    break
  fi
  t="$(trim "$raw")"
  i=$((i + 1))
  if [[ -z "$t" || "$t" == \#* ]]; then
    continue
  fi

  IFS='|' read -r -a parts <<<"$t"
  kind="${parts[0]//[[:space:]]/}"
  nfields="${#parts[@]}"

  expected=0
  case "$kind" in
    contains) expected=2 ;;
    code) expected=3 ;;
    *)
      report "$DOC_REL" "$i" "C1: tipo de claim desconocido '$kind' (contains|code)"
      continue
      ;;
  esac
  if [[ $nfields -ne $expected ]]; then
    report "$DOC_REL" "$i" "C1: la entrada '$kind' tiene $nfields campo(s) y se esperaban $expected"
    continue
  fi

  f1="$(trim "${parts[1]}")"
  f2=""
  if [[ $expected -eq 3 ]]; then
    f2="$(trim "${parts[2]}")"
  fi
  if [[ -z "$f1" || ( $expected -eq 3 && -z "$f2" ) ]]; then
    report "$DOC_REL" "$i" "C1: la entrada '$kind' tiene un campo vacío"
    continue
  fi

  if [[ "$kind" == "contains" ]]; then
    # Un literal degenerado (1-3 chars) hace el claim inverificable en la
    # práctica: cualquier prosa lo contiene. Se rechaza como mala entrada.
    if [[ ${#f1} -lt 4 ]]; then
      report "$DOC_REL" "$i" "C1: el literal '$f1' es demasiado corto para ser verificable (mínimo 4 caracteres)"
      continue
    fi
    N_CONTAINS+=("$f1")
    N_AT+=("$i")
    continue
  fi

  res=""
  rc=0
  res="$(resolve_path "$f1")" || rc=$?
  if [[ $rc -ne 0 ]]; then
    motivo="no existe, o el basename es ambiguo bajo crates/"
    [[ $rc -eq 2 ]] && motivo="sale de la raíz del repo — el guard no lee ningún archivo fuera de ella"
    report "$DOC_REL" "$i" "C1: no se pudo resolver la ruta '$f1' ($motivo)"
    continue
  fi
  N_CODE_FILE+=("$res")
  N_CODE_SYM+=("$f2")
  N_AT+=("$i")
done

if [[ ${#N_CONTAINS[@]} -eq 0 && ${#N_CODE_FILE[@]} -eq 0 ]]; then
  report "$DOC_REL" $((claims_start + 1)) "C1: el manifiesto está vacío — el doc no declara ninguna afirmación verificable"
  printf '::error::data-flow doc guard: 1 hallazgo\n' >&2
  exit 1
fi

# ─── C2: contains — el doc nombra el literal ─────────────────────────────────
# El grep excluye la región del manifiesto: si el claim se buscara también
# allí, `contains | X` se satisfaría a sí mismo — un falso verde, la peor
# dirección posible. La prosa es la única superficie que puede respaldar un
# claim.
mapfile -t PROSE_LINES < <(
  for i in "${!DOC_LINES[@]}"; do
    if [[ $i -lt $begin_idx || $i -gt $end_idx ]]; then
      printf '%s\n' "${DOC_LINES[$i]}"
    fi
  done
)
for idx in "${!N_CONTAINS[@]}"; do
  lit="${N_CONTAINS[$idx]}"
  if ! printf '%s\n' "${PROSE_LINES[@]}" | grep -qF -- "$lit"; then
    report "$DOC_REL" "${N_AT[$idx]}" "C2: el doc ya no nombra '$lit' — la afirmación del manifiesto quedó sin respaldo en la prosa"
  fi
done

# ─── C3: code — el archivo citado contiene el símbolo ────────────────────────
for idx in "${!N_CODE_FILE[@]}"; do
  res="${N_CODE_FILE[$idx]}"
  sym="${N_CODE_SYM[$idx]}"
  if ! grep -qF -- "$sym" "$ROOT/$res"; then
    report "$res" 1 "C3: el archivo ya no contiene '$sym' — el doc cita una superficie que se movió o renombró; actualizá la cita"
  fi
done

# ─── C4: el doc está en el libro ─────────────────────────────────────────────
SUMMARY="$ROOT/$SUMMARY_REL"
if [[ ! -f "$SUMMARY" ]]; then
  report "$SUMMARY_REL" 1 "C4: no existe $SUMMARY_REL — el doc de divulgación quedaría fuera del libro mdBook"
elif ! grep -qF -- 'data-flow.md' "$SUMMARY"; then
  report "$SUMMARY_REL" 1 "C4: $SUMMARY_REL no lista data-flow.md — el doc existe pero nadie puede llegar a él"
fi

# ─── resultado ───────────────────────────────────────────────────────────────
if [[ $FINDINGS -gt 0 ]]; then
  printf '::error::data-flow doc guard: %d hallazgo(s) en %s\n' "$FINDINGS" "$DOC_REL" >&2
  exit 1
fi

printf 'OK: %s — %d contains, %d code; el doc está listado en %s.\n' \
  "$DOC_REL" "${#N_CONTAINS[@]}" "${#N_CODE_FILE[@]}" "$SUMMARY_REL"
exit 0
