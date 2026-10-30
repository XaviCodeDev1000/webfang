#!/usr/bin/env bash
# The rotation event: invoked when vX.(Y+1).0 ships. Demotes the previous
# STABLE to MAINTENANCE (security-only), EOLs the previous MAINTENANCE via
# eol-line.sh (which announces the branch deletion as a runbook step), and
# registers the new STABLE at the HEAD of the list — newest first, so the
# rendered SUPPORT.md table keeps the descending order of the file it derives
# from (appending put the current release last in the published support
# table).
# Idempotent per argument: re-running with the same version aborts instead
# of duplicating entries (governance scripts must be re-runnable without
# damage — agents re-invoke on state confusion).
#
# MODES
#   scripts/rotate-stable.sh X.Y                        rotates AFTER the
#                                                    publication: the premise
#                                                    is that the tag vX.Y.0
#                                                    exists.
#   scripts/rotate-stable.sh X.Y --pending-publication  rotates BEFORE the
#                                                    publication: the premise
#                                                    is that the workspace
#                                                    version of THIS checkout
#                                                    is X.Y.0.
#
# WHY THE PENDING MODE EXISTS (issue #2531, option A)
# A minor release publishes its tag only AFTER the Release PR merges, so the
# Release PR itself is where the new support window has to be declared: the
# tag it is about to publish does not exist yet. The tag premise is TRUE for
# the manual governance path (an agent rotates after the publication) and
# FALSE for the Release PR path — with it, the rotation could not run before
# the merge, and the release published a tag whose declaration did not exist:
# the drift that reddened every pull request in the repository (#2052, #2520)
# and expelled unrelated PRs from the merge queue.
#
# The pending mode replaces the tag premise with the version this checkout is
# PREPARING — the version the Release PR carries and the version its tag will
# be cut from — read with the same sed discipline the release flow uses
# (scripts/apply_release_pins.sh: one grammar for the version, not two). The
# default mode keeps the tag premise untouched.
#
# WHAT THE PENDING MODE ALSO DOES
#   - it is IDEMPOTENT: a line that is already STABLE is left alone and the
#     script exits 0 without writing. release-plz regenerates the Release PR
#     branch on every push to main, so the flow can meet its own work;
#   - it re-renders SUPPORT.md through its single writer, because the Release
#     PR commits the declaration AND its projection together: the declared
#     scope compares them, and a Release PR carrying only one of the two
#     would fail the very gate this rotation exists to satisfy. The default
#     mode leaves the render to its operator, as it always did.
#
# Run inside a chore/support-* branch (base main); the mutation travels by PR.
#
# FAIL CLOSED (#1676)
# This script only knows the lines support.json already carries. Rotating over
# a gap — minors already published but never registered — silently produced a
# file that declared a stale line MAINTENANCE (it should have been EOL) and
# vanished the intermediate minors, i.e. a WORSE file, with no signal. So the
# rotation now refuses when it cannot see what it is rotating over, when the
# minor was never published, or when there is no STABLE to demote. Refusing is
# the correct outcome: closing a gap is a governance decision (approved issue),
# not a script run.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

usage() {
  cat <<'EOF'
uso: rotate-stable.sh X.Y [--pending-publication]

  X.Y                        la línea de soporte a rotar
  --pending-publication      rota ANTES de que exista el tag: la premisa es
                             que la versión del workspace de este checkout es
                             X.Y.0 (la que el Release PR prepara y de la que
                             se cortará el tag)
EOF
}

# The flag is accepted before or after the line, and the no-argument error
# stays the one this script has always produced (`${1:?}`): the default mode's
# contract does not move under its users.
MODE="published"
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi
if [[ "${1:-}" == "--pending-publication" ]]; then
  MODE="pending"
  shift
fi
NEW_MINOR="${1:?usage: rotate-stable.sh X.Y (recién publicada)}"
shift
if [[ "${1:-}" == "--pending-publication" ]]; then
  MODE="pending"
  shift
