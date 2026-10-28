#!/usr/bin/env bash
# breaking_surface_lib.sh — the breaking-marker surface matrix, in ONE place.
#
# #2328. Two consumers apply the same invariant and must never drift:
#
#   - scripts/merge-when-green.sh   (local defense in depth, #2320/#2327)
#   - scripts/check_breaking_surface.sh (the required CI check, `breaking-surface`)
#
# The matrix (measured in #2321, motivated by #2078/#2081): GitHub's squash
# subject depends on the commit count — 1 commit keeps the COMMIT subject, 2+
# keep the PR title. A merge commit keeps every branch commit and DISCARDS the
# PR title. A breaking marker is canonical only as `type!:` or `type(scope)!:`
# in a subject; it must live on the surface the merge strategy keeps.
#
# This library exports the pure parts only — the regexes, the subject
# classifier, the commit counter and the verdict function. It prints no errors
# and exits on nothing: the callers own their messaging and their exit codes
# (the script refuses with exit 5/4; the CI check turns a violation into a red
# step). Message drift between the two consumers is therefore impossible for
# the DECISION, which is the part that must never diverge.
# shellcheck shell=bash

# Canonical marker: `type!:` or `type(scope)!:`. `type!(scope):` is malformed:
# it expresses the intent but release-plz does not parse it. A `!` outside the
# header (prose) is not a marker. A `BREAKING CHANGE:` footer alone is not
# accepted: the gate requires the `!`.
canonical_subject_re='^[A-Za-z]+(\([^)]*\))?!:'
footer_re='(?im)^BREAKING[ -]CHANGE:'

# subject_kind <subject> -> canonical | malformed | none
subject_kind() {
  local subject="$1" header="${1%% *}"
  if [[ "$header" =~ $canonical_subject_re ]]; then
    printf 'canonical'
  elif [[ "$header" == *'!'* ]]; then
    printf 'malformed'
  else
    printf 'none'
  fi
}

# breaking_count_commits <commits-json> ->
#   "<n_commits>\t<n_canonical>\t<n_malformed>\t<n_footer>"
#
# commits-json is the array the consumers assemble, each element carrying
# `messageHeadline` and `messageBody` (the fields the classification reads).
# Callers shape-validate BEFORE counting: a zero-commit or malformed array is
# a fail-closed case at their boundary, never a silent "2+ commits" default.
breaking_count_commits() {
  local commits_json="$1"
  # shellcheck disable=SC2016  # $canon/$footer are jq variables passed with --arg
  printf '%s' "$commits_json" | jq -r \
    --arg canon "$canonical_subject_re" --arg footer "$footer_re" '
      def header: (. // "") | split(" ")[0];
      def canonical: header | test($canon);
      def malformed: (header | test($canon) | not) and (header | contains("!"));
      def footer: (. // "") | test($footer);
      (. // []) as $c
      | [
          ($c | length),
          ([$c[] | select((.messageHeadline // "") | canonical)] | length),
          ([$c[] | select((.messageHeadline // "") | malformed)] | length),
          ([$c[] | select((.messageBody // "") | footer)] | length)
        ] | @tsv
    '
}

# breaking_verdict <strategy> <n_commits> <title_kind> <n_canonical> <n_malformed> <n_footer>
#
# Prints exactly one verdict token and exits 0:
#
#   ok                   the kept surface carries a canonical marker, or
#                        nothing declares a break at all
#   commit-malformed     a kept commit subject has a `!` but is malformed
#                        (`type!(scope):`); refused under every strategy
#   commit-footer        a kept commit declares the break only with a
#                        `BREAKING CHANGE:` footer; refused under every strategy
#   title-malformed-n1   squash N=1: malformed title marker (title is dropped)
#   title-only-n1        squash N=1: the `!` lives only in the title
#                        (#2078's shape; the title is the dropped surface)
#   title-malformed-n2   squash N=2+: malformed title marker (title is kept)
#   commit-only-squash   squash N=2+: the `!` lives only in the commits, which
#                        the squash collapses into the title
#   title-only-merge     merge/rebase: the `!` lives only in the title, which a
#                        merge commit discards
#
# The verdict order IS the priority: a canonical marker on the kept surface
# wins, then the loud malformations, then the lost-surface cases. Both
# consumers must map every token; an unmapped token is a bug in the consumer,
# not a pass.
breaking_verdict() {
  local strategy="$1" n_commits="$2" title_kind="$3"
  local n_canonical="$4" n_malformed="$5" n_footer="$6"

  if [[ "$strategy" == "squash" ]]; then
    if (( n_commits == 1 )); then
      if (( n_canonical > 0 )); then
        printf 'ok'; return 0
      fi
      if (( n_malformed > 0 )); then
        printf 'commit-malformed'; return 0
      fi
      if (( n_footer > 0 )); then
        printf 'commit-footer'; return 0
      fi
      if [[ "$title_kind" == "malformed" ]]; then
        printf 'title-malformed-n1'; return 0
      fi
      if [[ "$title_kind" != "none" ]]; then
        printf 'title-only-n1'; return 0
      fi
      printf 'ok'; return 0
    fi
    # 2+ commits: the PR title is the squash subject.
    if [[ "$title_kind" == "canonical" ]]; then
      printf 'ok'; return 0
    fi
    if [[ "$title_kind" == "malformed" ]]; then
      printf 'title-malformed-n2'; return 0
    fi
    if (( n_canonical > 0 || n_malformed > 0 || n_footer > 0 )); then
      printf 'commit-only-squash'; return 0
    fi
    printf 'ok'; return 0
  fi

  # MERGE (and a REBASE queue, whose commits also survive): the branch commits
  # are kept and the PR title is discarded.
  if (( n_canonical > 0 )); then
    printf 'ok'; return 0
  fi
  if (( n_malformed > 0 )); then
    printf 'commit-malformed'; return 0
  fi
  if (( n_footer > 0 )); then
    printf 'commit-footer'; return 0
  fi
  if [[ "$title_kind" != "none" ]]; then
    printf 'title-only-merge'; return 0
  fi
  printf 'ok'; return 0
}

# breaking_queue_method_from_mirror <mirror-file> <base> -> method (or empty)
#
# Reads the tracked mirror of the queue configuration (.github/merge-queue.json)
# and prints the method recorded for <base>. Prints NOTHING when the file is
# unreadable, invalid, or has no entry for <base>: the caller decides
# fail-closed. Two consumers share this reader (the CI check and the local
# guard) so they cannot drift; more than one entry for the same base prints
# every match, which no caller's method case accepts — a duplicated base is
# therefore a fail-closed outcome too, not a silent pick.
breaking_queue_method_from_mirror() {
  local mirror_file="$1" base="$2"
  jq -r --arg base "$base" '
    (.queues // [])[]? | select((.base // "") == $base) | (.method // "")
  ' "$mirror_file" 2>/dev/null || true
}
