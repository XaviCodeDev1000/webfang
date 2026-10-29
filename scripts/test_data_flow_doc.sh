#!/usr/bin/env bash
# ============================================================================
# HARNESS — semántica del data-flow doc guard (#2241)
# ============================================================================
# Un guard que dejó de reconocer drift es indistinguible de no tener guard.
# Cada check del guard tiene al menos un caso que prueba su fallo CERRADO,
# sobre fixtures en mktemp -d: sin red, sin cargo, sin tocar el repo.
#
# El archivo tiene DOS secciones (el meta-guard scripts/test_guard_coverage.sh
# sólo conoce este archivo, así que un harness de wiring aparte quedaría sin
# cobertura propia — la forma #1707):
#
#   1. SEMÁNTICA — un caso por check del guard, sobre fixtures en mktemp -d.
#   2. WIRING   — la EDGE: que el guard y este harness estén invocados desde
#      el job `repo-guards` de ci.yml con la fase declarada exactamente una
#      vez, y que el espejo local (ci_fast_gate.sh) lo invoque en el MISMO
#      modo bloqueante en las dos lanes que ven las superficies del drift.
#
# NEGATION: pre-fix nada en la documentación de usuario decía qué sale de la
# máquina, cuál es el payload o qué puede contener una traza — el contrato
# entero vivía sólo en código (#2241, filas DF-D1/DF-D4/DF-D6 del umbrella
# #1609). Y un doc de divulgación que el código desmiente no tiene ningún
# gate que lo note: este harness es la prueba de que el guard puede fallar.
#
# Uso: scripts/test_data_flow_doc.sh [ci-yml] [fast-gate]
#   Los paths opcionales apuntan las aserciones de wiring a revisiones
#   mutadas; sin argumentos se usan las copias del repo.
# Salida: 0 si todos los casos pasan · 1 si alguno falla
# ============================================================================
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="$REPO_ROOT/scripts/check_data_flow_doc.sh"
CI_YML="${1:-$REPO_ROOT/.github/workflows/ci.yml}"
FAST_GATE="${2:-$REPO_ROOT/scripts/ci_fast_gate.sh}"
GUARD_REL="scripts/check_data_flow_doc.sh"
HARNESS_REL="scripts/test_data_flow_doc.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0
pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# ─── fixture base ────────────────────────────────────────────────────────────
new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/docs/src" "$dir/crates/webfang_core/src/domain"

  cat > "$dir/crates/webfang_core/src/domain/sample.rs" <<'RS'
//! Módulo de ejemplo con la superficie citada.
pub fn extract_text_content(html: &str) -> String {
    String::new()
}
RS

  cat > "$dir/docs/src/SUMMARY.md" <<'MD'
# Summary

- [Data Flow & Privacy](data-flow.md)
MD

  cat > "$dir/docs/src/data-flow.md" <<'MD'
# Data Flow & Privacy

The payload is extracted text, never raw HTML.

Egress goes to `chat/completions` and `embeddings` endpoints.

<!-- claims:begin
```claims
contains | never raw HTML
contains | chat/completions
code | crates/webfang_core/src/domain/sample.rs | extract_text_content
```
claims:end -->
MD

  printf '%s\n' "$dir"
}

# run_guard <dir> [args...]: el guard contra el fixture.
run_guard() {
  ( cd "$1" && bash "$GUARD" --root "$1" "${@:2}" ) >"$WORK/out.log" 2>&1
}

# expect_ok <label> <dir>
expect_ok() {
  if run_guard "$2"; then
    pass "$1"
  else
    fail "$1 — $(head -c 300 "$WORK/out.log")"
  fi
}

# expect_red <label> <dir> [motivo-esperado]
expect_red() {
  local label="$1" dir="$2" reason="${3:-}"
  if run_guard "$dir"; then
    fail "$label — el guard pasó verde sobre el drift: no ve su input"
  elif [[ -n "$reason" ]] && ! grep -qF "$reason" "$WORK/out.log"; then
    fail "$label — el guard se puso rojo por otro motivo, no por '$reason': $(head -c 300 "$WORK/out.log")"
  else
    pass "$label"
  fi
}

# ─── SEMÁNTICA ───────────────────────────────────────────────────────────────

S1="$(new_fixture s1-green)"
expect_ok "S1: fixture sano pasa" "$S1"

S2="$(new_fixture s2-nodoc)"
rm "$S2/docs/src/data-flow.md"
expect_red "S2: doc ausente se rechaza" "$S2" "no existe el doc"

S3="$(new_fixture s3-nomanifest)"
sed -i '/claims:begin/,$d' "$S3/docs/src/data-flow.md"
if grep -qF "claims:begin" "$S3/docs/src/data-flow.md"; then
  fail "S3: la mutación no aplicó — el caso no probó nada"
