#!/usr/bin/env bash
# ============================================================================
# HARNESS — semántica del guard de la matriz de features de release (#2430)
# ============================================================================
# Un guard que dejó de reconocer drift es indistinguible de no tener guard
# (mismo argumento que el harness de los pines de versión y el de
# ai-providers). Cada check del guard tiene al menos un caso que prueba su
# fallo CERRADO, sobre fixtures en mktemp -d: sin red, sin cargo, sin tocar el
# repo.
#
# El archivo tiene DOS secciones:
#
#   1. SEMÁNTICA — un caso por check del guard (C1..C6), sobre fixtures.
#   2. WIRING   — la EDGE: que el guard y este harness estén invocados desde
#      ci.yml con la fase `guard-required` declarada, y que el espejo del
#      fast gate exista en las tres lanes que ven las dos caras del drift
#      (doc en lane_docs, manifiesto en lane_fmt_and_guards, scripts en
#      lane_ci).
#
# La sección de wiring vive AQUÍ y no en un test aparte, a propósito: el
# meta-guard (scripts/test_guard_coverage.sh) sólo conoce
# `scripts/test_release_feature_matrix.sh`, así que un archivo de wiring
# separado quedaría sin cobertura propia — exactamente la forma #1707 que
# este repo combate.
#
# Uso: scripts/test_release_feature_matrix.sh [ci-yml] [fast-gate]
#   Los paths opcionales apuntan las aserciones de wiring a revisiones
#   mutadas; sin argumentos se usan las copias del repo.
# Salida: 0 si todos los casos pasan · 1 si alguno falla
# ============================================================================
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="$REPO_ROOT/scripts/check_release_feature_matrix.sh"
CI_YML="${1:-$REPO_ROOT/.github/workflows/ci.yml}"
FAST_GATE="${2:-$REPO_ROOT/scripts/ci_fast_gate.sh}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0
pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# ─── fixture base ────────────────────────────────────────────────────────────
# Una copia mínima (y honesta) del par doc + manifiesto: misma estructura de
# secciones, misma sintaxis de [features], mismo bloque-manifiesto.
new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/crates/webfang_cli" "$dir/docs/src"

  cat > "$dir/crates/webfang_cli/Cargo.toml" <<'TOML'
[package]
name = "webfang_cli"

[features]
# comentario de estructura: el guard debe saltarlo
default = []
adaptive-selectors = ["webfang_core/adaptive-selectors"]
chromium = ["webfang_core/chromium"]
mcp = []
persistence = ["webfang_core/persistence"]

[dependencies]
TOML

  cat > "$dir/docs/src/release-feature-matrix.md" <<'MD'
# Release Feature Matrix

Prosa de relleno.

<!-- feature-matrix:begin -->
```feature-matrix
chromium           | capability | --js-strategy full rejected at preflight
mcp                | marker     | explicit-empty marker
persistence        | capability | SQLite store absent
adaptive-selectors | capability | flags absent
```
<!-- feature-matrix:end -->
MD

  printf '%s\n' "$dir"
}

run_guard() { # fixture-dir → imprime el exit code, silencia la salida
  bash "$GUARD" --root "$1" >/dev/null 2>&1
  echo $?
}

expect_pass() { # nombre fixture-dir
  local rc
  rc="$(run_guard "$2")"
  if [[ "$rc" == "0" ]]; then pass "$1"; else fail "$1 (exit $rc, se esperaba 0)"; fi
}

expect_fail() { # nombre fixture-dir check
  local rc
  rc="$(run_guard "$2")"
  if [[ "$rc" == "1" ]]; then pass "$1"; else fail "$1 (exit $rc, se esperaba 1)"; fi
}

echo "== SEMÁNTICA =="

# 0. El árbol real del repo pasa: el par doc↔Cargo.toml comprometido está en
#    verde. Si esto falla, el árbol llegó roto y el resto del harness no dice
#    nada sobre el guard.
if bash "$GUARD" >/dev/null 2>&1; then
  pass "real tree clean"
else
  fail "real tree clean (el árbol comprometido no pasa el guard)"
