#!/usr/bin/env bash
# ============================================================================
# DOCS CLAIMS — does the README still tell the truth?
# ============================================================================
# The repository gates its CODE with 69 steps in `repo-guards` and gated its
# DOCUMENTATION with none. The cost was measured on 2026-10-24:
#
#   - README drew "`webfang_cli ──→ webfang_mcp ──→ webfang_core`", an edge that
#     does not exist in any manifest and that AGENTS.md explicitly forbids;
#   - a "`tests-1,337`" badge survived three months while `webfang_core --lib`
#     alone ran 2,907 tests;
#   - a "`Miri-domain+core-passing`" badge survived four consecutive red
#     scheduled runs (2026-09-28 .. 2026-10-19).
#
# Every one of those is a claim a machine can check. This gate checks the
# three classes that are cheap and unambiguous:
#
#   1. DEPENDENCY DIAGRAM. Every edge drawn in the Architecture block must
#      exist in a [dependencies] table, and every production edge among
#      {core, mcp, cli} must be drawn. A diagram is a claim about the graph.
#   2. STATIC METRIC BADGES. A hand-written count ("tests-1,337",
#      "coverage-87") drifts the moment the number changes and nothing reads
#      it. Live badges (workflow status) and stable facts (license, MSRV) are
#      allowed; MSRV is cross-checked against rust-version.
#   3. RETIRED CLAIMS. Specific sentences that were proven false are banned
#      outright, so a revert or a copy-paste cannot resurrect them.
#
# What this gate does NOT do: it does not read prose. "Fast", "robust" and
# "production-ready" are opinions; the three classes above are facts.
#
# Usage: scripts/check_docs_claims.sh
# Exit: 0 clean · 1 a claim contradicts the tree (or an input is unreadable)
# ============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

err() { printf '::error::%s\n' "$*" >&2; exit 1; }

README="README.md"
MANIFEST="Cargo.toml"

[[ -f $README ]] || err "no se encontró $README en $ROOT: no se puede verificar ninguna afirmación."
[[ -f $MANIFEST ]] || err "no se encontró $MANIFEST en $ROOT."

FAILED=0
fail() { printf '::error::%s\n' "$*" >&2; FAILED=$((FAILED + 1)); }

# ----------------------------------------------------------------------------
# 1. Dependency diagram
# ----------------------------------------------------------------------------
# The Architecture section ends at the next top-level heading.
ARCH="$(sed -n '/^## Architecture$/,/^## [A-Z]/p' "$README")"
[[ -n $ARCH ]] || err "no se encontró la sección '## Architecture' en $README."

# Edges drawn: "crate ──→ crate". Tolerate any run of ─ before the → so a
# reflow does not blind the gate.
README_EDGES="$(printf '%s\n' "$ARCH" \
  | sed -n 's/^[[:space:]]*\([a-z0-9_]*\)[[:space:]]*─*→[[:space:]]*\([a-z0-9_]*\).*/\1->\2/p' \
  | sort -u)"

# Real edges: crate names listed under [dependencies] in each crate manifest.
# dev-dependencies are a different tier and must NOT be drawn as a production
# edge (webfang_core -> webfang_test_utils is dev-only by policy).
real_edges() {
  local manifest crate dep section line
  for manifest in crates/*/Cargo.toml; do
    [[ -f $manifest ]] || continue
    crate="$(basename "$(dirname "$manifest")")"
    section=""
    while IFS= read -r line; do
      case "$line" in
        '['*']') section="$line" ;;
        webfang_*)
          if [[ $section == '[dependencies]' ]]; then
            dep="${line%%[ =]*}"
            [[ $dep == "$crate" ]] && continue
            printf '%s->%s\n' "$crate" "$dep"
          fi
          ;;
      esac
    done < "$manifest"
  done | sort -u
}

REAL_EDGES="$(real_edges)"
[[ -n $REAL_EDGES ]] || err "no se leyó ninguna arista webfang_* de crates/*/Cargo.toml."

# Every drawn edge must be real.
while IFS= read -r edge; do
  [[ -z $edge ]] && continue
  if ! printf '%s\n' "$REAL_EDGES" | grep -qxF "$edge"; then
    fail "README dibuja '$edge' en el diagrama de arquitectura, pero esa arista no existe en ningún [dependencies]."
  fi
done <<< "$README_EDGES"

# Every production edge among {core,mcp,cli} must be drawn.
PROD_CRATES='webfang_core|webfang_mcp|webfang_cli'
while IFS= read -r edge; do
  [[ -z $edge ]] && continue
  src="${edge%%->*}"
  [[ $src =~ ^($PROD_CRATES)$ ]] || continue
  if ! printf '%s\n' "$README_EDGES" | grep -qxF "$edge"; then
    fail "falta en el diagrama de arquitectura la arista de producción '$edge' (existe en el manifiesto)."
  fi
done <<< "$REAL_EDGES"

# ----------------------------------------------------------------------------
# 2. Static metric badges
# ----------------------------------------------------------------------------
BANNED_METRICS='tests|test|coverage|cov|loc|lines|files|downloads|stars'
while IFS= read -r badge; do
  [[ -z $badge ]] && continue
  payload="${badge#*badge/}"
  payload="${payload%%)*}"
  payload="${payload%%\?*}"
  metric="${payload%%-*}"
  value="${payload#*-}"
  case "$metric" in
    rust)
      declared="$(sed -n 's/^rust-version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$MANIFEST" | head -n1)"
      [[ -n $declared ]] || err "no se pudo leer rust-version de $MANIFEST."
      badge_msrv="$(printf '%s' "$value" | sed -n 's/^\([0-9][0-9.]*\).*/\1/p')"
      [[ -n $badge_msrv ]] || fail "el badge de Rust no declara una versión legible: '$badge'."
      [[ $badge_msrv == "$declared" ]] || fail "el badge de Rust dice $badge_msrv pero rust-version es $declared."
      ;;
    *)
      if printf '%s' "$metric" | grep -qE "^($BANNED_METRICS)$"; then
        fail "badge estático de métrica '$badge': un conteo escrito a mano se desactualiza sin que nada lo lea. Usá un badge live o el comando en el texto."
      fi
      ;;
  esac
done < <(grep -oE 'img\.shields\.io/badge/[^)]*' "$README" || true)

# ----------------------------------------------------------------------------
# 3. Retired claims
# ----------------------------------------------------------------------------
RETIRED_CLAIMS=(
  '1,337'
  'tests-1%2C337'
  'Miri-domain%2Bcore-passing'
  'CLI → MCP → Core'
  'Domain + Core layers verified'
)
for claim in "${RETIRED_CLAIMS[@]}"; do
  if grep -qF -- "$claim" "$README"; then
    fail "afirmación retirada y probada falsa: '$claim'. Ver webfang-audit/AUDIT-QAQC-20261024/01-matriz-verificacion.md."
  fi
done
if grep -rqF -- '1,337' docs/src 2>/dev/null; then
  fail "el conteo retirado '1,337' reaparece en docs/src."
fi

if (( FAILED > 0 )); then
  printf '::error::check_docs_claims: %d afirmación(es) contradicen el árbol.\n' "$FAILED" >&2
  exit 1
fi

printf 'OK: check_docs_claims: diagrama, badges y afirmaciones retiradas coinciden con el árbol.\n'
printf '  aristas dibujadas: %s\n' "$(printf '%s\n' "$README_EDGES" | tr '\n' ' ')"
printf '  aristas reales:    %s\n' "$(printf '%s\n' "$REAL_EDGES" | tr '\n' ' ')"
