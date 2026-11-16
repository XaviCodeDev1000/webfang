#!/usr/bin/env bash
# Semantics harness for the AGENTS.md size/structure guard
# (scripts/check_agents_doc.sh, #2839).
#
# A gate that only ever runs on a healthy AGENTS.md is indistinguishable from
# no gate: the 740 -> 862 regrowth happened precisely because nothing evaluated
# the size. These cases pin the decisions that cannot be proven by reading the
# script — one negative case per check, plus the fail-closed inputs, the budget
# pin and the descending-only ratchet. Everything runs against mktemp fixtures;
# no network, no mutation of the repository this harness runs from.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# new_fixture <name>: minimal tree the guard can read — a copy of the guard,
# a fixture baseline at the pinned budget, and a small healthy AGENTS.md.
new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/scripts"
  cp "$REPO_ROOT/scripts/check_agents_doc.sh" "$dir/scripts/"
  chmod +x "$dir/scripts/check_agents_doc.sh"
  cat > "$dir/scripts/quality-baselines.json" <<'EOF'
{
  "agents_doc": {
    "max_lines": 650,
    "max_bytes": 40960
  }
}
EOF
  cat > "$dir/AGENTS.md" <<'EOF'
# fixture router

```bash
echo hola
```

Citá el símbolo (`WreqDownloader::fetch_inner`), no la línea.
EOF
  printf '%s\n' "$dir"
}

# expect <fixture> <0|1> <name>
expect() {
  local dir="$1" want="$2" name="$3" got=0
  bash "$dir/scripts/check_agents_doc.sh" >"$dir/out.log" 2>&1 || got=$?
  if [[ $got -eq $want ]]; then
    pass "$name (exit $got)"
  else
    fail "$name (esperaba exit $want, obtuve $got)"
    sed -n '1,5p' "$dir/out.log" | sed 's/^/       /'
  fi
}

printf 'check_agents_doc semantics\n'

# 1. healthy fixture passes
d="$(new_fixture healthy)"
expect "$d" 0 "árbol sano"

# NEGATION: pre-fix the router regrew 740 -> 862 lines with nothing reading
# the size — one line over the budget must red the gate.
# 2. oversize lines
d="$(new_fixture oversize-lines)"
{ cat "$d/AGENTS.md"; for _ in $(seq 1 650); do echo 'línea de relleno'; done; } > "$d/AGENTS.md.big"
mv "$d/AGENTS.md.big" "$d/AGENTS.md"
expect "$d" 1 "líneas por encima del presupuesto"

# NEGATION: a byte budget without a line budget is blind to one huge line
# (a pasted runbook blob); both ceilings are enforced independently.
# 3. oversize bytes
d="$(new_fixture oversize-bytes)"
printf '%s\n' "$(printf 'a%.0s' $(seq 1 41000))" > "$d/AGENTS.md"
expect "$d" 1 "bytes por encima del presupuesto"

# NEGATION: an unclosed code block swallows the rest of the router when it
# renders — the fence count must be even.
# 4. odd fences
d="$(new_fixture odd-fences)"
printf '```bash\nbloque sin cerrar\n' >> "$d/AGENTS.md"
expect "$d" 1 "fences impares"

# NEGATION: a line-number citation is invalidated by every edit above it
# (the AGENTS.md rule the prune enforced by hand); the gate now reads it.
# 5. line-number citation
d="$(new_fixture line-cite)"
printf "Ver \`crates/webfang_core/src/cli/error.rs:42\` para el código.\n" >> "$d/AGENTS.md"
expect "$d" 1 "cita con número de línea"

# 6. the allow-marker exempts its own line (intentional edge cases)
d="$(new_fixture line-cite-allow)"
printf "Ejemplo histórico \`cli/error.rs:42\` <!-- line-cite:allow -->\n" >> "$d/AGENTS.md"
expect "$d" 0 "cita con marcador de excepción"

# 7. extension-anchored regex: issue refs, versions, sockets never match
d="$(new_fixture no-false-positives)"
cat >> "$d/AGENTS.md" <<'EOF'
Obs #2307, versión v8.0.0, endpoint 127.0.0.1:8080/mcp, SSH :443.
EOF
expect "$d" 0 "sin falsos positivos (#NNN, vX.Y.Z, IP:puerto)"