fi
expect_red "S3: manifiesto ausente se rechaza" "$S3" "no existe el manifiesto"

S4="$(new_fixture s4-empty)"
sed -i 's/^contains .*$//; s/^code .*$//' "$S4/docs/src/data-flow.md"
expect_red "S4: manifiesto vacío se rechaza" "$S4" "manifiesto está vacío"

S5="$(new_fixture s5-unknown)"
sed -i 's/^contains | never raw HTML$/ghost | never raw HTML/' "$S5/docs/src/data-flow.md"
expect_red "S5: tipo de claim desconocido se rechaza" "$S5" "tipo de claim desconocido"

S6="$(new_fixture s6-payload)"
# Sólo la prosa cambia, nunca la entrada del manifiesto: el drift es que el
# doc dejó de RESPALDAR un claim que sigue declarado.
sed -i 's/The payload is extracted text, never raw HTML\./The payload is extracted text, never raw snippet./' "$S6/docs/src/data-flow.md"
expect_red "S6: la prosa deja de nombrar el payload se rechaza" "$S6" \
  "ya no nombra 'never raw HTML'"

S7="$(new_fixture s7-symbol)"
sed -i 's/extract_text_content/extract_snippet_content/' "$S7/crates/webfang_core/src/domain/sample.rs"
expect_red "S7: el símbolo citado se renombra se rechaza" "$S7" \
  "se movió o renombró"

S8="$(new_fixture s8-file)"
rm "$S8/crates/webfang_core/src/domain/sample.rs"
expect_red "S8: el archivo citado desaparece se rechaza" "$S8" "basename es ambiguo"

S9="$(new_fixture s9-escape)"
python3 - "$S9/docs/src/data-flow.md" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read().replace(
    "code | crates/webfang_core/src/domain/sample.rs | extract_text_content",
    "code | ../../etc/passwd | root")
open(p, "w").write(s)
PY
expect_red "S9: la ruta que escapa de la raíz se rechaza" "$S9" \
  "sale de la raíz del repo"

S10="$(new_fixture s10-summary)"
sed -i '/data-flow.md/d' "$S10/docs/src/SUMMARY.md"
expect_red "S10: el doc fuera del SUMMARY se rechaza" "$S10" \
  "no lista data-flow.md"

S11="$(new_fixture s11-fields)"
sed -i 's/^contains | never raw HTML$/contains | never raw HTML | extra/' "$S11/docs/src/data-flow.md"
expect_red "S11: la entrada con campos de más se rechaza" "$S11" \
  "campo(s) y se esperaban"

S12="$(new_fixture s12-short)"
sed -i 's/^contains | never raw HTML$/contains | ab/' "$S12/docs/src/data-flow.md"
expect_red "S12: el literal degenerado se rechaza" "$S12" \
  "demasiado corto"

# ─── WIRING ──────────────────────────────────────────────────────────────────
# NEGATION: pre-fix el guard podía existir, decidir drift y no estar conectado
# a ningún job: verde en el árbol, invisible en CI (#1707). Y el espejo local
# podía perder la referencia sin que nadie lo notara.

job_block() { # <workflow> <job>
  awk -v job="$2" '
    $0 == "  " job ":" { in_job = 1; print; next }
    in_job && /^  [a-z0-9_-]+:/ { exit }
    in_job { print }
  ' "$1"
}

step_block() { # <file>: el paso YAML que ejecuta el guard, si lo hay
  awk '
    /^      - / {
      if (hit) { printf "%s", buf; found = 1; exit }
      buf = $0 "\n"; hit = 0; next
    }
    { buf = buf $0 "\n" }
    /run:[[:space:]].*scripts\/check_data_flow_doc\.sh/ { hit = 1 }
    END { if (!found && hit) printf "%s", buf }
  ' "$1"
}

lane_block() { # <fast-gate> <fn>: el cuerpo de una función bash
  awk -v fn="$2" '
    $0 == fn "() {" { in_fn = 1; print; next }
    in_fn && /^}/ { print; exit }
    in_fn { print }
  ' "$1"
}