fi

# Caso limpio del fixture: la línea base contra la que los negativos diferir.
FIX="$(new_fixture clean)"
expect_pass "clean fixture" "$FIX"

# NEGATION: pre-fix una feature nueva en [features] dejaba la página
# describiendo un binario que ya no existe — el defecto que fundó #2430.
# C2 (cobertura): una feature agregada a [features] sin entrada en el
# manifiesto es el drift canónico de #2430. La mutación va DENTRO de la
# sección: una línea al final del archivo caería bajo [dependencies] y el
# parser no la vería.
FIX="$(new_fixture c2-added)"
sed -i '/^chromium = /a newthing = ["webfang_core/newthing"]' "$FIX/crates/webfang_cli/Cargo.toml"
expect_fail "C2: feature nueva sin entrada en el manifiesto" "$FIX"

# C2 (cobertura): una feature ELIMINADA de Cargo.toml deja una entrada
# huérfana en el doc — misma cobertura, dirección opuesta (lo cubre C3, y el
# caso es el mismo fixture mutado al revés).
FIX="$(new_fixture c3-removed)"
sed -i '/^persistence = /d' "$FIX/crates/webfang_cli/Cargo.toml"
expect_fail "C3: feature eliminada deja entrada huérfana en el doc" "$FIX"

# NEGATION: pre-fix el doc podía declarar features que Cargo.toml nunca tuvo.
# C3 (huérfano): una entrada del doc que nunca existió en [features].
FIX="$(new_fixture c3-orphan)"
sed -i 's/persistence[[:space:]]*|[[:space:]]*capability.*/ghost | capability | invented/' "$FIX/docs/src/release-feature-matrix.md"
expect_fail "C3: key del manifiesto inexistente en Cargo.toml" "$FIX"

# NEGATION: pre-fix un marcador explícito-vacío leía como capacidad del CLI.
# C4 (kind): mcp declarado como capability — el falso positivo que un parser
# ingenuo produce y que #2430 exige distinguir.
FIX="$(new_fixture c4-kind)"
sed -i 's/mcp[[:space:]]*|[[:space:]]*marker.*/mcp | capability | wrong kind/' "$FIX/docs/src/release-feature-matrix.md"
expect_fail "C4: kind divergente (marker declarado capability)" "$FIX"

# NEGATION: pre-fix un doc sin contrato verificable daba verde igual.
# C1 (estructura): el bloque del manifiesto ausente — un doc sin contrato
# verificable no puede dar verde.
FIX="$(new_fixture c1-noblock)"
sed -i '/<!-- feature-matrix:begin -->/,/<!-- feature-matrix:end -->/d' "$FIX/docs/src/release-feature-matrix.md"
expect_fail "C1: bloque del manifiesto ausente" "$FIX"

# C1 (estructura): el doc vacío por completo.
FIX="$(new_fixture c1-nodoc)"
: > "$FIX/docs/src/release-feature-matrix.md"
expect_fail "C1: doc vacío" "$FIX"

# C1 (estructura): manifiesto sin entries.
FIX="$(new_fixture c1-empty)"
sed -i '/^chromium[[:space:]]*|/d; /^mcp[[:space:]]*|/d; /^persistence[[:space:]]*|/d; /^adaptive-selectors |/d' "$FIX/docs/src/release-feature-matrix.md"
expect_fail "C1: manifiesto vacío" "$FIX"

# C1 (estructura): kind desconocido.
FIX="$(new_fixture c1-kind)"
sed -i 's/mcp[[:space:]]*|[[:space:]]*marker.*/mcp | optional | not a kind/' "$FIX/docs/src/release-feature-matrix.md"
expect_fail "C1: kind desconocido" "$FIX"

# NEGATION: pre-fix una segunda declaración contradictoria no se veía.
# C5 (duplicados): la misma key dos veces — la segunda podría contradecir a la
# primera y el guard no puede decidir cuál es la buena. La inserción va dentro
# del fence: fuera del bloque-manifiesto el parser no la leería.
FIX="$(new_fixture c5-dup)"
sed -i '/^chromium[[:space:]]*|/a chromium | capability | duplicated row' "$FIX/docs/src/release-feature-matrix.md"
expect_fail "C5: key repetida en el manifiesto" "$FIX"