# NEGATION: a guard that degrades to "skip" when blind reports safety it
# never established — every unreadable input fails closed.
# 8. fails closed without AGENTS.md
d="$(new_fixture no-doc)"
rm -f "$d/AGENTS.md"
expect "$d" 1 "sin AGENTS.md (fail-closed)"

# NEGATION: without the baseline there is no budget to enforce; a missing
# file must red, never default to "anything goes".
# 9. fails closed without baseline
d="$(new_fixture no-baseline)"
rm -f "$d/scripts/quality-baselines.json"
expect "$d" 1 "sin baseline (fail-closed)"

# NEGATION: a corrupted baseline is not a budget — bad JSON fails closed.
# 10. invalid JSON baseline
d="$(new_fixture bad-json)"
printf '{ agents_doc: broken' > "$d/scripts/quality-baselines.json"
expect "$d" 1 "baseline con JSON inválido (fail-closed)"

# NEGATION: a baseline without the agents_doc key cannot enforce anything.
# 11. missing key
d="$(new_fixture missing-key)"
printf '{"duplicated_lines_rust": 8219}\n' > "$d/scripts/quality-baselines.json"
expect "$d" 1 "baseline sin la clave agents_doc (fail-closed)"

# NEGATION: a non-integer budget is a measurement error, not a ceiling.
# 12. non-integer budget
d="$(new_fixture bad-budget)"
printf '{"agents_doc": {"max_lines": "650", "max_bytes": 40960}}\n' > "$d/scripts/quality-baselines.json"
expect "$d" 1 "presupuesto no entero (fail-closed)"

# 13. budget pin: the committed baseline carries exactly the owner-approved
# ceiling. A silent JSON edit that raises it breaks this pin (raising is a
# maintainer-reviewed act that must update this harness in the same PR).
pin="$(python3 - "$REPO_ROOT/scripts/quality-baselines.json" <<'PY'
import json, sys
b = json.load(open(sys.argv[1]))["agents_doc"]
print("%d %d" % (b["max_lines"], b["max_bytes"]))
PY
)"
if [[ "$pin" == "650 40960" ]]; then
  pass "presupuesto comprometido pineado (650 / 40960)"
else
  fail "presupuesto comprometido cambió: '$pin' (esperaba '650 40960')"
fi

# 14. --update lowers descending-only and persists
d="$(new_fixture update-down)"
got=0
bash "$d/scripts/check_agents_doc.sh" --update max_lines=600 >"$d/out.log" 2>&1 || got=$?
after="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['agents_doc']['max_lines'])" "$d/scripts/quality-baselines.json")"
if [[ $got -eq 0 && "$after" == "600" ]]; then
  pass "--update descendente aplicado (650 -> 600)"
else
  fail "--update descendente (exit $got, max_lines=$after)"
fi

# NEGATION: the ratchet is DESCENDING — an upward --update is the exact move
# an agent would make to pass a grown doc, and it must be refused untouched.
# 15. --update refuses to raise, file unchanged
d="$(new_fixture update-up)"
before="$(cat "$d/scripts/quality-baselines.json")"
got=0
bash "$d/scripts/check_agents_doc.sh" --update max_lines=700 >"$d/out.log" 2>&1 || got=$?
after="$(cat "$d/scripts/quality-baselines.json")"
if [[ $got -eq 1 && "$before" == "$after" ]]; then
  pass "--update ascendente rechazado sin tocar el JSON"
else
  fail "--update ascendente (exit $got, modificado=$([[ "$before" != "$after" ]] && echo sí || echo no))"
fi

# NEGATION: an unknown key would smuggle a new budget past the pin.
# 16. --update refuses unknown keys
d="$(new_fixture update-unknown)"
got=0
bash "$d/scripts/check_agents_doc.sh" --update max_words=100 >"$d/out.log" 2>&1 || got=$?
if [[ $got -eq 1 ]]; then
  pass "--update con clave desconocida rechazado"
else
  fail "--update con clave desconocida (exit $got)"
fi

printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
(( FAILED == 0 )) || exit 1