fi
if [[ $# -gt 0 ]]; then
  echo "argumento inesperado: $1" >&2
  usage >&2
  exit 2
fi
SUPPORT_JSON=".github/support.json"
[[ -f "$SUPPORT_JSON" ]] || { echo "$SUPPORT_JSON no existe" >&2; exit 1; }
[[ "$NEW_MINOR" =~ ^[0-9]+\.[0-9]+$ ]] || { echo "línea inválida '$NEW_MINOR': se espera X.Y" >&2; exit 1; }
NOW="$(date -Iseconds)"

# read_workspace_version: [workspace.package] version of THIS checkout, read
# with the SAME sed discipline scripts/apply_release_pins.sh uses — one
# grammar for the version, not two.
read_workspace_version() {
  sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml \
    | sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1
}

# ─── The premise of the rotation ─────────────────────────────────────────────
if [[ "$MODE" == "pending" ]]; then
  # The rotation is about to publish: the premise is the version THIS
  # checkout is preparing — the version the Release PR carries and the
  # version its tag will be cut from.
  WS_VERSION="$(read_workspace_version)"
  if [[ -z "$WS_VERSION" ]]; then
    echo "no se pudo leer la versión del workspace de este checkout: la rotación pendiente se niega, no se adivina" >&2
    exit 1
  fi
  if [[ "$WS_VERSION" != "${NEW_MINOR}.0" ]]; then
    echo "la versión del workspace de este checkout es $WS_VERSION y la rotación pendiente es de ${NEW_MINOR}.0: la rotación se niega" >&2
    exit 1
  fi
fi

EXISTS="$(jq -r --arg m "$NEW_MINOR" '.lines[] | select(.minor==$m) | .minor // empty' "$SUPPORT_JSON")"
if [[ -n "$EXISTS" ]]; then
  if [[ "$MODE" == "pending" ]]; then
    EXISTING_STATE="$(jq -r --arg m "$NEW_MINOR" '.lines[] | select(.minor==$m) | .state // empty' "$SUPPORT_JSON")"
    if [[ "$EXISTING_STATE" == "STABLE" ]]; then
      # Already rotated: release-plz regenerates the Release PR branch on every
      # push to main, so the flow can meet its own work. Nothing is written —
      # not even a timestamp.
      echo "OK: la línea $NEW_MINOR ya es STABLE: la rotación ya está declarada, no se escribe nada."
      exit 0
    fi
    echo "línea $NEW_MINOR ya declarada en estado $EXISTING_STATE: rotar sobre una línea ya declarada no se hace" >&2
    exit 1
  fi
  echo "línea $NEW_MINOR ya registrada — no reintentar con el mismo argumento" >&2
  exit 1
fi

# The rotation event is "vX.(Y+1).0 ships". If that tag does not exist, the
# premise is false — refuse rather than declare a line for a release that never
# happened. (The pending mode replaces this premise with the workspace
# version, checked above.)
if [[ "$MODE" != "pending" ]]; then
  git rev-parse --verify --quiet "refs/tags/v${NEW_MINOR}.0" >/dev/null || {
    echo "v${NEW_MINOR}.0 no está publicado: la rotación presupone que la minor ya salió" >&2; exit 1; }
fi

CURRENT_STABLE="$(jq -r '.lines[] | select(.state=="STABLE") | .minor // empty' "$SUPPORT_JSON")"
[[ -n "$CURRENT_STABLE" ]] || {
  echo "no hay ninguna línea STABLE declarada que degradar — sanear support.json primero" >&2; exit 1; }

# Published minors strictly between the declared STABLE and the requested one
# that support.json does not know about. Rotating here would leave the chain
# broken exactly as it already is, but now silently and with the intermediate
# minors still undeclared.
#
# Both sides are compared NUMERICALLY inside one awk pass on purpose. They used
# to be diffed with `comm`, which demands each input be sorted in the LOCALE's
# collation order, while the declared side was sorted in VERSION order — where
# 2.9 precedes 2.10 and collation says the opposite. The first rotation whose
# declared lines crossed that pair made comm call the input unsorted and abort
# the whole script, failing precisely on the release that needed it. Bringing
# the declared set into awk removes the ordering dependency instead of hiding it
# behind LC_ALL.
PUBLISHED_MINORS="$(git tag --list 'v*' | sed 's/^v//' | grep -v -- '-' \
  | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | cut -d. -f1,2 | sort -u -V || true)"
# The `|| true` is not a shrug: the grep of an EMPTY tag stream exits 1, and
# under `pipefail` that made the assignment fail and `set -e` killed the
# script with NO message at all — a rotation that dies silently on a
# repository without tags. No tags means no published minors, which is an
# answer (the gap is empty), not a crash. The default mode never reaches
# this line without a tag (the tag premise comes first), so this only
# concerns the pending mode.
GAP="$(printf '%s\n' "$PUBLISHED_MINORS" | awk \
  -v s="$CURRENT_STABLE" -v n="$NEW_MINOR" \
  -v declared="$(jq -r '.lines[].minor' "$SUPPORT_JSON" | tr '\n' ' ')" '
    function gt(a, b,  x, y, i, m, k) {
      m = split(a, x, "."); k = split(b, y, ".")
      for (i = 1; i <= 3; i++) {
        if ((i <= m ? x[i] + 0 : 0) != (i <= k ? y[i] + 0 : 0)) return (i <= m ? x[i] + 0 : 0) > (i <= k ? y[i] + 0 : 0)
      }
      return 0
    }
    function is_declared(v,  i) {
      for (i = 1; i <= dn; i++) if (d[i] == v) return 1
      return 0
    }
    BEGIN { dn = split(declared, d, " ") }
    gt($0, s) && gt(n, $0) && !is_declared($0) { print }')"
if [[ -n "$GAP" ]]; then
  echo "líneas publicadas sin declarar entre $CURRENT_STABLE y $NEW_MINOR: $(echo "$GAP" | tr '\n' ' ')" >&2
  echo "rotar por encima de ellas dejaría la cadena rota en silencio; registrarlas (o marcarlas EOL) es una decisión de gobernanza con issue aprobada detrás" >&2
  exit 1
fi

CURRENT_MAINTENANCE="$(jq -r '.lines[] | select(.state=="MAINTENANCE") | .minor // empty' "$SUPPORT_JSON")"
if [[ -n "$CURRENT_MAINTENANCE" ]]; then
  # shellcheck disable=SC2086
  for m in $CURRENT_MAINTENANCE; do ./scripts/eol-line.sh "$m"; done
fi

jq --arg new "$NEW_MINOR" --arg at "$NOW" '
  (.lines[] | select(.state=="STABLE")) |=
    (.state = "MAINTENANCE" | .demoted_from_stable_at = $at | .accepts = ["security"])
  | .lines = [{
      "minor": $new, "state": "STABLE", "branch": null,
      "latest": ($new + ".0"), "released_at": $at,
      "accepts": ["bugfix", "security"]
    }] + .lines
' "$SUPPORT_JSON" > "${SUPPORT_JSON}.tmp"
mv "${SUPPORT_JSON}.tmp" "$SUPPORT_JSON"
if [[ "$MODE" == "pending" ]]; then
  # The projection travels with the declaration: the Release PR commits both,
  # and the renderer is the single writer of SUPPORT.md.
  bash scripts/render-support-md.sh >/dev/null
fi
echo "OK: $NEW_MINOR es STABLE; anterior STABLE a MAINTENANCE."
