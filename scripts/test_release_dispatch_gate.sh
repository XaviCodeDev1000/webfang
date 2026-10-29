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
# Three of this harness's own guarantees are pinned here too, because a harness
# that cannot fail is indistinguishable from a green one (webfang#2488):
#   * every fixture mutation goes through mutate(), which verifies it applied —
#     including the appended-job case, which used to bypass it;
#   * a guard that fails WITHOUT emitting `::error` is still a counted failure:
#     the diagnostic grep is never the tail of the branch, so `set -o pipefail`
#     plus `set -e` cannot abort the run before the summary line;
#   * the gate's premise is proven, not assumed: the publish job checks out only
#     `scripts/`, so the verifier is executed from a tree that contains nothing
#     but `scripts/` and must not fail on a missing file.
#
# Everything runs offline against mktemp fixtures. No network, no real workflow.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="${RELEASE_DISPATCH_GUARD:-$REPO/scripts/check_release_dispatch.sh}"
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

# Diagnostics, never a verdict. A guard can fail without printing a single
# `::error` line — a syntax error, a missing tool, a `set -u` abort — and the
# grep used to be the LAST command of the failure branch. Under `set -o pipefail`
# a silent guard made grep exit 1, and under `set -e` the harness died before it
# counted the failure or printed its summary: a real failure, unrecorded.
dump_guard_errors() {
  grep -E '::error' <<<"$1" | head -2 >&2 || true
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
    dump_guard_errors "$out"
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
    dump_guard_errors "$out"
  elif ! grep -Fq -- "$needle" <<<"$out"; then
    fail "[$label] guard rejected it for the wrong reason (missing: $needle)"
    dump_guard_errors "$out"
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
#    Through mutate() like every other case: an append that silently no-ops
#    (a missing newline, a heredoc that lands nowhere) would leave the shipped
#    tree intact and the assertion would pass while testing nothing.
D="$(fixture two-gates)"
# shellcheck disable=SC2016  # the GitHub expression is perl-regex text, not shell
mutate "$D/.github/workflows/release.yml" \
  's{\z}{\n  gate-echo:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout\@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09\n        with:\n          ref: \${{ github.sha }}\n}' \
  "two github.sha checkouts" \
  && expect_reject "$D" "two github.sha checkouts" \
       "checkouts pinned to \${{ github.sha }}"

# 5b. A COMMENT quoting the tag's verifier must not flip a correct tree to a
# rejection: comment lines are not command lines (webfang#2486 fix for check 18).
# The old file-wide count treated the comment as a tag-verifier invocation.
D="$(fixture verifier-comment)"
mutate "$D/.github/workflows/release.yml" \
  's{(\n          ref: \$\{\{ github\.sha \}\}\n)}{$1          # reference only: bash scripts/verify-release-signatures.sh is the tag copy\n}m' \
  "comment quoting the tag verifier on a correct tree" \
  && expect_accept "$D" "comment quoting the tag verifier on a correct tree"

# 5c. A FULL gate checkout plus an UNRELATED sparse-checkout: scripts elsewhere
# must still be rejected: the sparse key is bound to the gate block, not counted
# file-wide (webfang#2486 fix for check 8). The old code counted the unrelated
# key and accepted.
D="$(fixture gate-full-plus-sparse-elsewhere)"
mutate "$D/.github/workflows/release.yml" \
  's{^          sparse-checkout: scripts\n}{}mg; s{(\n          ref: \$\{\{ github\.event\.inputs\.tag \|\| github\.ref_name \}\}\n          fetch-tags: true\n)}{$1          sparse-checkout: scripts\n}m' \
  "full gate checkout plus unrelated sparse-checkout elsewhere" \
  && expect_reject "$D" "full gate checkout plus unrelated sparse-checkout elsewhere" \
       "must declare 'sparse-checkout: scripts'"

# 5d. A real checkout missing fetch-depth: 0 must be rejected even when a COMMENT
# mentions fetch-depth: 0: the requirement is bound to the checkout blocks, not
# to a file-wide grep (webfang#2486 fix for check 14). The old code let the
# comment satisfy the count.
D="$(fixture fetch-depth-comment)"
mutate "$D/.github/workflows/release.yml" \
  's{^          fetch-depth: 0\n}{}m; s{(\n          ref: \$\{\{ github\.event\.inputs\.tag \|\| github\.ref_name \}\}\n          fetch-tags: true\n)}{$1          # fetch-depth: 0\n}m' \
  "real checkout missing fetch-depth behind a comment" \
  && expect_reject "$D" "real checkout missing fetch-depth behind a comment" \
       "missing 'fetch-depth: 0'"

# 5e. A COMMENT quoting the gate verifier must not rescue a tree that actually
# runs the tag's verifier: the comment is not a command line (webfang#2486 fix
# for check 18). The old code counted the comment as GATE_INVOKED and accepted.
D="$(fixture tag-verifier-plus-gate-comment)"
mutate "$D/.github/workflows/release.yml" \
  's{bash gate/scripts/verify-release-signatures\.sh}{bash scripts/verify-release-signatures.sh}; s{(\n          ref: \$\{\{ github\.sha \}\}\n)}{$1          # bash gate/scripts/verify-release-signatures.sh is what we intend\n}m' \
  "tag verifier plus gate-form comment" \
  && expect_reject "$D" "tag verifier plus gate-form comment" \
       "must verify signatures with the verifier from the RUN"

# 5f. A COMMENT quoting 'sparse-checkout: scripts' inside the gate block must not
# satisfy check 8 (webfang#2486). The count regex is line-anchored
# (^[[:space:]]*sparse-checkout:), so prose can never match it; this case pins
# the property so the regex cannot later be loosened to an unanchored match
# without failing the harness.
D="$(fixture gate-sparse-behind-comment)"
mutate "$D/.github/workflows/release.yml" \
  's{^          sparse-checkout: scripts\n}{}mg; s{(\n          ref: \$\{\{ github\.sha \}\}\n)}{$1          # sparse-checkout: scripts (reference only)\n}m' \
  "gate sparse-checkout hidden behind a comment" \
  && expect_reject "$D" "gate sparse-checkout hidden behind a comment" \
       "must declare 'sparse-checkout: scripts'"

# 6. A guard that fails WITHOUT ::error must still be a RECORDED failure, and the
#    run must still reach its summary. Proven end to end: re-run this harness
#    against a stub guard that exits 1 silently and assert that it (a) exits
#    non-zero — the failures were counted — and (b) still prints the summary
#    line, which a set -e abort skips. The diagnostic grep used to be the tail
#    of the failure branch: under set -o pipefail a silent guard made it exit 1,
#    and set -e killed the harness with no summary and no count.
if [[ "${HARNESS_SILENT_PROBE:-}" != "1" ]]; then
  SILENT_GUARD="$WORK/silent-guard.sh"
  printf '#!/usr/bin/env bash\nexit 1\n' >"$SILENT_GUARD"
  chmod +x "$SILENT_GUARD"
  set +e
  sub_out="$(HARNESS_SILENT_PROBE=1 RELEASE_DISPATCH_GUARD="$SILENT_GUARD" \
    bash "${BASH_SOURCE[0]}" 2>&1)"
  sub_rc=$?
  set -e
  if [[ "$sub_rc" -ne 0 ]]; then
    pass "[silent guard] failures are recorded (exit $sub_rc, not a clean run)"
  else
    fail "[silent guard] a guard that rejects everything was reported green"
  fi
  case "$sub_out" in
    *"release dispatch gate harness:"*"failure(s)")
      pass "[silent guard] summary line is still printed" ;;
    *) fail "[silent guard] the run aborted before its summary line" ;;
  esac
