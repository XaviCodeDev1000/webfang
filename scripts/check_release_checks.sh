#!/usr/bin/env bash
#
# Release check-runs gate — the release pipeline must consult the CI result of
# the commit it is about to publish, not only its own provenance.
#
# Subcommands: checks
#
# Exit 0 = every required context is present on the commit and its conclusion is
#           success -> proceed. A non-required check run that is absent or not
#           successful is printed as an informational ::warning:: that names the
#           context and its conclusion (or says it is absent); it never fails.
# Exit 1 = decided red: at least one required context is missing, failed, still
#           running, cancelled or otherwise not success. The single ::error::
#           line names every offending context with the status observed.
# Exit 2 = cannot tell: a required input is missing, jq is absent, the API call
#           failed (auth, network, non-2xx) or the answer is unparsable. Never a
#           silent pass. Every exit-2 message that a missing permission or a bad
#           token can produce carries its REMEDY on the SAME line — which
#           permission, in which file, and how to retry — because the only
#           reader is an operator at 3am holding one log line.
#
# WHY THIS EXISTS
#   check_release_provenance.sh preflight proves WHICH commit a tag points at
#   (L1.1) and `ruleset` proves the tag is immutable (L1.5). Neither answers
#   whether that commit ever went green: a tag can point at a perfect ancestor of
#   main and still carry a red `CI Gate`, so a release could certify a build no
#   gate ever accepted. Same fail-closed shape as L1.5 — a non-2xx answer blocks
#   instead of passing, and "cannot tell" has its own exit code.
#
# INPUTS (environment, so a harness can drive them without GitHub)
#   GH_TOKEN           token for the API; required for the API read.
#   GITHUB_REPOSITORY  owner/repo; required for the API read.
#   PROV_VALIDATED_COMMIT  commit to verify when preflight exposes it, else
#                          GITHUB_SHA.
#   RELEASE_CHECKS_FIXTURE  TEST HOOK: when set, the answer is read from this
#                          JSON file (same shape as the API response:
#                          {"check_runs":[{"name","status","conclusion"}...]})
#                          instead of calling the API. No token is needed in
#                          this mode, which is what makes the decision table
#                          testable offline. An unreadable or unparsable
#                          fixture is a cannot-tell (exit 2).
set -euo pipefail

SUBCOMMAND="${1:-}"
[[ -n "$SUBCOMMAND" ]] || { echo "usage: $(basename "$0") {checks}" >&2; exit 2; }

# REQUIRED CONTEXTS — the ONE place this list lives. ONE context: `CI Gate`.
#
# WHY THE FOUR PR CONTEXTS CANNOT BE ASSERTED HERE. A release tag points at a
# SQUASH MERGE commit on main. That commit carries the aggregate `CI Gate` run
# (ci.yml runs on push to main) but NOT the PR-scoped contexts:
# `Validate PR metadata` (pr-validation.yml) and `cargo-mutants (PR diff)`
# (mutants.yml, pull_request/merge_group only) attach to the PR HEAD SHA that
# the squash replaced — a SHA that is not an ancestor of main and is never the
# commit a tag publishes. `breaking-surface` is the same class. Requiring all
# four on a tag commit would block EVERY release, not only a bad one: a gate
# that no correct release can satisfy is an outage wearing a gate's clothes.
#
# WHAT IS STILL PROVEN. Those four decisions were made where they belong, on
# the PR that reached main, and `CI Gate` is not green on the push-to-main run
# unless every job ci.yml fans out to succeeded there. So this gate asserts the
# one fact a tag adds: the commit being published was accepted by the
# aggregate gate. Every OTHER non-success check run on the commit is named as
# an informational ::warning:: (context + conclusion, or the fact that it is
# absent) and blocks nothing — a context nobody requires must not become a
# release blocker by silence, and must not be hidden by it either.
#
# COUPLED TO THE MERGE METHOD, NOT TO THIS WORKFLOW. The required set depends on
# the merge method declared in `.github/merge-queue.json` (the same tracked
# mirror `scripts/breaking_surface_lib.sh` reads). Today the queue declares
# SQUASH, so the branch commits do not survive to main and the PR contexts
# cannot appear on the tag commit. If the repository switches to MERGE, the
# branch commits reach main with their check runs intact and this set changes —
# whoever flips the method must re-read this comment, not assume it still holds.
#
# `docs/break-glass-enforce-admins.md` (audited 2026-09-29) lists three of the
# PR-level contexts and predates `breaking-surface`, which is why they are named
# above: the drift is recorded on purpose rather than trusted from either source.
REQUIRED_CONTEXTS=(
  "CI Gate"
)

