#!/usr/bin/env bash
# ============================================================================
# RELEASE FEATURE MATRIX DRIFT GUARD (#2430)
# ============================================================================
# Qué protege: `docs/src/release-feature-matrix.md` responde "¿qué puede hacer
# el binario que acabo de descargar?" — capacidad por capacidad, por artefacto
# de release. Esa respuesta se pudre exactamente cuando cambia la tabla
# `[features]` de `crates/webfang_cli/Cargo.toml`: alguien agrega una feature
# de capacidad, la quita, o la convierte en marcador vacío, y la página queda
# describiendo un binario que ya no existe (el defecto exacto del que nació
# #2430).
#
# Este guard es el LECTOR. La fuente de verdad es el manifiesto de la CLI; el
# doc la refleja en un bloque-manifiesto machine-readable al pie de la página
# (`<!-- feature-matrix:begin -->` … `<!-- feature-matrix:end -->`, fence
# ```feature-matrix```). El mismo modelo que el guard de los pines de versión
# y el de ai-providers: documento con literales + fuente de verdad + check
# fail-closed.
#
# Checks (TODOS fail-closed):
#   C1 estructura  bloque y fence presentes, entries parseables, kind válido
#                  (capability | marker), ninguna key vacía
#   C2 cobertura   toda feature de `crates/webfang_cli/Cargo.toml` (salvo
#                  `default`) está en el manifiesto
#   C3 huérfanos   toda key del manifiesto existe en Cargo.toml (y no es
#                  `default`)
#   C4 kind        el kind declarado coincide con la declaración: lista de
#                  valores vacía → marker; lista con valores → capability
#   C5 duplicados  ninguna key repetida en el manifiesto
#   C6 parseo      toda línea con `=` dentro de [features] es parseable en una
#                  sola línea (`key = [ ... ]`) — un array multilínea es un
#                  hallazgo, no un silencio
#
# `mcp = []` es un marcador EXPLÍCITO-VACÍO (#1815 T4): un parser ingenuo lo
# anunciaría como capacidad del CLI. El guard lo exige clasificado como
# `marker`, que es exactamente lo que la página documenta.
#
# Uso: scripts/check_release_feature_matrix.sh [--root DIR]
# Salida: 0 limpio · 1 drift (fail-closed)
# ============================================================================
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Uso: scripts/check_release_feature_matrix.sh [--root DIR]

  --root DIR   raíz del repo (default: la raíz derivada de BASH_SOURCE)

Salida: 0 limpio · 1 drift o entrada malformada (fail-closed)
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
      printf '::error::argumento desconocido: %s\n' "$1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$ROOT" ]]; then
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
if [[ ! -d "$ROOT" ]]; then
  printf '::error::la raíz %s no existe\n' "$ROOT" >&2
  exit 1
fi
ROOT="$(cd "$ROOT" && pwd)"

MANIFEST_REL="crates/webfang_cli/Cargo.toml"
DOC_REL="docs/src/release-feature-matrix.md"

FINDINGS=0
report() { # file line message
  printf '::error file=%s,line=%s::%s\n' "$1" "$2" "$3" >&2
  FINDINGS=$((FINDINGS + 1))
}

