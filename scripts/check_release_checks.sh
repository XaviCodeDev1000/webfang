#!/usr/bin/env bash
#
# Release check-runs gate — the release pipeline must consult the CI result of
# the commit it is about to publish, not only its own provenance.
#
# Subcommands: checks
#
# Exit 0 = every required context is present on the commit and its conclusion is
#           success -> proceed.
# Exit 1 = decided red: at least one required context is missing, failed, still
#           running, cancelled or otherwise not success. The single ::error::
#           line NAMES every offending context with the status observed.
# Exit 2 = cannot tell: a required input is missing, jq is absent, the API call
#           failed (auth, network, non-2xx) or the answer is unparsable. Never a
#           silent pass.
#
# WHY THIS EXISTS
#   check_release_provenance.sh preflight proves WHICH commit a tag points at
#   (L1.1) and `ruleset` proves the tag is immutable (L1.5). Neither answers
#   whether that commit ever went green: a tag can point at a perfect ancestor of
#   main and still carry a red `CI Gate`. Nothing in the pipeline asked the API
#   what the commit's check runs were, so a release could certify a build no gate
#   ever accepted. This is the same fail-closed shape as L1.5 — a non-2xx answer
#   blocks the release instead of passing it, and "cannot tell" has its own exit
#   code so it can never be mistaken for a green.
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
#                          fixture is a cannot-tell (exit 2), exactly like a
#                          failed API call.
set -euo pipefail

SUBCOMMAND="${1:-}"
[[ -n "$SUBCOMMAND" ]] || { echo "usage: $(basename "$0") {checks}" >&2; exit 2; }

# ============================================================================
# REQUIRED CONTEXTS — the ONE place this list lives.
#
# These are the contexts required on the PR that lands a commit, observed live on
# 2026-11-09. `docs/break-glass-enforce-admins.md` (audited 2026-09-29) lists
# only three — CI Gate, Validate PR metadata, cargo-mutants (PR diff) — and
# predates `breaking-surface` becoming required. The drift is recorded here on
# purpose: this gate asserts the LIVE set, and a context silently dropped from
# this array is a hole no other gate covers (the harness pins each of the four).
# ============================================================================
REQUIRED_CONTEXTS=(
  "CI Gate"
  "cargo-mutants (PR diff)"
  "Validate PR metadata"
  "breaking-surface"
)

# Normalizes one API page (or one fixture document) to TAB-separated
# name<TAB>status<TAB>conclusion lines. A document without a check_runs array
# makes jq fail, which becomes a cannot-tell.
CHECK_RUNS_JQ='if (.check_runs | type) == "array" then (.check_runs[] | [(.name // ""), (.status // ""), (.conclusion // "")] | @tsv) else error("no check_runs array in the answer") end'

reads_fixture() { [[ -n "${RELEASE_CHECKS_FIXTURE:-}" ]]; }

case "$SUBCOMMAND" in
  checks)
    GH_TOKEN="${GH_TOKEN:-}"
    GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-}"
    TARGET_COMMIT="${PROV_VALIDATED_COMMIT:-${GITHUB_SHA:-}}"

    command -v jq >/dev/null 2>&1 || {
      echo "::error::Release checks: jq is required to read the check-runs answer - cannot tell whether the required checks passed" >&2
      exit 2
    }

    [[ -n "$TARGET_COMMIT" ]] || {
      echo "::error::Release checks: no commit to verify - PROV_VALIDATED_COMMIT and GITHUB_SHA are both empty, cannot tell whether the required checks passed" >&2
      exit 2
    }

    runs=""
    if reads_fixture; then
      # Fixture mode: no API, no token. An unreadable or unparsable fixture is a
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
        echo "::error::Release checks: GITHUB_REPOSITORY is required to read the check runs - cannot tell whether the required checks passed" >&2
        exit 2
      }
      [[ -n "$GH_TOKEN" ]] || {
        echo "::error::Release checks: GH_TOKEN is required to read the check runs - cannot tell whether the required checks passed" >&2
        exit 2
      }
      rc=0
      # --paginate + --jq applies the filter per page and concatenates the TSV
      # lines, so a multi-page answer keeps one line per check run.
      runs="$(gh api --paginate --method GET \
        -H 'Accept: application/vnd.github+json' \
        --jq "$CHECK_RUNS_JQ" \
        "repos/$GITHUB_REPOSITORY/commits/$TARGET_COMMIT/check-runs?per_page=100")" || rc=$?
      [[ $rc -eq 0 ]] || {
        echo "::error::Release checks: cannot tell - the check-runs API request for commit $TARGET_COMMIT failed (auth, network or non-2xx); a release stays blocked until the answer is readable" >&2
        exit 2
      }
    fi

    offenders=""
    satisfied=0
    for context in "${REQUIRED_CONTEXTS[@]}"; do
      # Every check run carrying this context name, as status<TAB>conclusion.
      observed="$(printf '%s\n' "$runs" | awk -F'\t' -v want="$context" 'NF >= 1 && $1 == want { print $2 "\t" $3 }')"
      if [[ -z "$observed" ]]; then
        offenders="${offenders}${context} (missing: the commit carries no check run with this context); "
        continue
      fi
      # Fail closed: EVERY run with this name must be completed and successful.
      # A stale red next to a fresh green is not a green — the filter=latest
      # default gives one run per context, so this only fires on genuine doubt.
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