check_wiring() { # <ci-yml> <fast-gate>
  local wf="$1" fg="$2" bad=0 step
  local job="$WORK/wire-repo-guards.yml"

  job_block "$wf" repo-guards > "$job"
  if [[ ! -s "$job" ]]; then
    echo "job 'repo-guards' not found: the guard would sit in a renamed job, unnoticed"
    return 1
  fi

  # (a) el guard corre en ese job, con la fase declarada exactamente una vez
  step="$(step_block "$job")"
  if [[ -z "$step" ]]; then
    echo "no step in 'repo-guards' runs $GUARD_REL: the guard is in the tree and connected to nothing"
    bad=1
  else
    local adv=0 req=0
    grep -qE '^[[:space:]]*continue-on-error:[[:space:]]*true([[:space:]]|$)' <<<"$step" && adv=1
    grep -qE '^[[:space:]]*#[[:space:]]*guard-required([[:space:]]|$)' <<<"$step" && req=1
    if [[ $adv -eq 1 && $req -eq 1 ]]; then
      echo "the guard step declares both phases ('continue-on-error: true' AND '# guard-required'): a false promotion"
      bad=1
    elif [[ $adv -eq 0 && $req -eq 0 ]]; then
      echo "the guard step declares no phase: it needs 'continue-on-error: true' (advisory) or the '# guard-required' marker (required) — exactly one"
      bad=1
    fi
  fi

  # (b) el harness corre en el mismo job
  if ! grep -qE "run:[[:space:]].*${HARNESS_REL//./\\.}" "$job"; then
    echo "no step in 'repo-guards' runs $HARNESS_REL: the harness that proves the guard can fail is connected to nothing"
    bad=1
  fi

  # (c) el espejo local invoca el guard, BLOQUEANTE (sin --advisory), en las
  # dos lanes que ven las superficies del drift: el doc (lane_docs) y el
  # código citado (lane_fmt_and_guards). Una referencia sólo en comentario
  # no es una conexión.
  for lane in lane_docs lane_fmt_and_guards; do
    lane_block "$fg" "$lane" > "$WORK/wire-$lane.sh"
    if [[ ! -s "$WORK/wire-$lane.sh" ]]; then
      echo "function '$lane' not found in the fast gate: the local mirror cannot be checked"
      bad=1
      continue
    fi
    local inv
    inv="$(grep -vE '^[[:space:]]*#' "$WORK/wire-$lane.sh" | grep -F "$GUARD_REL" || true)"
    if [[ -z "$inv" ]]; then
      echo "lane '$lane' does not run $GUARD_REL: one drift surface would pass locally with the drift unnamed"
      bad=1
    elif grep -qF -- '--advisory' <<<"$inv" || grep -qF 'run_advisory_guard' <<<"$inv"; then
      echo "lane '$lane' invokes $GUARD_REL in advisory mode: the CI step is required, so a blocking-in-CI-only guard is a surprise the developer discovers in the CI run"
      bad=1
    fi
  done

  [[ $bad -eq 0 ]]
}

expect_wiring() { # <expected-ok?> <label> <ci-yml> <fast-gate> [motivo]
  local want="$1" label="$2" wf="$3" fg="$4" reason="${5:-}"
  if check_wiring "$wf" "$fg" > "$WORK/wiring.out" 2>&1; then
    if [[ "$want" == "ok" ]]; then
      pass "$label"
    else
      fail "$label — la aserción pasó sobre una revisión mutada: no está viendo su input"
    fi
  else
    if [[ "$want" == "ok" ]]; then
      fail "$label — $(head -c 400 "$WORK/wiring.out")"
    elif [[ -n "$reason" ]] && ! grep -qF "$reason" "$WORK/wiring.out"; then
      fail "$label — el wiring se rechazó por otro motivo, no por el esperado ('$reason'): $(head -c 400 "$WORK/wiring.out")"
    else
      pass "$label"
    fi
  fi
}

expect_wiring ok "W1: el wiring comiteado está completo" "$CI_YML" "$FAST_GATE"

# W2: control negativo — ci.yml sin el paso del guard.
awk '
  /^      - / { if (!pending) { printf "%s", pre; pre = "" } else if (!hit) { printf "%s", buf }
    pending = 1; buf = $0 "\n"; hit = 0; next }
  pending { buf = buf $0 "\n" }
  !pending { pre = pre $0 "\n" }
  /run:[[:space:]].*scripts\/check_data_flow_doc\.sh/ { hit = 1 }
  END { if (pending && !hit) printf "%s", buf; if (!pending) printf "%s", pre }
' "$CI_YML" > "$WORK/wire-ci-no-step.yml"
if cmp -s "$CI_YML" "$WORK/wire-ci-no-step.yml"; then
  fail "W2: la mutación de ci.yml no aplicó — el caso no probó nada"
else
  expect_wiring red "W2: el paso del guard borrado de ci.yml se rechaza" \
    "$WORK/wire-ci-no-step.yml" "$FAST_GATE" "connected to nothing"
fi

# W3: control negativo — el fast gate pierde la referencia al guard.
sed "\|$GUARD_REL|d" "$FAST_GATE" > "$WORK/wire-fg-no-ref.sh"
if cmp -s "$FAST_GATE" "$WORK/wire-fg-no-ref.sh"; then
  fail "W3: la mutación del fast gate no aplicó — el caso no probó nada"
else
  expect_wiring red "W3: la referencia perdida del fast gate se rechaza" \
    "$CI_YML" "$WORK/wire-fg-no-ref.sh" "does not run"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]
