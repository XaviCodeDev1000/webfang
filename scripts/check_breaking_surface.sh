#!/usr/bin/env bash
# check_breaking_surface.sh — the breaking-change invariant as a CI check (#2328).
#
# WHY THIS EXISTS. The local guard in merge-when-green.sh (#2320/#2327) has two
# gaps it cannot close by itself:
#
#   1. TOCTOU: the script re-validates at the merge boundary, but between that
#      read and the enqueue/merge a title edit or a push can still slip a
#      marker off the kept surface.
#   2. COVERAGE: it only runs when the merge goes through the script. A merge
#      from the UI, plain `gh pr merge`, or any other tool bypasses it.
#
# A required check closes both: GitHub evaluates it against the PR's real head
# (`pull_request`) and against the queue's synthetic entry (`merge_group`), and
# re-runs it on every push or title change. This script is the job's body; the
# DECISION (the surface matrix) comes from scripts/breaking_surface_lib.sh —
# the same library the local guard consumes, so the two cannot drift.
#
# Fail-closed contract (never green by default):
#   - the PR number cannot be resolved (the workflow step exits before us) or
#     the PR metadata cannot be read / is incomplete / carries zero commits;
#   - the base branch's merge-queue method is not available in the tracked
#     mirror (.github/merge-queue.json) or is an unknown method — the method
#     decides WHICH surface is kept, so guessing one would judge the wrong
#     surface. The mirror exists because GITHUB_TOKEN cannot read the live
#     queue configuration (GraphQL answers FORBIDDEN on
#     Repository.mergeQueue.configuration for integrations, #2412);
#   - the commit list cannot be read (REST, paginated).
#
# Commit pagination: `gh pr view --json commits` truncates the GraphQL
# connection (a marker on commit 101 would be invisible). This script reads
# `repos/<repo>/pulls/<n>/commits` with `--paginate`, which returns every
# commit, and derives headline/body from the full message.
#
# Usage:
#   PR_NUMBER=<n> REPO=<owner/repo> GH_TOKEN=<t> bash scripts/check_breaking_surface.sh
#       production mode: read everything from the GitHub API and judge
#   bash scripts/check_breaking_surface.sh --fixture <file> --strategy squash|merge
#       offline mode: judge an assembled meta file
#       ({title, headRefOid, commits: [{oid, messageHeadline, messageBody}]});
#       used by scripts/test_breaking_surface.sh
set -euo pipefail

# shellcheck source=scripts/breaking_surface_lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/breaking_surface_lib.sh"

strategy=""
fixture_file=""

usage() {
  cat <<'EOF'
Usage:
  PR_NUMBER=<n> REPO=<owner/repo> GH_TOKEN=<t> bash scripts/check_breaking_surface.sh
  bash scripts/check_breaking_surface.sh --fixture <file> --strategy squash|merge

The breaking-change invariant as a required CI check (#2328): the marker must
live on the surface the merge strategy keeps. The strategy comes from the
tracked mirror of the queue configuration (.github/merge-queue.json), never
from a flag. Fail closed on unreadable/incomplete metadata, zero commits, or
a method absent/unknown in the mirror. See the header comment for the full
contract.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fixture)
      [[ -n "${2:-}" ]] || { echo "--fixture requires a file" >&2; exit 2; }
      fixture_file="$2"
      shift 2
      ;;
    --strategy)
      strategy="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

die() { # die <message> — a required check fails LOUD, never green by default.
  printf '::error::%s\n' "$1" >&2
  exit 1
}