# NEGATION: pre-fix una línea no parseable truncaba el conjunto en silencio.
# C6 (parseo): una feature multilínea dentro de [features] — el guard exige
# estilo single-line y falla en lugar de reconstruir en silencio.
FIX="$(new_fixture c6-multiline)"
sed -i 's|^persistence = .*|persistence = [\n  "webfang_core/persistence",\n]|' "$FIX/crates/webfang_cli/Cargo.toml"
expect_fail "C6: feature multilínea no parseable" "$FIX"

# default: NO es una feature de capacidad y NO debe aparecer en el manifiesto;
# si alguien lo agrega, es hallazgo (C3), no un verde.
FIX="$(new_fixture c3-default)"
sed -i '/^persistence[[:space:]]*|/a default | marker | wrongly declared' "$FIX/docs/src/release-feature-matrix.md"
expect_fail "C3: 'default' declarado en el manifiesto" "$FIX"

# Fail-closed: una línea de feature rota (sin cerrar el array) es un hallazgo
# C6 — el guard no trunca el conjunto silenciosamente ni ignora la línea.
FIX="$(new_fixture c6-broken)"
sed -i 's|^chromium = .*|chromium = ["webfang_core/chromium"|' "$FIX/crates/webfang_cli/Cargo.toml"
expect_fail "C6: línea de feature rota dentro de [features]" "$FIX"

echo "== WIRING =="

# El guard está invocado desde ci.yml, EXACTAMENTE una vez, como paso
# requerido (marcador # guard-required, sin continue-on-error): el modelo de
# fase del gate de ai-providers, #2136.
n_gate="$(grep -c 'run: bash scripts/check_release_feature_matrix.sh' "$CI_YML" || true)"
if [[ "$n_gate" == "1" ]]; then
  pass "ci.yml invokes the gate exactly once"
else
  fail "ci.yml invokes the gate exactly once (encontrado: $n_gate)"
fi

if grep -B2 'run: bash scripts/check_release_feature_matrix.sh' "$CI_YML" | grep -q '# guard-required'; then
  pass "ci.yml declares the gate as required (# guard-required)"
else
  fail "ci.yml declares the gate as required (# guard-required)"
fi

if grep -B1 'run: bash scripts/check_release_feature_matrix.sh' "$CI_YML" | grep -q 'continue-on-error'; then
  fail "ci.yml gate step has no continue-on-error"
else
  pass "ci.yml gate step has no continue-on-error"
fi

# El harness está invocado desde ci.yml: sin este paso, las aserciones de
# arriba son decoración que nunca corre (#1707).
n_harness="$(grep -c 'run: bash scripts/test_release_feature_matrix.sh' "$CI_YML" || true)"
if [[ "$n_harness" == "1" ]]; then
  pass "ci.yml invokes this harness"
else
  fail "ci.yml invokes this harness (encontrado: $n_harness)"
fi

# El espejo del fast gate existe: un guard que bloquea en CI y no localmente
# es una sorpresa que el desarrollador descubre recién en el run de CI.
# Tres lanes ven las dos caras del drift: el doc (lane_docs), el manifiesto
# (lane_fmt_and_guards) y los scripts mismos (lane_ci).
n_mirror="$(grep -c 'run_guard "release feature matrix gate (#2430)" scripts/check_release_feature_matrix.sh' "$FAST_GATE" || true)"
if [[ "$n_mirror" == "3" ]]; then
  pass "fast gate mirrors the guard in the 3 drift-facing lanes"
else
  fail "fast gate mirrors the guard in the 3 drift-facing lanes (encontrado: $n_mirror)"
fi

# ─── resultado ───────────────────────────────────────────────────────────────
echo
echo "pasados: $PASSED · fallados: $FAILED"
if [[ $FAILED -gt 0 ]]; then
  exit 1
fi
exit 0