fi

# 7. The gate premise, proven: the publish job checks out ONLY scripts/, so the
#    verifier must be self-contained there. Today it resolves its legacy
#    allowlist from SCRIPT_DIR; nothing stopped a future edit from reading a path
#    outside scripts/, which would pass every grep above and fail in the publish
#    job. Run the real verifier from a tree that contains nothing but scripts/.
SCRIPTS_ONLY="$WORK/scripts-only"
mkdir -p "$SCRIPTS_ONLY"
cp -r "$REPO/scripts" "$SCRIPTS_ONLY/"
# Vacuity guard: the fixture really is scripts-only.
if [[ -z "$(find "$SCRIPTS_ONLY" -mindepth 1 -maxdepth 1 ! -name scripts)" ]]; then
  pass "[scripts-only] fixture contains nothing but scripts/"
else
  fail "[scripts-only] fixture has entries outside scripts/"
fi
set +e
out="$(cd "$SCRIPTS_ONLY" && COSIGN=/nonexistent-cosign bash scripts/verify-release-signatures.sh --tag v1.0.0 some-asset 2>&1)"
rc=$?
set -e
# It must fail (no cosign), but on the verifier terms: the allowlist next to the
# script was read, and nothing outside scripts/ was looked for.
if [[ "$rc" -ne 0 ]]; then
  pass "[scripts-only] verifier fails closed without cosign (rc=$rc)"
else
  fail "[scripts-only] verifier exited 0 without cosign"
fi
case "$out" in
  *"historical tag allowlist: v1.0.0"*) pass "[scripts-only] allowlist read from SCRIPT_DIR" ;;
  *) fail "[scripts-only] allowlist was not read from SCRIPT_DIR" ;;
esac
case "$out" in
  *"falta la allowlist histórica"*) fail "[scripts-only] verifier wanted a file outside scripts/" ;;
  *) pass "[scripts-only] no file outside scripts/ is required" ;;
esac
# And the allowlist is consulted, not merely present: a tag that is not in it is
# rejected by content, in the same scripts-only tree.
set +e
out="$(cd "$SCRIPTS_ONLY" && COSIGN=/nonexistent-cosign bash scripts/verify-release-signatures.sh --tag v9.9.9 some-asset 2>&1)"
set -e
case "$out" in
  *"no está en la allowlist histórica"*) pass "[scripts-only] allowlist content is consulted" ;;
  *) fail "[scripts-only] an unlisted tag was not rejected by the allowlist" ;;
esac

if [[ "$FAIL" -ne 0 ]]; then
  echo "release dispatch gate harness: $FAIL failure(s)" >&2
  exit 1
fi

echo "release dispatch gate harness: all checks passed"
