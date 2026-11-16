#!/usr/bin/env bash
# test_agents_doc_wiring.sh
#
# Wiring harness for scripts/check_agents_doc.sh, the AGENTS.md size/structure
# gate (#2839). Companion to test_agents_doc.sh, which pins the gate's
# DECISIONS; this file pins that the gate is connected to anything at all.
#
# Why this file exists: this repository already shipped a docs guard that was
# semantically correct and completely inert — check_docs_version_pins.sh landed
# in #2012 and no workflow invoked it, so nine stale install recipes stayed
# green for a whole release. A deleted CI step is valid YAML, and a green run
# with no guard looks exactly like a green run with one. The AGENTS.md regrowth
# (740 -> 862 lines) is the same class one level up: a budget nobody evaluates.
#
# What it proves, offline and non-vacuously:
#
#   - ci.yml runs the gate, its semantics harness and this harness in
#     `repo-guards`, a job with no `paths:` filter and no `if:` condition, so
#     an oversize AGENTS.md reds a required check on EVERY PR — including a
#     markdown-only one, which is exactly where the growth starts;
#   - the gate step carries the `# guard-required` marker: blocking from day
#     one, never an advisory phase that absorbs its own failure;
#   - `gate` names `repo-guards` in its `needs:`, so the failure reaches the
#     required aggregator instead of dying in a leaf job;
#   - scripts/ci_fast_gate.sh mirrors the gate in BOTH lanes that can carry
#     the drift (lane_docs for a markdown edit, lane_fmt_and_guards for a
#     baseline or code change) and runs both harnesses in the CI-ONLY lane,
#     which is the only lane a scripts/** change ever lands in.
#
# Usage: bash scripts/test_agents_doc_wiring.sh [ci-yml] [fast-gate]
# The optional paths exist so the negative controls can point the same checks
# at a mutated copy; they default to the repository's files.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CI_YML="${1:-$ROOT/.github/workflows/ci.yml}"
FAST_GATE="${2:-$ROOT/scripts/ci_fast_gate.sh}"

GATE="scripts/check_agents_doc.sh"
SEMANTICS="scripts/test_agents_doc.sh"
WIRING="scripts/test_agents_doc_wiring.sh"

FAILED=0
fail() { printf '  FAIL %s\n' "$1"; FAILED=$((FAILED + 1)); }
pass() { printf '  ok   %s\n' "$1"; }

[[ -f $CI_YML ]] || { printf '  FAIL no existe %s\n' "$CI_YML"; exit 1; }
[[ -f $FAST_GATE ]] || { printf '  FAIL no existe %s\n' "$FAST_GATE"; exit 1; }

# job_block <job-name>: the YAML block of a top-level job (2-space indent),
# up to the next top-level job or the end of file.
job_block() {
  local want="$1" inside=0 line
  while IFS= read -r line; do
    if [[ $line == "  $want:" ]]; then
      inside=1
      printf '%s\n' "$line"
      continue
    fi
    if (( inside )) && [[ $line =~ ^"  "[a-zA-Z0-9_-]+:$ ]]; then
      break
    fi
    if (( inside )); then
      printf '%s\n' "$line"
    fi
  done < "$CI_YML"
}

# ---------------------------------------------------------------------------
# 1. repo-guards runs all three scripts
# ---------------------------------------------------------------------------
GUARDS="$(job_block repo-guards)"
[[ -n $GUARDS ]] || fail "no se encontró el job 'repo-guards' en $CI_YML"

for script in "$GATE" "$SEMANTICS" "$WIRING"; do
  # Herestring, not a pipe: with pipefail, grep -q exiting on the first match
  # SIGPIPEs the producer and turns a MATCH into a pipeline failure.
  if grep -qF "bash $script" <<< "$GUARDS"; then
    pass "repo-guards ejecuta $script"
  else
    fail "repo-guards NO ejecuta $script (gate inerte: verde sin leer nada)"
  fi
done

# ---------------------------------------------------------------------------
# 2. the gate step is REQUIRED: the marker sits on the line above its run:
# ---------------------------------------------------------------------------
if grep -B1 -F "run: bash $GATE" <<< "$GUARDS" | grep -qF '# guard-required'; then
  pass "el paso del gate lleva el marcador # guard-required"
else
  fail "el paso del gate NO lleva '# guard-required': sin fase declarada el gate puede volverse advisory en silencio"
fi

# ---------------------------------------------------------------------------
# 3. repo-guards is unconditional and unfiltered
# ---------------------------------------------------------------------------
if grep -qE '^    (if|paths|paths-ignore):' <<< "$GUARDS"; then
  fail "repo-guards tiene 'if:' o 'paths:' a nivel de job: un PR de solo markdown puede saltarlo"
else
  pass "repo-guards no tiene 'if:' ni 'paths:'"
fi

# ---------------------------------------------------------------------------
# 4. gate waits on repo-guards
# ---------------------------------------------------------------------------
GATE_JOB="$(job_block gate)"
[[ -n $GATE_JOB ]] || fail "no se encontró el job 'gate' en $CI_YML"
if grep -qE 'repo-guards' <<< "$GATE_JOB"; then
  pass "gate espera a repo-guards"
else
  fail "gate NO espera a repo-guards: el rojo nunca llega al check requerido"
fi

# ---------------------------------------------------------------------------
# 5. local fast gate mirrors the gate in BOTH lanes and runs both harnesses
# ---------------------------------------------------------------------------
mirrors="$(grep -cF "bash $GATE" "$FAST_GATE" || true)"
if [[ "$mirrors" -ge 2 ]]; then
  pass "ci_fast_gate.sh ejecuta el gate en $mirrors sitios (lane_docs + lane_fmt_and_guards)"
else
  fail "ci_fast_gate.sh ejecuta el gate en $mirrors sitio(s); se esperan 2 (markdown y baseline/código)"
fi

for script in "$SEMANTICS" "$WIRING"; do
  if grep -qF "bash $script" "$FAST_GATE"; then
    pass "ci_fast_gate.sh ejecuta $script"
  else
    fail "ci_fast_gate.sh NO ejecuta $script"
  fi
done

printf '\n'
if (( FAILED > 0 )); then
  printf 'wiring: %d fallo(s)\n' "$FAILED"
  exit 1
fi
printf 'OK: wiring de agents-doc completo.\n'
