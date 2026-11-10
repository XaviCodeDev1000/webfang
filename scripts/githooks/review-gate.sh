#!/usr/bin/env bash
# Gentle AI review delivery gate (ADR: #1036, wired 2026-08-30).
#
# `gentle-ai review validate` and the named gates are compatibility/informational
# in gentle-ai 4.x: `gentle-ai review validate --help` states "gates do not read
# authority", and the documented results are `invalidated/unmanaged` with RDD
# enabled and `disabled/unmanaged` with RDD disabled. They never decide delivery;
# delivery follows ordinary repository policy.
#
# This hook reports the verdict but never blocks on the absence of a usable one.
# Decision matrix:
#
#   - `gentle-ai` binary absent -> SKIP with a warning (fail-open): a machine
#     without the tool cannot gate anything, and blocking would brick the repo.
#   - validate exits non-zero (including the 20s timeout) or emits unparseable
#     output -> SKIP with a warning (fail-open), matching main-push-gate.sh: a
#     broken or hung tool must never become the reason delivery is impossible.
#   - `delivery` is `unmanaged` or `disabled/unmanaged` -> ALLOW: no review
#     authority governs this candidate, so ordinary repository policy applies.
#     This is the normal path for every candidate that never started a review.
#   - `allowed == true` -> ALLOW.
#   - otherwise -> BLOCK: a parseable verdict explicitly governs this candidate
#     and denies delivery. gentle-ai 4.x gates do not produce this state; the
#     branch stays as defense-in-depth for other versions.
#
# Gate context: `gentle-ai review start` cannot run from a plain shell
# (`immutable_review_transport_unsupported` — the relay contract is host-only),
# so this hook only *reports* a compatibility verdict; it never produces one.
# Reviews run through the host's review lifecycle, not through git hooks.

set -euo pipefail

GATE="${1:?usage: review-gate.sh <pre-commit|pre-push>}"
REPO_ROOT="$(git rev-parse --show-toplevel)"

if ! command -v gentle-ai >/dev/null 2>&1; then
    echo "[review-gate] SKIP: 'gentle-ai' not on PATH — delivery gate cannot report a review verdict." >&2
    exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "[review-gate] NOTE: 'jq' not on PATH — using the built-in scalar fallback parser." >&2
fi

# Bound the consult: a hung validate must not halt delivery indefinitely (#1048).
TIMEOUT_CMD=()
command -v timeout >/dev/null 2>&1 && TIMEOUT_CMD=(timeout 20s)

OUT="$(${TIMEOUT_CMD[@]+"${TIMEOUT_CMD[@]}"} gentle-ai review validate --gate "$GATE" --cwd "$REPO_ROOT" 2>/dev/null)" || {
    code=$?
    echo "[review-gate] SKIP: 'gentle-ai review validate' failed (exit ${code}) — cannot verify delivery gate; fail-open." >&2
    exit 0
}

# Extract a flat scalar field from the verdict: jq when available, sed
# otherwise, so a missing jq cannot disable delivery (#1048 R4-001).
gate_field() {
    local key="$1"
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$OUT" \
            | jq -r --arg k "$key" \
                'if has($k) and (.[$k] != null) then .[$k] | tostring else empty end' 2>/dev/null
    else
        printf '%s' "$OUT" \
            | sed -n "s/.*\"$key\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",}]*\)\"\{0,1\}.*/\1/p" | head -1
    fi
}

# Best-effort extraction: a jq parse failure must not abort the hook via
# `set -e`/`pipefail` before the fail-open branch below can run.
DELIVERY="$(gate_field delivery)" || true
ALLOWED="$(gate_field allowed)" || true

if [[ -z "$DELIVERY" ]]; then
    echo "[review-gate] SKIP: unparseable gate verdict — cannot verify delivery gate; fail-open." >&2
    exit 0
fi

if [[ "$DELIVERY" == "unmanaged" || "$DELIVERY" == "disabled/unmanaged" ]]; then
    echo "[review-gate] OK: no review authority governs this candidate; ordinary repository policy applies."
    exit 0
fi

if [[ "$ALLOWED" == "true" ]]; then
    echo "[review-gate] OK: review authority allows this delivery."
    exit 0
fi

echo "[review-gate] BLOCK: a review verdict governs this candidate but does not allow delivery (allowed=${ALLOWED:-<none>}, delivery=${DELIVERY:-<none>})." >&2
echo "[review-gate] Complete or acknowledge the review, or inspect the verdict with 'gentle-ai review validate --gate $GATE --cwd \"$REPO_ROOT\"'." >&2
exit 1
