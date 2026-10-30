#!/usr/bin/env bash
# Semantics harness for scripts/rotate-stable.sh (issue #2531, option A).
#
# WHY THIS EXISTS
# The rotation event is "vX.(Y+1).0 ships", and the script used to prove it by
# requiring the tag to exist. That premise is TRUE for the manual governance
# path (an agent rotates after the publication) and FALSE for the path that
# now owns the rotation: a minor release publishes its tag only AFTER the
# Release PR merges, so the Release PR itself has to carry the new support
# window. With the tag premise, the rotation could not run before the
# publication — and the release then published a tag whose declaration did not
# exist, which is the exact drift that reddened every pull request in the
# repository (#2052, #2520) and expelled unrelated PRs from the merge queue.
#
# The pending mode replaces the tag premise with the version THIS checkout is
# preparing — the version the Release PR carries and the version its tag will
# be cut from — read with the same sed discipline the release flow already
# uses. The default mode keeps the tag premise untouched.
#
# Fixture: a real repo pushed to a LOCAL bare repo as `origin`, with
# synthetic tags, so no network and no mutation of this repository. Same git
# pinning discipline as the other support harnesses (no gpgsign, no hooks, no
# inherited global config).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# new_fx <name> <workspace_version>: repo + local bare origin, main pushed,
# rotation scripts installed, support.json declared (2.1 STABLE, 2.0
# MAINTENANCE, 1.0 EOL) and SUPPORT.md rendered.
new_fx() {
  local name="$1" version="$2"
  local bare="$WORK/$name.git" repo="$WORK/$name-clone"
  mkdir -p "$repo/scripts" "$repo/.github"
  cp "$REPO_ROOT/scripts/rotate-stable.sh" "$repo/scripts/"
  cp "$REPO_ROOT/scripts/eol-line.sh" "$repo/scripts/"
  cp "$REPO_ROOT/scripts/render-support-md.sh" "$repo/scripts/"
  chmod +x "$repo/scripts/"*.sh
  git -C "$repo" init -q -b main
  git -C "$repo" config user.email fixture@example.invalid
  git -C "$repo" config user.name fixture
  git -C "$repo" config commit.gpgsign false
  git -C "$repo" config tag.gpgsign false
  git -C "$repo" config core.hooksPath /dev/null
  git -C "$repo" config commit.template /dev/null
  git init -q --bare "$bare"
  git -C "$repo" remote add origin "$bare"
  printf '[workspace.package]\nversion = "%s"\nedition = "2024"\n\n[workspace]\nmembers = []\n' \
    "$version" > "$repo/Cargo.toml"
  printf '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.1","state":"STABLE","branch":null,"latest":"2.1.0","accepts":["bugfix","security"]},
    {"minor":"2.0","state":"MAINTENANCE","branch":"support/2.0","latest":"2.0.0","accepts":["security"]},
    {"minor":"1.0","state":"EOL","branch":null,"latest":"1.0.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}
' > "$repo/.github/support.json"
  git -C "$repo" add -A
  git -C "$repo" commit -q -m init
  git -C "$repo" push -q origin main 2>/dev/null
  # The support branch of the MAINTENANCE line, local AND remote: the rotation
  # EOLs that line, and the assertion "the branch survives" is only
  # falsifiable when there is a branch to survive.
  git -C "$repo" branch support/2.0
  git -C "$repo" push -q origin support/2.0 2>/dev/null
  ( cd "$repo" && ./scripts/render-support-md.sh >/dev/null )
  git -C "$repo" add -A
  git -C "$repo" commit -q -m rendered
  echo "$repo"
}

# add_tags <fixture> <tag...>: lightweight tags on HEAD, local and remote.
add_tags() {
  local dir="$1"; shift
  local tag
  for tag in "$@"; do
    git -C "$dir" tag -f "$tag" >/dev/null
  done
  git -C "$dir" push -q origin --tags 2>/dev/null
}

# set_version <fixture> <version>: the version this checkout is preparing.
set_version() {
  local dir="$1" version="$2"
  sed -i "s/^version = \".*\"$/version = \"$version\"/" "$dir/Cargo.toml"
}

# run_rotate <fixture> <minor> [args...]: LAST_RC / LAST_OUT carry the verdict.
run_rotate() {
  local dir="$1" minor="$2"; shift 2
  local rc=0
  ( cd "$dir" && ./scripts/rotate-stable.sh "$minor" "$@" ) > "$WORK/out.txt" 2>&1 || rc=$?
  LAST_RC=$rc
  LAST_OUT="$(cat "$WORK/out.txt")"
}

# state_of <fixture> <minor>: the declared state of a line.
state_of() {
  jq -r --arg m "$2" '.lines[] | select(.minor==$m) | .state // empty' "$1/.github/support.json"
}

# line_of <fixture> <minor>: the declared line as compact JSON.
line_of() {
  jq -c --arg m "$2" '.lines[] | select(.minor==$m)' "$1/.github/support.json"
}

echo "== la rotación pendiente: el tag todavía no existe =="

FX="$(new_fx pending 2.2.0)"
add_tags "$FX" v1.0.0 v2.0.0 v2.1.0
run_rotate "$FX" 2.2 --pending-publication
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "la rotación pendiente procede sin que exista el tag v2.2.0"
else
  fail "la rotación pendiente se negó (rc=$LAST_RC): $LAST_OUT"
fi
# NEGATION: pre-#2531 the script required the published tag, so the Release
# PR could not carry the window it was about to publish: the tag lands after
# the merge, and the declaration had to land before it.
if [[ -z "$(git -C "$FX" tag --list 'v2.2.0')" ]]; then
  pass "la premisa es la versión del workspace, no el tag (v2.2.0 sigue sin existir)"
else
  fail "la fixture ya tenía el tag: la prueba de la premisa no dice nada"
fi
if [[ "$(state_of "$FX" 2.2)" == "STABLE" ]]; then
  pass "la línea 2.2 queda STABLE"
else
  fail "la línea 2.2 no quedó STABLE (state=$(state_of "$FX" 2.2))"
fi
if [[ "$(jq -r '.lines[] | select(.minor=="2.2") | .latest' "$FX/.github/support.json")" == "2.2.0" ]]; then
  pass "la línea 2.2 declara latest 2.2.0"
else
  fail "la línea 2.2 no declara latest 2.2.0"
fi
if [[ "$(jq -c '.lines[] | select(.minor=="2.2") | .accepts' "$FX/.github/support.json")" == '["bugfix","security"]' ]]; then
  pass "la línea 2.2 STABLE admite bugfix y security"
else
  fail "la línea 2.2 no admite bugfix y security"
fi
if [[ "$(state_of "$FX" 2.1)" == "MAINTENANCE" ]]; then
  pass "la anterior STABLE 2.1 queda MAINTENANCE"
else
  fail "la anterior STABLE 2.1 no quedó MAINTENANCE (state=$(state_of "$FX" 2.1))"
fi
if [[ "$(jq -c '.lines[] | select(.minor=="2.1") | .accepts' "$FX/.github/support.json")" == '["security"]' ]]; then
  pass "la línea 2.1 MAINTENANCE admite sólo security"
else
  fail "la línea 2.1 MAINTENANCE admite más que security"
fi
if [[ "$(state_of "$FX" 2.0)" == "EOL" ]]; then
  pass "la línea 2.0 (la tercera) queda EOL"
else
  fail "la línea 2.0 no quedó EOL (state=$(state_of "$FX" 2.0))"
fi
if [[ -n "$(jq -r '.lines[] | select(.minor=="2.0") | .eol_at // empty' "$FX/.github/support.json")" ]]; then
  pass "la línea EOL lleva eol_at"
else
  fail "la línea EOL quedó sin eol_at"
fi
if grep -qF 'git push origin --delete support/2.0' <<<"$LAST_OUT"; then
  pass "la rotación anuncia el comando de borrado de la rama EOL"
else
  fail "la rotación no anuncia el comando de borrado: $LAST_OUT"
fi
if git -C "$FX" ls-remote --exit-code --heads origin support/2.0 >/dev/null 2>&1; then
  pass "la rama remota support/2.0 SIGUE viva: la rotación no borra ramas"
else
  fail "la rama remota support/2.0 fue borrada por la rotación"
fi

echo "== la proyección queda coherente en el mismo commit =="

if ( cd "$FX" && ./scripts/render-support-md.sh --check >/dev/null 2>&1 ); then
  pass "SUPPORT.md coincide con el render de la declaración rotada"
else
  fail "SUPPORT.md quedó desactualizada respecto de la declaración rotada"
fi

echo "== el modo por defecto sigue exigiendo el tag publicado =="

FX_DEF="$(new_fx default_mode 2.2.0)"
add_tags "$FX_DEF" v1.0.0 v2.0.0 v2.1.0
BEFORE_DEF="$(cat "$FX_DEF/.github/support.json")"
run_rotate "$FX_DEF" 2.2
# NEGATION: the default mode keeps the tag premise. Loosening it would let a
# rotation declare a line for a release that never happened.
if [[ "$LAST_RC" -ne 0 ]] && grep -qF 'v2.2.0 no está publicado' <<<"$LAST_OUT"; then
  pass "sin --pending-publication, la rotación exige el tag publicado"
else
  fail "el modo por defecto dejó de exigir el tag (rc=$LAST_RC): $LAST_OUT"
fi
if [[ "$(cat "$FX_DEF/.github/support.json")" == "$BEFORE_DEF" ]]; then
  pass "el rechazo del modo por defecto deja support.json byte a byte intacto"
else
  fail "el rechazo del modo por defecto mutó support.json"
fi

echo "== el modo por defecto sigue rotando cuando el tag existe =="

FX_PUB="$(new_fx published 2.2.0)"
add_tags "$FX_PUB" v1.0.0 v2.0.0 v2.1.0 v2.2.0
run_rotate "$FX_PUB" 2.2
if [[ "$LAST_RC" -eq 0 ]] && [[ "$(state_of "$FX_PUB" 2.2)" == "STABLE" ]]; then
  pass "con el tag publicado, la rotación de siempre sigue funcionando"
else
  fail "la rotación publicada se rompió (rc=$LAST_RC): $LAST_OUT"
fi

echo "== la versión del workspace es la premisa, no una suposición =="

FX_VER="$(new_fx wrong_version 2.3.0)"
add_tags "$FX_VER" v1.0.0 v2.0.0 v2.1.0
BEFORE_VER="$(cat "$FX_VER/.github/support.json")"
run_rotate "$FX_VER" 2.2 --pending-publication
# NEGATION: the pending mode does not rotate "whatever looks next": the
# checkout that prepares 2.3.0 may not declare 2.2.
if [[ "$LAST_RC" -ne 0 ]] && grep -qF 'la versión del workspace de este checkout es' <<<"$LAST_OUT"; then
  pass "una versión de workspace distinta de X.Y.0 se niega (por la versión, no por otra cosa)"
else
  fail "la rotación pendiente aceptó una versión de workspace que no le corresponde (rc=$LAST_RC): $LAST_OUT"
fi
if [[ "$(cat "$FX_VER/.github/support.json")" == "$BEFORE_VER" ]]; then
  pass "el rechazo por versión deja support.json byte a byte intacto"
else
  fail "el rechazo por versión mutó support.json"
fi

echo "== el GAP check sigue vivo en el modo pendiente =="

FX_GAP="$(new_fx gap 2.3.0)"
# 2.2 is published but never declared: rotating over it would leave the chain
# broken, and closing that gap is a governance decision, not a script run.
add_tags "$FX_GAP" v1.0.0 v2.0.0 v2.1.0 v2.2.0
BEFORE_GAP="$(cat "$FX_GAP/.github/support.json")"
run_rotate "$FX_GAP" 2.3 --pending-publication
if [[ "$LAST_RC" -ne 0 ]] && grep -qF 'sin declarar entre' <<<"$LAST_OUT"; then
  pass "una minor publicada sin declarar entre medio se niega (por el gap, no por otra cosa)"
else
  fail "la rotación pendiente rotó por encima de una minor sin declarar (rc=$LAST_RC): $LAST_OUT"
fi
if [[ "$(cat "$FX_GAP/.github/support.json")" == "$BEFORE_GAP" ]]; then
  pass "el rechazo por gap deja support.json byte a byte intacto"
else
  fail "el rechazo por gap mutó support.json"
fi

echo "== la rotación pendiente en un repositorio sin tags =="

# The `|| true` rescue on the published-minors pipeline exists for exactly
# this repository: zero tags, an empty published set, an empty gap. Every
# other fixture publishes tags before rotating, so the one line whose stated
# purpose is the tagless scenario was unproved (RDD review of this PR:
# R3-tagless-pending-rotation-untested). The rotation proceeds — an empty
# published set is an answer, not a crash — and the window still rotates
# whole: new STABLE, previous STABLE demoted, third line EOL.
FX_NOTAG="$(new_fx tagless 2.2.0)"
run_rotate "$FX_NOTAG" 2.2 --pending-publication
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "una rotación pendiente sin ningún tag procede"
else
  fail "una rotación pendiente sin ningún tag se negó (rc=$LAST_RC): $LAST_OUT"
fi
if [[ "$(state_of "$FX_NOTAG" 2.2)" == "STABLE" ]] \
  && [[ "$(jq -r '.lines[] | select(.minor=="2.2") | .latest' "$FX_NOTAG/.github/support.json")" == "2.2.0" ]]; then
  pass "la línea 2.2 queda STABLE declarando latest 2.2.0"
else
  fail "la línea 2.2 no quedó STABLE con latest 2.2.0 (state=$(state_of "$FX_NOTAG" 2.2))"
fi
if [[ "$(state_of "$FX_NOTAG" 2.1)" == "MAINTENANCE" ]] && [[ "$(state_of "$FX_NOTAG" 2.0)" == "EOL" ]]; then
  pass "la ventana rota entera: 2.1 MAINTENANCE y 2.0 EOL"
else
  fail "la ventana no rotó entera (2.1=$(state_of "$FX_NOTAG" 2.1), 2.0=$(state_of "$FX_NOTAG" 2.0))"
fi
if [[ -z "$(git -C "$FX_NOTAG" tag --list 'v*')" ]]; then
  pass "la rotación sin tags no publicó ningún tag"
else
  fail "la rotación sin tags creó tags: $(git -C "$FX_NOTAG" tag --list 'v*' | tr '\n' ' ')"
fi

echo "== la rotación pendiente es idempotente =="

FX_IDEM="$(new_fx idempotent 2.2.0)"
add_tags "$FX_IDEM" v1.0.0 v2.0.0 v2.1.0
run_rotate "$FX_IDEM" 2.2 --pending-publication
AFTER_FIRST="$(cat "$FX_IDEM/.github/support.json")"
run_rotate "$FX_IDEM" 2.2 --pending-publication
# NEGATION: release-plz regenerates the Release PR branch on every push to
# main, so the flow can meet its own work. A second run that rotated again
# would duplicate the line or move the timestamps.
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "una segunda rotación pendiente termina bien"
else
  fail "una segunda rotación pendiente falló (rc=$LAST_RC): $LAST_OUT"
fi
if [[ "$(cat "$FX_IDEM/.github/support.json")" == "$AFTER_FIRST" ]]; then
  pass "la segunda rotación pendiente no escribe nada"
else
  fail "la segunda rotación pendiente volvió a escribir la declaración"
fi

echo
echo "harness: $PASSED ok, $FAILED fail"
[[ "$FAILED" -eq 0 ]]
