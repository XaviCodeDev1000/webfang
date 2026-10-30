#!/usr/bin/env bash
# Semantics harness for scripts/eol-line.sh (issue #2531, option A).
#
# WHY THIS EXISTS
# Declaring a support line EOL used to delete its branch as a side effect:
# `git push origin --delete support/X.Y` (remote) and `git branch -D` (local).
# That made the script a remote-mutating actor inside a flow that now runs
# UNATTENDED: the release-plz Release PR rotates the support window before the
# tag exists, and that flow runs with `persist-credentials: false` — only the
# push helper of scripts/declare_pending_publication.sh injects a token. A
# rotation that half-succeeds (the state written, the deletion rejected)
# leaves the repository in two places at once, and a deletion that DOES land
# removes a branch nobody approved.
#
# THE DECISION (maintainer, option A)
# The branch deletion is NOT automatic. The state is written (it is the source
# of truth), and the deletion becomes a runbook step announced with a
# `::notice` that names the exact command. The Release PR's diff shows the
# EOL line, and the human approves it while reviewing the PR.
#
# Fixture: a real repo pushed to a LOCAL bare repo as `origin`, so a deletion
# WOULD succeed if the script attempted one — the assertion "the remote branch
# survives" is therefore falsifiable, not vacuous. Same git pinning discipline
# as test_support_drift.sh and test_cut_support_branch.sh (no gpgsign, no
# hooks, no inherited global config).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# new_fx <name> [branch_2_1]: repo + local bare origin, main pushed, eol script
# installed. Lines: 2.2 STABLE, 2.1 MAINTENANCE (with a materialized branch,
# unless the second argument is empty: a branchless MAINTENANCE line — branch
# null — is a shape the declaration model itself allows), 2.0 EOL.
new_fx() {
  local bare="$WORK/$1.git" repo="$WORK/$1-clone" branch_21="${2-support/2.1}"
  local branch_21_json=""
  if [[ -n "$branch_21" ]]; then branch_21_json="\"$branch_21\""; else branch_21_json="null"; fi
  mkdir -p "$repo/scripts"
  cp "$REPO_ROOT/scripts/eol-line.sh" "$repo/scripts/"
  chmod +x "$repo/scripts/eol-line.sh"
  git -C "$repo" init -q -b main
  git -C "$repo" config user.email fixture@example.invalid
  git -C "$repo" config user.name fixture
  git -C "$repo" config commit.gpgsign false
  git -C "$repo" config tag.gpgsign false
  git -C "$repo" config core.hooksPath /dev/null
  git -C "$repo" config commit.template /dev/null
  git init -q --bare "$bare"
  git -C "$repo" remote add origin "$bare"
  mkdir -p "$repo/.github"
  printf '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.0","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":%s,"latest":"2.1.0","accepts":["security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}
' "$branch_21_json" > "$repo/.github/support.json"
  git -C "$repo" add -A
  git -C "$repo" commit -q -m init
  git -C "$repo" push -q origin main 2>/dev/null
  # The support branch of the MAINTENANCE line, local AND remote: a deletion
  # would have something real to delete. A branchless line has no branch to
  # create — that is the point of the shape.
  if [[ -n "$branch_21" ]]; then
    git -C "$repo" branch "$branch_21"
    git -C "$repo" push -q origin "$branch_21" 2>/dev/null
  fi
  echo "$repo"
}

# run_eol <fixture> <minor>: LAST_RC / LAST_OUT carry the verdict.
run_eol() {
  local dir="$1" minor="$2"
  local rc=0
  ( cd "$dir" && ./scripts/eol-line.sh "$minor" ) > "$WORK/out.txt" 2>&1 || rc=$?
  LAST_RC=$rc
  LAST_OUT="$(cat "$WORK/out.txt")"
}

echo "== el estado EOL se escribe; la rama NO se borra =="

FX="$(new_fx eol_keeps_branch)"
run_eol "$FX" 2.1
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "declarar EOL termina bien"
else
  fail "declarar EOL falló (rc=$LAST_RC): $LAST_OUT"
fi

STATE="$(jq -r '.lines[] | select(.minor=="2.1") | .state' "$FX/.github/support.json")"
if [[ "$STATE" == "EOL" ]]; then
  pass "la línea 2.1 queda EOL"
else
  fail "la línea 2.1 no quedó EOL (state=$STATE)"
fi
if [[ "$(jq -r '.lines[] | select(.minor=="2.1") | .branch // "null"' "$FX/.github/support.json")" == "null" ]]; then
  pass "la línea EOL no ancla ninguna rama"
else
  fail "la línea EOL sigue anclando una rama"
fi
if [[ -n "$(jq -r '.lines[] | select(.minor=="2.1") | .eol_at // ""' "$FX/.github/support.json")" ]] \
   && [[ -n "$(jq -r '.lines[] | select(.minor=="2.1") | .eol_reason // ""' "$FX/.github/support.json")" ]]; then
  pass "la línea EOL lleva eol_at y eol_reason"