# judge <meta-json> <strategy> — shape-validate, count, decide, report.
# Exit 0 only on "ok" (or a clean PR); any other outcome is a red step.
judge() {
  local meta="$1" strategy="$2"
  local pr_title head_oid commits_json title_kind verdict
  local n_commits n_canonical n_malformed n_footer

  # The same shape contract as read_breaking_meta in merge-when-green.sh: a
  # non-empty but INCOMPLETE payload is not evidence that the marker is
  # absent. Zero commits have no effective subject to judge — fail closed.
  if [[ "$(printf '%s' "$meta" | jq -e '
      (.title | type == "string")
      and (.headRefOid | type == "string")
      and (.commits | type == "array" and length >= 1)
      and all(.commits[];
        ((.oid // null) | type == "string")
        and ((.messageHeadline // null) | type == "string")
        and ((.messageBody // null) | type == "string"))
    ' 2>/dev/null || true)" != "true" ]]; then
    die "No se pudo leer metadata completa del PR (title/headRefOid/commits). Un check requerido no avanza con datos incompletos (fail closed)."
  fi

  pr_title="$(printf '%s' "$meta" | jq -r '.title // ""')"
  head_oid="$(printf '%s' "$meta" | jq -r '.headRefOid // ""')"
  commits_json="$(printf '%s' "$meta" | jq -c '.commits // []')"
  title_kind="$(subject_kind "$pr_title")"

  read -r n_commits n_canonical n_malformed n_footer < <(breaking_count_commits "$commits_json")
  verdict="$(breaking_verdict "$strategy" "$n_commits" "$title_kind" \
    "$n_canonical" "$n_malformed" "$n_footer")"

  if [[ "$verdict" == "ok" ]]; then
    echo "OK: breaking-surface (estrategia=${strategy}, commits=${n_commits}, head=${head_oid:0:12})"
    return 0
  fi

  # Evidence: the commits the classification flagged (same rows the local
  # guard prints), so the log shows exactly which subjects are in play.
  # shellcheck disable=SC2016  # $canon/$footer are jq variables passed with --arg
  local rows
  rows="$(printf '%s' "$commits_json" | jq -r \
    --arg canon "$canonical_subject_re" --arg footer "$footer_re" '
      def header: (. // "") | split(" ")[0];
      def canonical: header | test($canon);
      def malformed: (header | test($canon) | not) and (header | contains("!"));
      def footer: (. // "") | test($footer);
      .[]?
      | select(
          ((.messageHeadline // "") | canonical)
          or ((.messageHeadline // "") | malformed)
          or ((.messageBody // "") | footer)
        )
      | "::error::  commit \(.oid[0:8]): \(.messageHeadline)\(if ((.messageBody // "") | footer) then " [footer]" else "" end)"
    ')"

  case "$verdict" in
    commit-malformed)
      printf '::error::El subject de un commit lleva un marcador de ruptura malformado (type!(scope):).\n' >&2
      ;;
    commit-footer)
      printf "::error::Un commit declara la ruptura solo con un footer 'BREAKING CHANGE:'.\n" >&2
      ;;
    title-malformed-n1)
      printf '::error::El titulo del PR tiene un marcador malformado y el squash de 1 commit conserva el SUBJECT del commit, no el titulo.\n' >&2
      ;;
    title-only-n1)
      printf '::error::El PR declara la ruptura solo en el titulo, pero el squash de 1 commit conserva el SUBJECT del commit; el titulo se descarta.\n' >&2
      ;;
    title-malformed-n2)
      printf '::error::El PR tiene %s commits y el squash conserva el TITULO, cuyo marcador es malformado.\n' "$n_commits" >&2
      ;;
    commit-only-squash)
      printf '::error::El PR tiene %s commits y el squash conserva el TITULO, pero el marcador vive solo en los commits.\n' "$n_commits" >&2
      ;;
    title-only-merge)
      printf '::error::El unico marcador de ruptura esta en el titulo del PR, que un merge commit DESCARTA.\n' >&2
      ;;
    *)
      die "Veredicto desconocido '${verdict}' de breaking_surface_lib.sh (drift librería/consumidor)."
      ;;
  esac
  if [[ -n "$rows" ]]; then
    printf '%s\n' "$rows" >&2
  fi
  printf "::error::Coloca 'type!:' o 'type(scope)!:' en la superficie que la estrategia conserva (titulo del PR para squash con 2+ commits; subject de commit para squash con 1 commit o para merge). 'type!(scope):' no es parseado por release-plz y un footer alone no es aceptado.\n" >&2
  return 1
}

if [[ -n "$fixture_file" ]]; then
  [[ -r "$fixture_file" ]] || die "Fixture no legible: ${fixture_file}"
  case "$strategy" in
    squash|merge) ;;
    *) die "--strategy requiere squash|merge en modo fixture." ;;
  esac
  # A violation inside judge exits 1 on its own (die / set -e); reaching this
  # line means the verdict was "ok", and the offline mode ends here.
  judge "$(cat "$fixture_file")" "$strategy"
  exit 0
fi

# --- production mode ---------------------------------------------------------
case "${PR_NUMBER:-}" in
  ''|*[!0-9]*) die "PR_NUMBER debe ser un numero de PR (evento pull_request o merge_group resuelto). Recibido: '${PR_NUMBER:-}'." ;;
esac
case "${REPO:-}" in
  */*) ;;
  *) die "REPO debe ser owner/name. Recibido: '${REPO:-}'." ;;
esac

meta="$(gh pr view "$PR_NUMBER" --repo "$REPO" --json title,headRefOid,baseRefName 2>/dev/null)" \
  || die "No se pudo leer title/headRefOid/baseRefName del PR #${PR_NUMBER} (gh/API)."

base="$(printf '%s' "$meta" | jq -r '.baseRefName // ""')"
[[ -n "$base" ]] || die "El PR #${PR_NUMBER} no reporta baseRefName."

# --- queue merge method (tracked mirror) -------------------------------------
# GITHUB_TOKEN cannot read the live queue configuration: GraphQL answers
# FORBIDDEN ("Resource not accessible by integration") on
# Repository.mergeQueue.configuration for integrations (#2412). The method is
# therefore mirrored in .github/merge-queue.json — tracked, auditable — and
# scripts/merge-when-green.sh cross-checks the mirror against the LIVE
# configuration at the merge boundary; a divergence refuses the merge there.
mirror_file="${BREAKING_SURFACE_QUEUE_FILE:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../.github/merge-queue.json}"
queue_method="$(breaking_queue_method_from_mirror "$mirror_file" "$base")"
case "$queue_method" in
  SQUASH) strategy="squash" ;;
  MERGE|REBASE) strategy="merge" ;;
  *)
    die "El metodo de merge de la cola de '${base}' no esta disponible en el espejo '${mirror_file}' (archivo ilegible, entrada ausente para la base, o metodo desconocido: '${queue_method:-}'). Sin metodo conocido no se puede juzgar la superficie (fail closed)."
    ;;
esac

# --- commits, paginated ------------------------------------------------------
# REST + --paginate: `gh pr view --json commits` truncates at 100 and a marker
# on commit 101 would be invisible. Headline/body derive from the full commit
# message with the same semantics the GraphQL fields have.
# shellcheck disable=SC2016  # single-quoted jq program
commits_jsonl="$(gh api --paginate "repos/$REPO/pulls/$PR_NUMBER/commits?per_page=100" \
  --jq 'def m: ((.commit.message // "") | split("\n"));
        .[] | {oid: .sha,
               messageHeadline: (m[0] // ""),
               messageBody: ((m[1:]) | join("\n"))}' 2>/dev/null)" \
  || die "No se pudo leer la lista de commits del PR #${PR_NUMBER} (REST paginada)."
[[ -n "$commits_jsonl" ]] || die "La lista de commits del PR #${PR_NUMBER} vino vacía (fail closed)."

meta_full="$(printf '%s' "$commits_jsonl" | jq -s --argjson head "$meta" '{title: $head.title, headRefOid: $head.headRefOid, commits: .}')"

judge "$meta_full" "$strategy"
