#!/usr/bin/env bash
# Canonical vocabulary for a guard that cannot run, and for an exemption that
# has expired. Sourced, never executed.
#
# WHY THIS FILE EXISTS. Two failure modes kept the same shape in different
# words: a guard whose prerequisite was missing said nothing useful (or said it
# into a discarded stream), and an exemption kept passing after the fact it
# denied had become false. Both are now spelled in exactly one place, here. A
# printer copied into each guard drifts within months, and the meta-guard then
# recognises none of the variants — which is the same fail-open shape one level
# up, wearing a helpful message.
#
# CONTRACT (what a caller may rely on):
#   prereq_require <what> <remedy>
#       prints  prerequisite-missing: <what> — run <remedy>   and exits 1
#       Use it when the environment is unfinished, not when the code is wrong.
#   stale_exemption <key> <what-no-longer-holds>
#       prints  stale-exemption: <key> — <what-no-longer-holds>  and returns 1
#       Use it when an allowlist entry's own condition has become false.
#
# WHY `prerequisite-missing` CAN NEVER BE BASELINED. A missing prerequisite is
# an unfinished environment: the guard did not decide anything, so recording it
# as a tolerated defect would buy immunity from the only check that matters.
# scripts/test_guard_coverage.sh enforces that by never listing it as a
# baselinable category, and it reads the token from here rather than spelling it
# again.

GUARD_PREREQ_TOKEN="prerequisite-missing"
GUARD_STALE_EXEMPTION_TOKEN="stale-exemption"

# Single source, queried by the meta-guard so the two cannot drift apart.
guard_prereq_token() { printf '%s\n' "$GUARD_PREREQ_TOKEN"; }
guard_stale_exemption_token() { printf '%s\n' "$GUARD_STALE_EXEMPTION_TOKEN"; }

prereq_require() { # what remedy
  local what="${1:-}" remedy="${2:-}"
  printf '%s: %s — run %s\n' "$GUARD_PREREQ_TOKEN" "${what:-unnamed prerequisite}" \
    "${remedy:-the documented bootstrap step}" >&2
  exit 1
}

stale_exemption() { # key what-no-longer-holds
  local key="${1:-}" what="${2:-}"
  printf '%s: %s — %s\n' "$GUARD_STALE_EXEMPTION_TOKEN" "${key:-unnamed exemption}" \
    "${what:-its condition no longer holds}" >&2
  return 1
}