trim() {
  local v="${1#"${1%%[![:space:]]*}"}"
  printf '%s' "${v%"${v##*[![:space:]]}"}"
}

# ─── fuente de verdad: [features] de la CLI ─────────────────────────────────
MANIFEST="$ROOT/$MANIFEST_REL"
if [[ ! -f "$MANIFEST" ]]; then
  report "$MANIFEST_REL" 1 "no existe el manifiesto $MANIFEST_REL bajo la raíz $ROOT"
  printf '::error::guard de la matriz de features: %d hallazgo(s); no se puede continuar sin fuente de verdad\n' "$FINDINGS" >&2
  exit 1
fi

# El parseo es por LÍNEA y single-line a propósito: una feature declarada en
# varias líneas (`key = [\n "a",\n]`) es un hallazgo (C6), no una estructura
# que el guard intente reconstruir. Si algún día se necesita, se agrega con su
# caso negativo en el harness — nunca en silencio.
M_KEY=()
M_KIND=()
in_features=0
lineno=0
while IFS= read -r raw; do
  lineno=$((lineno + 1))
  t="$(trim "$raw")"
  case "$t" in
    ''|\#*) continue ;;
  esac
  if [[ "$t" == "["*"]" ]]; then
    if [[ "$t" == "[features]" ]]; then
      in_features=1
    else
      in_features=0
    fi
    continue
  fi
  [[ $in_features -eq 1 ]] || continue
  if [[ ! "$t" =~ ^([A-Za-z0-9_-]+)[[:space:]]*=[[:space:]]*\[(.*)\][[:space:]]*$ ]]; then
    report "$MANIFEST_REL" "$lineno" "C6: línea no parseable dentro de [features] — se espera 'key = [valores]' en una sola línea: '$t'"
    continue
  fi
  key="${BASH_REMATCH[1]}"
  vals="$(trim "${BASH_REMATCH[2]}")"
  [[ "$key" == "default" ]] && continue
  if [[ -z "$vals" ]]; then
    kind="marker"
  else
    kind="capability"
  fi
  M_KEY+=("$key")
  M_KIND+=("$kind")
done <"$MANIFEST"

if [[ ${#M_KEY[@]} -eq 0 ]]; then
  report "$MANIFEST_REL" 1 "C2: no se extrajo ninguna feature de [features] — ¿se movió la sección? El guard no puede dar un verde sin fuente de verdad"
fi

# ─── el doc y su bloque-manifiesto ───────────────────────────────────────────
DOC="$ROOT/$DOC_REL"
if [[ ! -f "$DOC" ]]; then
  report "$DOC_REL" 1 "no existe el doc $DOC_REL bajo la raíz $ROOT"
  printf '::error::guard de la matriz de features: %d hallazgo(s)\n' "$FINDINGS" >&2
  exit 1
fi
mapfile -t DOC_LINES <"$DOC"

begin_idx=-1
end_idx=-1
for i in "${!DOC_LINES[@]}"; do
  t="${DOC_LINES[$i]//[[:space:]]/}"
  if [[ "$t" == "<!--feature-matrix:begin-->" && $begin_idx -lt 0 ]]; then
    begin_idx=$i
  elif [[ "$t" == "<!--feature-matrix:end-->" && $end_idx -lt 0 ]]; then
    end_idx=$i
  fi
done
if [[ $begin_idx -lt 0 || $end_idx -lt 0 || $end_idx -le $begin_idx ]]; then
  report "$DOC_REL" 1 "C1: no existe el bloque del manifiesto (<!-- feature-matrix:begin --> … <!-- feature-matrix:end -->)"
  printf '::error::guard de la matriz de features: %d hallazgo(s)\n' "$FINDINGS" >&2
  exit 1
fi

FENCE_OPEN_RE='^```feature-matrix[[:space:]]*$'
FENCE_CLOSE_RE='^```[[:space:]]*$'
fence_start=-1
for ((i = begin_idx + 1; i < end_idx; i++)); do
  if [[ "${DOC_LINES[$i]}" =~ $FENCE_OPEN_RE ]]; then
    fence_start=$i
    break
  fi
done
if [[ $fence_start -lt 0 ]]; then
  report "$DOC_REL" $((begin_idx + 1)) "C1: el bloque del manifiesto no contiene un fence con info-string 'feature-matrix'"
  printf '::error::guard de la matriz de features: %d hallazgo(s)\n' "$FINDINGS" >&2
  exit 1
fi

D_KEY=()
D_KIND=()
D_AT=()
i=$((fence_start + 1))
while [[ $i -lt $end_idx ]]; do
  raw="${DOC_LINES[$i]}"
  at=$((i + 1))
  i=$((i + 1))
  t="$(trim "$raw")"
  case "$t" in
    ''|\#*) continue ;;
  esac
  if [[ "$t" =~ $FENCE_CLOSE_RE ]]; then
    break
  fi
  IFS='|' read -r -a parts <<<"$t"
  if [[ ${#parts[@]} -lt 2 ]]; then
    report "$DOC_REL" "$at" "C1: la entrada del manifiesto necesita al menos 'key | kind': '$t'"
    continue
  fi
  key="$(trim "${parts[0]}")"
  kind="$(trim "${parts[1]}")"
  if [[ -z "$key" ]]; then
    report "$DOC_REL" "$at" "C1: la entrada tiene una key vacía: '$t'"
    continue
  fi
  case "$kind" in
    capability|marker) ;;
    *)
      report "$DOC_REL" "$at" "C1: kind desconocido '$kind' (capability|marker) en la entrada '$key'"
      continue
      ;;
  esac
  D_KEY+=("$key")
  D_KIND+=("$kind")
  D_AT+=("$at")
done

if [[ ${#D_KEY[@]} -eq 0 ]]; then
  report "$DOC_REL" $((fence_start + 1)) "C1: el manifiesto del doc está vacío — no declara ninguna feature"
fi

# ─── C5: duplicados en el manifiesto del doc ─────────────────────────────────
for a in "${!D_KEY[@]}"; do
  for ((b = a + 1; b < ${#D_KEY[@]}; b++)); do
    if [[ "${D_KEY[$a]}" == "${D_KEY[$b]}" ]]; then
      report "$DOC_REL" "${D_AT[$b]}" "C5: la key '${D_KEY[$a]}' está repetida en el manifiesto (primera declaración en la línea ${D_AT[$a]})"
    fi
  done
done

# ─── C3: huérfanos (doc → Cargo.toml) ────────────────────────────────────────
for e in "${!D_KEY[@]}"; do
  key="${D_KEY[$e]}"
  found=-1
  for m in "${!M_KEY[@]}"; do
    if [[ "${M_KEY[$m]}" == "$key" ]]; then
      found=$m
      break
    fi
  done
  if [[ $found -lt 0 ]]; then
    if [[ "$key" == "default" ]]; then
      report "$DOC_REL" "${D_AT[$e]}" "C3: 'default' no se declara en el manifiesto — no es una feature de capacidad, el guard la excluye de la comparación"
    else
      report "$DOC_REL" "${D_AT[$e]}" "C3: la key '$key' del manifiesto no existe en [features] de $MANIFEST_REL — ¿se renombró o eliminó la feature?"
    fi
    continue
  fi
  # ─── C4: el kind coincide con la declaración ──────────────────────────────
  if [[ "${D_KIND[$e]}" != "${M_KIND[$found]}" ]]; then
    report "$DOC_REL" "${D_AT[$e]}" "C4: '$key' está declarada como '${D_KIND[$e]}' en el doc, pero en $MANIFEST_REL es '${M_KIND[$found]}' (lista vacía → marker; lista con valores → capability)"
  fi
done

# ─── C2: cobertura (Cargo.toml → doc) ────────────────────────────────────────
for m in "${!M_KEY[@]}"; do
  key="${M_KEY[$m]}"
  found=0
  for d in "${D_KEY[@]}"; do
    [[ "$d" == "$key" ]] && found=1 && break
  done
  if [[ $found -eq 0 ]]; then
    report "$MANIFEST_REL" 1 "C2: la feature '$key' de [features] no está declarada en el manifiesto de $DOC_REL"
  fi
done

# ─── resultado ───────────────────────────────────────────────────────────────
if [[ $FINDINGS -gt 0 ]]; then
  printf '::error::guard de la matriz de features: %d hallazgo(s) — actualizá %s junto con [features] de %s\n' \
    "$FINDINGS" "$DOC_REL" "$MANIFEST_REL" >&2
  exit 1
fi

printf 'OK: %s — %d features en [features], %d entradas en el manifiesto; cobertura y kinds verificados.\n' \
  "$DOC_REL" "${#M_KEY[@]}" "${#D_KEY[@]}"
exit 0