# The PR-scoped contexts a SQUASH merge leaves behind on the PR head: not
# required (see above), but their absence or failure is NAMED below.
PR_SCOPED_CONTEXTS=(
  "Validate PR metadata"
  "cargo-mutants (PR diff)"
  "breaking-surface"
)

is_required_context() { # name
  local c
  for c in "${REQUIRED_CONTEXTS[@]}"; do [[ "$c" == "$1" ]] && return 0; done
  return 1
}

is_pr_scoped_context() { # name
  local c
  for c in "${PR_SCOPED_CONTEXTS[@]}"; do [[ "$c" == "$1" ]] && return 0; done
  return 1
}

# Normalizes one API page (or fixture) to name<TAB>status<TAB>conclusion lines;
# a document without a check_runs array makes jq fail -> cannot-tell.
CHECK_RUNS_JQ='if (.check_runs | type) == "array" then (.check_runs[] | [(.name // ""), (.status // ""), (.conclusion // "")] | @tsv) else error("no check_runs array in the answer") end'

reads_fixture() { [[ -n "${RELEASE_CHECKS_FIXTURE:-}" ]]; }

case "$SUBCOMMAND" in
  checks)
    GH_TOKEN="${GH_TOKEN:-}"
    GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-}"
    TARGET_COMMIT="${PROV_VALIDATED_COMMIT:-${GITHUB_SHA:-}}"

    command -v jq >/dev/null 2>&1 || {
      echo "::error::Release checks: cannot read the check-runs answer (jq is missing). Remedy: install jq on the runner (ubuntu-latest ships it), then re-run the release workflow for this tag." >&2
      exit 2
    }

    [[ -n "$TARGET_COMMIT" ]] || {
      echo "::error::Release checks: cannot tell which commit to verify (PROV_VALIDATED_COMMIT and GITHUB_SHA are both empty). Remedy: run the release workflow from .github/workflows/release.yml ('gh workflow run release.yml -f tag=<tag>'), which exports PROV_VALIDATED_COMMIT from the provenance preflight step, instead of invoking this script by hand." >&2
      exit 2
    }

    runs=""
    if reads_fixture; then
      # Fixture mode: no API, no token; unreadable or unparsable is a
      # cannot-tell, never a green.
      [[ -r "$RELEASE_CHECKS_FIXTURE" ]] || {
        echo "::error::Release checks: cannot tell - the answer fixture '$RELEASE_CHECKS_FIXTURE' is missing or unreadable" >&2
        exit 2
      }
      rc=0
      runs="$(jq -r "$CHECK_RUNS_JQ" "$RELEASE_CHECKS_FIXTURE")" || rc=$?
      [[ $rc -eq 0 ]] || {
        echo "::error::Release checks: cannot tell - the answer fixture '$RELEASE_CHECKS_FIXTURE' is unparsable (no check_runs array)" >&2
        exit 2
      }
    else
      [[ -n "$GITHUB_REPOSITORY" ]] || {
        echo "::error::Release checks: cannot read check-runs for $TARGET_COMMIT (GITHUB_REPOSITORY is unset). Remedy: restore 'GITHUB_REPOSITORY: \${{ github.repository }}' in the env of the release-checks step in the preflight job of .github/workflows/release.yml, then re-run the release workflow for this tag." >&2
        exit 2
      }
      [[ -n "$GH_TOKEN" ]] || {
        echo "::error::Release checks: cannot read check-runs for $TARGET_COMMIT (GH_TOKEN is unset). Remedy: declare 'checks: read' in the permissions block of the preflight job in .github/workflows/release.yml and pass 'GH_TOKEN: \${{ github.token }}' in the step env, then re-run the release workflow for this tag." >&2
        exit 2
      }
      rc=0
      # --paginate + --jq applies the filter per page, so a multi-page answer
      # keeps one line per check run. stderr is folded into the same capture: on
      # failure it holds gh's diagnosis, the ONLY way to tell a missing
      # permission (403) from an unknown commit (404) and print the remedy.
      api_answer=""
      api_answer="$(gh api --paginate --method GET \
        -H 'Accept: application/vnd.github+json' \
        --jq "$CHECK_RUNS_JQ" \
        "repos/$GITHUB_REPOSITORY/commits/$TARGET_COMMIT/check-runs?per_page=100" 2>&1)" || rc=$?
      if [[ $rc -ne 0 ]]; then
        case "$api_answer" in
          *"HTTP 403"*)
            echo "::error::Release checks: cannot read check-runs for $TARGET_COMMIT (HTTP 403: the token is not allowed to read check runs). Remedy: declare 'checks: read' in the permissions block of the preflight job in .github/workflows/release.yml, then re-run the release workflow for this tag." >&2
            ;;
           *"HTTP 404"*)
            echo "::error::Release checks: cannot read check-runs for $TARGET_COMMIT (HTTP 404: the commit is unknown to this repository or carries no check runs). Remedy: confirm the tag points at a commit on main and that GITHUB_REPOSITORY names this repository, then re-run the release workflow for this tag." >&2
            ;;
          *)
            echo "::error::Release checks: cannot read check-runs for $TARGET_COMMIT (auth, network or another non-2xx answer from the API). Remedy: verify GH_TOKEN is \${{ github.token }} with 'checks: read' in .github/workflows/release.yml and that the runner reaches api.github.com, then re-run the release workflow for this tag." >&2
            ;;
        esac
        exit 2
      fi
      runs="$api_answer"
    fi

    offenders=""
    satisfied=0
    for context in "${REQUIRED_CONTEXTS[@]}"; do
      observed="$(printf '%s\n' "$runs" | awk -F'\t' -v want="$context" 'NF >= 1 && $1 == want { print $2 "\t" $3 }')"
      if [[ -z "$observed" ]]; then
        offenders="${offenders}${context} (missing: the commit carries no check run with this context); "
        continue
      fi
      # Fail closed: EVERY run with this name must be completed and successful —
      # a stale red next to a fresh green is not a green.
      verdicts="$(printf '%s\n' "$observed" | awk -F'\t' '$1 != "completed" || $2 != "success" {
        status = ($1 == "" ? "unknown-status" : $1)
        conclusion = ($2 == "" ? "no-conclusion" : $2)
        printf "%s/%s ", status, conclusion
      }')"
      if [[ -n "$verdicts" ]]; then
        offenders="${offenders}${context} (observed ${verdicts% }); "
      else
        satisfied=$((satisfied + 1))
      fi
    done

    # Informational warnings, printed BEFORE the verdict so a red release still
    # states every other context's state — named, never "other checks failed".
    for context in "${PR_SCOPED_CONTEXTS[@]}"; do
      observed="$(printf '%s\n' "$runs" | awk -F'\t' -v want="$context" 'NF >= 1 && $1 == want { print $2 "\t" $3 }')"
      if [[ -z "$observed" ]]; then
        # Absence is the EXPECTED squash-merge shape for the PR-scoped set.
        echo "::warning::Release checks: $context: no such check-run on $TARGET_COMMIT (expected for a squash merge — this context attaches to the PR head that the squash replaced)" >&2
        continue
      fi
      while IFS=$'\t' read -r wstatus wconclusion; do
        [[ -n "${wstatus:-}${wconclusion:-}" ]] || continue
        [[ "$wstatus" == "completed" && "$wconclusion" == "success" ]] && continue
        echo "::warning::Release checks: $context: conclusion=${wconclusion:-none}, status=${wstatus:-unknown} (informational — not in the required set)" >&2
      done <<<"$observed"
    done
    # Any OTHER non-success run (an advisory job nobody listed is still named).
    while IFS=$'\t' read -r oname ostatus oconclusion; do
      [[ -n "${oname:-}" ]] || continue
      is_required_context "$oname" && continue
      is_pr_scoped_context "$oname" && continue
      [[ "$ostatus" == "completed" && "$oconclusion" == "success" ]] && continue
      echo "::warning::Release checks: $oname: conclusion=${oconclusion:-none}, status=${ostatus:-unknown} (informational — not in the required set)" >&2
    done <<<"$(printf '%s\n' "$runs" | awk -F'\t' 'NF >= 3')"

    if [[ -n "$offenders" ]]; then
      echo "::error::Release checks: ${#REQUIRED_CONTEXTS[@]} required context(s) must be present and successful on commit $TARGET_COMMIT; ${satisfied} satisfied, offending: ${offenders%; }" >&2
      exit 1
    fi

    echo "Release checks: all ${#REQUIRED_CONTEXTS[@]} required contexts are present and successful on $TARGET_COMMIT (${REQUIRED_CONTEXTS[*]})"
    ;;
  *)
    echo "::error::unknown subcommand '$SUBCOMMAND'" >&2
    exit 2
    ;;
esac