else
  fail "la línea EOL quedó sin eol_at/eol_reason"
fi
if [[ "$(jq -r '[.lines[] | select(.minor=="2.1") | (.accepts // []) | length] | first' "$FX/.github/support.json")" == "0" ]]; then
  pass "la línea EOL no admite parches (accepts vacío)"
else
  fail "la línea EOL conserva accepts"
fi

# NEGATION: pre-#2531 the script deleted the branch on its own: the remote
# `git push origin --delete` landed and the local `git branch -D` removed it.
# The Release PR path runs without the credentials that deletion needs, and a
# deletion nobody approved is not a consequence anybody can audit.
if git -C "$FX" remote get-url origin >/dev/null 2>&1 \
   && git -C "$FX" ls-remote --exit-code --heads origin support/2.1 >/dev/null 2>&1; then
  pass "la rama remota support/2.1 SIGUE viva: el borrado no es automático"
else
  fail "la rama remota support/2.1 fue borrada: el script se sigue adjudicando el borrado"
fi
if git -C "$FX" show-ref --verify --quiet "refs/heads/support/2.1"; then
  pass "la rama local support/2.1 SIGUE viva: el borrado no es automático"
else
  fail "la rama local support/2.1 fue borrada: el script se sigue adjudicando el borrado"
fi

if grep -qF '::notice' <<<"$LAST_OUT" && grep -qF 'git push origin --delete support/2.1' <<<"$LAST_OUT"; then
  pass "el ::notice anuncia el comando exacto del runbook"
else
  fail "no hay un ::notice con el comando del runbook: $LAST_OUT"
fi
if grep -qF 'OK: línea 2.1 en EOL.' <<<"$LAST_OUT"; then
  pass "el contrato de salida (OK: línea X.Y en EOL.) sigue intacto"
else
  fail "la línea de OK cambió o desapareció: $LAST_OUT"
fi

echo "== una línea MAINTENANCE sin rama no anuncia ningún borrado =="

# The declaration model allows a MAINTENANCE line with branch null (the drift
# fixtures use exactly that shape), and the rotation EOLs the previous
# MAINTENANCE line through this script. For such a line there is no branch to
# delete: announcing `git push origin --delete` would hand the runbook a
# command that cannot succeed. The baseline answered an absent branch with its
# own message and no command; the notice-as-runbook flow has to keep that
# honesty.
# NEGATION: pre-fix both notices interpolated the branch name unconditionally,
# so a branchless line got a deletion command with a name that does not exist.
FX_NULL="$(new_fx eol_branchless "")"
run_eol "$FX_NULL" 2.1
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "declarar EOL de una línea sin rama termina bien"
else
  fail "declarar EOL de una línea sin rama falló (rc=$LAST_RC): $LAST_OUT"
fi
STATE_NULL="$(jq -r '.lines[] | select(.minor=="2.1") | .state' "$FX_NULL/.github/support.json")"
if [[ "$STATE_NULL" == "EOL" ]]; then
  pass "la línea 2.1 sin rama queda EOL"
else
  fail "la línea 2.1 sin rama no quedó EOL (state=$STATE_NULL)"
fi
if grep -qF 'git push origin --delete' <<<"$LAST_OUT"; then
  fail "una línea sin rama NO anuncia ningún comando de borrado: $LAST_OUT"
else
  pass "una línea sin rama NO anuncia ningún comando de borrado"
fi
if grep -qF 'OK: línea 2.1 en EOL.' <<<"$LAST_OUT"; then
  pass "el contrato de salida (OK: línea X.Y en EOL.) vale también sin rama"
else
  fail "la línea de OK cambió o desapareció para la línea sin rama: $LAST_OUT"
fi

echo "== una línea inexistente falla cerrada y no muta nada =="

FX_MISSING="$(new_fx eol_unknown)"
BEFORE="$(cat "$FX_MISSING/.github/support.json")"
run_eol "$FX_MISSING" 9.9
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "una línea inexistente se niega"
else
  fail "una línea inexistente terminó con rc=0"
fi
if [[ "$(cat "$FX_MISSING/.github/support.json")" == "$BEFORE" ]]; then
  pass "el rechazo deja support.json byte a byte intacto"
else
  fail "el rechazo mutó support.json"
fi
if git -C "$FX_MISSING" show-ref --verify --quiet "refs/heads/support/2.1"; then
  pass "el rechazo no toca ninguna rama"
else
  fail "el rechazo movió una rama"
fi
if grep -qF '9.9' <<<"$LAST_OUT"; then
  pass "el rechazo nombra la línea"
else
  fail "el rechazo no nombra la línea: $LAST_OUT"
fi

echo
echo "harness: $PASSED ok, $FAILED fail"
[[ "$FAILED" -eq 0 ]]
