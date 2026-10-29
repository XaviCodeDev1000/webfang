#!/usr/bin/env bash
#
# Semantics harness for the publish gate's provenance (webfang#2479).
#
# The defect this pins cannot be proven by grepping the workflow: the publish
# job checks out the VALIDATED (tagged) commit, so on a `workflow_dispatch` the
# workflow file is current while every `scripts/` file it executes is the tag's.
# The tag's verifier then decides whether the tag may publish — the older the
# tag, the weaker the gate, on the one path that exists to recover an old tag.
# v7.0.0 was published that way: 15/15 verified by the pre-#2464 verifier while
# the workflow around it declared the fix.
#
# check_release_dispatch.sh check 18 asserts the gate comes from the run's own
# ref. This harness asserts that check 18 (and the check 8 refinement that keeps
# the gate checkout from becoming a second build input) actually REJECT the
# shapes they exist to refuse. Every mutation is verified to have applied before
# its assertion runs: a mutation that silently no-ops would turn the case green
# while testing nothing, which is the failure mode a guard harness must not have.
#
# Everything runs offline against mktemp fixtures. No network, no real workflow.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="$REPO/scripts/check_release_dispatch.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
FAIL=0

fail() {
  echo "FAIL: $*" >&2
  FAIL=$((FAIL + 1))
}

pass() {
  echo "OK: $*"
}

# A fixture is a copy of the real workflows + scripts, so every check other than
# the one under test sees the shipped tree and cannot mask the assertion.
fixture() {
  local dir="$WORK/fixture-$1"
  mkdir -p "$dir/.github"
  cp -r "$REPO/.github/workflows" "$dir/.github/"
  cp -r "$REPO/scripts" "$dir/"
  printf '%s' "$dir"
}

# Applies a perl expression and refuses to proceed if nothing changed.
mutate() {
  local file="$1" expr="$2" label="$3"
  local before after
  before="$(cksum < "$file")"
  perl -0pi -e "$expr" "$file"
  after="$(cksum < "$file")"
  if [[ "$before" == "$after" ]]; then
    fail "[$label] mutation did not apply — the case would assert nothing"
    return 1
  fi
  return 0
}

expect_accept() {
  local dir="$1" label="$2" out rc
  set +e
  out="$(RELEASE_DISPATCH_ROOT="$dir" bash "$GUARD" 2>&1)"
  rc=$?
  set -e
  if [[ "$rc" -eq 0 ]]; then
    pass "[$label] guard accepts it"
  else
    fail "[$label] guard rejected the shipped tree"
    grep -E '::error' <<<"$out" | head -2 >&2
  fi
}

# Rejection must be caused by the expected check, not by some unrelated one.
expect_reject() {
  local dir="$1" label="$2" needle="$3" out rc
  set +e
  out="$(RELEASE_DISPATCH_ROOT="$dir" bash "$GUARD" 2>&1)"
  rc=$?
  set -e
  if [[ "$rc" -eq 0 ]]; then
    fail "[$label] guard accepted a tree it must reject"
  elif ! grep -Fq -- "$needle" <<<"$out"; then
    fail "[$label] guard rejected it for the wrong reason (missing: $needle)"
    grep -E '::error' <<<"$out" | head -2 >&2
  else
    pass "[$label] guard rejects it for the right reason"
  fi
}

# 1. The shipped tree passes. Vacuity guard for everything below: if the real
#    tree already failed, each rejection case would prove nothing.
D="$(fixture base)"
expect_accept "$D" "shipped tree"

# 2. The gate invokes the tag's verifier again: the exact v7.0.0 shape.
D="$(fixture tag-verifier)"
mutate "$D/.github/workflows/release.yml" \
  's{bash gate/scripts/verify-release-signatures\.sh}{bash scripts/verify-release-signatures.sh}' \
  "verifier from the tag's tree" \
  && expect_reject "$D" "verifier from the tag's tree" \
       'must verify signatures with the verifier from the RUN'"'"'s own ref'

# 3. The gate checkout is repointed at the validated commit: no gate at all,
#    so the tag's scripts are what execute.
D="$(fixture gate-repointed)"
# shellcheck disable=SC2016  # the GitHub expression is perl-regex text, not shell
mutate "$D/.github/workflows/release.yml" \
  's{ref: \$\{\{ github\.sha \}\}}{ref: \${{ needs.preflight.outputs.validated_commit }}}' \
  "gate checkout repointed at the tag" \
  && expect_reject "$D" "gate checkout repointed at the tag" \
       'must verify signatures with the verifier from the RUN'"'"'s own ref'

# 4. The gate checkout is a FULL checkout of the run's ref. It would satisfy
#    check 18 while giving the publish job a tree it could build from — which is
#    how a branch gets published under a tag.
D="$(fixture gate-not-sparse)"
mutate "$D/.github/workflows/release.yml" \
  's{^          sparse-checkout: scripts\n}{}m' \
  "gate checkout is not scripts-only" \
  && expect_reject "$D" "gate checkout is not scripts-only" \
       "must declare 'sparse-checkout: scripts'"

# 5. A second github.sha checkout: ambiguous about which tree the build uses.
D="$(fixture two-gates)"
cat >> "$D/.github/workflows/release.yml" <<'YAML'

  gate-echo:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09
        with:
          ref: ${{ github.sha }}
YAML
expect_reject "$D" "two github.sha checkouts" \
  "checkouts pinned to \${{ github.sha }}"

if [[ "$FAIL" -ne 0 ]]; then
  echo "release dispatch gate harness: $FAIL failure(s)" >&2
  exit 1
fi

echo "release dispatch gate harness: all checks passed"
