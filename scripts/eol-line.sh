#!/usr/bin/env bash
# Declares a line EOL: marks the entry and announces the branch deletion as a
# runbook step. The branch is the physical projection of the state, not the
# state itself: support.json is the source of truth, and the branch is deleted
# by an explicit, announced command rather than as a side effect of this
# script. EOL means no fixes, no security fixes, no backports, no releases —
# except via an explicitly approved exception issue (which recreates the line).
#
# WHY THE DELETION IS NOT AUTOMATIC (issue #2531, option A)
# The deletion used to be a side effect of this script: `git push origin
# --delete support/X.Y` plus a local `git branch -D`. That made the script a
# remote-mutating actor inside a flow that now runs UNATTENDED — the
# release-plz Release PR rotates the support window before the tag exists, and
# that flow runs with `persist-credentials: false` (only the push helper of
# scripts/declare_pending_publication.sh injects a token). So the deletion
# either failed, leaving a half-rotated state (the line EOL with its branch
# still published), or it landed without anybody having approved it.
#
# Now the state is written here and the deletion is announced with a
# `::notice` that names the exact command. The Release PR's diff shows the
# EOL line, and the human approves it while reviewing the PR.
#
# Run inside a chore/support-* branch (base main); the mutation travels by PR.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

MINOR="${1:?usage: eol-line.sh X.Y}"
SUPPORT_JSON=".github/support.json"
[[ -f "$SUPPORT_JSON" ]] || { echo "$SUPPORT_JSON no existe" >&2; exit 1; }

# Audit B10: the line is validated BEFORE anything else — a nonexistent minor
# used to fall through `select()` unpatched (jq writes an unchanged document,
# exit 0, "OK: línea X en EOL") AFTER the branch deletion had already run, so
# an orphan branch deleted itself while support.json never knew the line.
if ! jq -e --arg minor "$MINOR" 'any(.lines[]; .minor == $minor)' "$SUPPORT_JSON" >/dev/null; then
  echo "::error::eol-line.sh: la línea $MINOR no existe en $SUPPORT_JSON — falla-cerrada: ninguna mutación (branch ni estado) corrió (audit B10)." >&2
  exit 1
fi

# The branch the line declares BEFORE the EOL write nulls it. The state model
# allows a MAINTENANCE line with branch null (the drift-gate fixtures use
# exactly that shape), and the rotation EOLs the previous MAINTENANCE line
# through this script: for such a line there is no branch to delete, and
# announcing a deletion command would hand the runbook a command that cannot
# succeed. Read here, before any mutation, so a read failure leaves the
# declaration untouched (fail closed, audit B10 discipline).
DECLARED_BRANCH="$(jq -r --arg minor "$MINOR" '(.lines[] | select(.minor == $minor) | (.branch // ""))' "$SUPPORT_JSON")" \
  || { echo "::error::eol-line.sh: no se pudo leer la rama declarada de $MINOR — falla-cerrada: ninguna mutación corrió." >&2; exit 1; }

# State is written FIRST (audit B10): a failed state write must leave the
# declaration exactly as it was. A jq failure aborts BEFORE the write lands —
# nothing is mutated, and nothing else happens in this script.
if ! jq --arg minor "$MINOR" --arg at "$(date -Iseconds)" \
  '(.lines[] | select(.minor == $minor)) |= (
     .state = "EOL" | .branch = null | .eol_at = $at |
     .eol_reason = (.eol_reason // "auto: rotate-stable, third line back from STABLE") |
     del(.accepts)
   )' "$SUPPORT_JSON" > "${SUPPORT_JSON}.tmp"; then
  rm -f "${SUPPORT_JSON}.tmp"
  echo "::error::eol-line.sh: la escritura de estado falló (jq rc=$?) — rechazo nombrado, el estado y la branch quedan intactos." >&2
  exit 1
fi
mv "${SUPPORT_JSON}.tmp" "$SUPPORT_JSON"
echo "estado escrito: línea $MINOR -> EOL."

# The branch deletion is NOT automatic (see the header): the state is the
# source of truth, and the branch is its physical projection — deleting it is
# an explicit step of the runbook, announced here with the exact command, not
# a side effect of the rotation. Nothing in this script touches the remote.
# The runbook is only announced for a line that actually anchored a branch:
# a branchless declaration gets its own honest notice, never a command that
# cannot succeed.
echo "::notice::eol-line.sh: la línea $MINOR queda EOL."
if [[ -n "$DECLARED_BRANCH" ]]; then
  echo "::notice::eol-line.sh: la rama $DECLARED_BRANCH NO se borra automáticamente: la branch es la proyección física del estado, y su borrado es un paso explícito del runbook."
  echo "::notice::eol-line.sh: borrá la rama de soporte con: git push origin --delete $DECLARED_BRANCH"
  if git show-ref --verify --quiet "refs/heads/$DECLARED_BRANCH"; then
    echo "::notice::eol-line.sh: la branch local $DECLARED_BRANCH existe; después del borrado remoto se elimina local con: git branch -D $DECLARED_BRANCH"
  fi
else
  echo "::notice::eol-line.sh: la línea $MINOR no ancla ninguna rama (branch null): no hay borrado que anunciar en el runbook."
fi
echo "OK: línea $MINOR en EOL."
