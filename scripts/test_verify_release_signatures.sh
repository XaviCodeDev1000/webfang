#!/usr/bin/env bash
# Behavioral harness for the release-signature identity rules (#2281).
#
# Proves, without network or a real cosign:
#   - the default identity is the current repository, derived from
#     GITHUB_REPOSITORY when present;
#   - --tag accepts only frozen pre-migration tags from the checked-in
#     allowlist, and refuses every new tag;
#   - --tag cannot be combined with an explicit identity override;
#   - a missing bundle, a missing cosign and an empty asset list fail closed;
#   - the identity regexp actually reaches cosign, not just the log line;
#   - the OIDC workflow-ref and workflow-trigger claims reach cosign when
#     pinned, and the default invocation pins none of them (webfang#2464).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/verify-release-signatures.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

LOG="$WORK/cosign.log"
FAKE_COSIGN="$WORK/cosign"
ASSET="$WORK/webfang-test.tar.gz"
NO_BUNDLE="$WORK/webfang-no-bundle.tar.gz"
FAIL=0
OLD_OWNER="XaviCode""1000"

cat > "$FAKE_COSIGN" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "version" ]]; then
  exit 0
fi
if [[ "${1:-}" == "verify-blob" ]]; then
  printf '%s\n' "$*" >> "$FAKE_COSIGN_LOG"
  # Drive the two rejection classes the script now distinguishes (webfang#2487):
  #   FAKE_COSIGN_EXIT         -> exit when --certificate-identity-regexp is
  #                              present (the full, identity-constrained check).
  #   FAKE_COSIGN_RELAXED_EXIT -> exit when it is absent (the bare issuer-only
  #                              relaxed check proves the signature itself is
  #                              sound, i.e. a ref/trigger-shape mismatch).
  #                              Default 0 in both cases.
  has_identity=0
  for a in "$@"; do
    if [[ "$a" == "--certificate-identity-regexp" ]]; then
      has_identity=1
    fi
  done
  if [[ "$has_identity" -eq 1 ]]; then
    exit "${FAKE_COSIGN_EXIT:-0}"
  else
    exit "${FAKE_COSIGN_RELAXED_EXIT:-0}"
  fi
fi
exit 1
FAKE
chmod +x "$FAKE_COSIGN"

printf 'payload\n' > "$ASSET"
printf '{}\n' > "$ASSET.sigstore.json"
printf 'payload\n' > "$NO_BUNDLE"
: > "$LOG"

fail() {
  echo "FAIL: $*" >&2
  FAIL=$((FAIL + 1))
}

pass() {
  echo "OK: $*"
}

run_strict() {
  env -u GITHUB_REPOSITORY -u RELEASE_CERT_IDENTITY_REGEXP \
    -u RELEASE_CERT_WORKFLOW_REF -u RELEASE_CERT_WORKFLOW_TRIGGER \
    COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" \
    bash "$SCRIPT" "$@"
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  if grep -Fq -- "$needle" <<<"$haystack"; then
    pass "output contains $needle"
  else
    fail "output does not contain $needle"
  fi
}

assert_not_contains() {
  local haystack="$1"
  local needle="$2"
  if grep -Fq -- "$needle" <<<"$haystack"; then
    fail "output unexpectedly contains $needle"
  else
    pass "output does not contain $needle"
  fi
}

assert_fails() {
  local label="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    fail "$label unexpectedly succeeded"
  else
    pass "$label fails closed"
  fi
}

# 1. Strict default: current repository, no legacy identity.
out="$(run_strict "$ASSET" 2>&1)"
assert_contains "$out" "XaviCodeDev1000/webfang"
assert_not_contains "$out" "$OLD_OWNER/webfang"
assert_contains "$(cat "$LOG")" "XaviCodeDev1000/webfang"

# 2. A fork verifies its own releases without editing the script.
: > "$LOG"
out="$(env -u RELEASE_CERT_IDENTITY_REGEXP GITHUB_REPOSITORY='fork-owner/fork-repo' \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" bash "$SCRIPT" "$ASSET" 2>&1)"
assert_contains "$out" "fork-owner/fork-repo"

# 3. A frozen historical tag uses the allowlisted old identity.
: > "$LOG"
out="$(run_strict --tag v5.0.0 "$ASSET" 2>&1)"
assert_contains "$out" "historical tag allowlist: v5.0.0"
assert_contains "$out" "$OLD_OWNER/webfang"
assert_contains "$(cat "$LOG")" "$OLD_OWNER/webfang"

# 4. A new tag is not allowed to borrow the old identity.
assert_fails "tag v5.0.1 outside the allowlist" \
  run_strict --tag v5.0.1 "$ASSET"

# 5. --tag and an explicit override are ambiguous and must not be combined.
assert_fails "--tag with --identity-regexp" \
  run_strict --tag v5.0.0 --identity-regexp '^https://example\.com/release$' "$ASSET"

# 6. --tag and RELEASE_CERT_IDENTITY_REGEXP are ambiguous too.
assert_fails "--tag with RELEASE_CERT_IDENTITY_REGEXP" \
  env -u GITHUB_REPOSITORY RELEASE_CERT_IDENTITY_REGEXP='^https://example\.com/release$' \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" bash "$SCRIPT" --tag v5.0.0 "$ASSET"

# 7. Explicit override still works and reaches cosign.
: > "$LOG"
out="$(run_strict --identity-regexp '^https://example\.com/release$' "$ASSET" 2>&1)"
assert_contains "$out" "certificate identity regexp: ^https://example\\.com/release$"
assert_contains "$(cat "$LOG")" "--certificate-identity-regexp ^https://example\.com/release$"

# 8. Missing bundle and empty asset list fail closed.
assert_fails "asset without a bundle" run_strict "$NO_BUNDLE"
assert_fails "empty asset list" run_strict

# 9. A missing verifier fails closed.
assert_fails "missing cosign" \
  env -u GITHUB_REPOSITORY -u RELEASE_CERT_IDENTITY_REGEXP \
  COSIGN="$WORK/does-not-exist" bash "$SCRIPT" "$ASSET"

# ─────────────────────────────────────────────────────────────────────────────
# OIDC claim pinning (webfang#2464). The certificate identity alone cannot tell
# a tag push from a default-branch dispatch: cosign reads the ref from the
# ambient OIDC token, so a dispatch signs as `@refs/heads/<default-branch>` and
# a tag-only identity rejects the pipeline's own output. The ref and the trigger
# are pinned as CLAIMS instead: the identity regexp stays a statement about
# WHICH workflow file, and the claims state WHICH run.
# ─────────────────────────────────────────────────────────────────────────────
BRANCH_IDENTITY='^https://github\.com/XaviCodeDev1000/webfang/\.github/workflows/release\.yml@refs/(heads|tags)/.+$'

# 10. Pinned claims reach cosign, not just the log line.
: > "$LOG"
out="$(run_strict --identity-regexp "$BRANCH_IDENTITY" \
  --workflow-ref refs/heads/main --workflow-trigger workflow_dispatch "$ASSET" 2>&1)"
assert_contains "$(cat "$LOG")" "--certificate-github-workflow-ref refs/heads/main"
assert_contains "$(cat "$LOG")" "--certificate-github-workflow-trigger workflow_dispatch"
assert_contains "$out" "pinned workflow ref:      refs/heads/main"
assert_contains "$out" "pinned workflow trigger:  workflow_dispatch"

# 11. The default invocation pins NO claims: a tag push keeps the tag-only
#     identity, and a claim nobody asked for is never silently accepted.
: > "$LOG"
run_strict "$ASSET" >/dev/null 2>&1
assert_not_contains "$(cat "$LOG")" "--certificate-github-workflow-ref"
assert_not_contains "$(cat "$LOG")" "--certificate-github-workflow-trigger"

# 12. The env form is the one release.yml uses, and behaves identically.
: > "$LOG"
out="$(env -u GITHUB_REPOSITORY \
  RELEASE_CERT_IDENTITY_REGEXP="$BRANCH_IDENTITY" \
  RELEASE_CERT_WORKFLOW_REF='refs/tags/v7.1.0' \
  RELEASE_CERT_WORKFLOW_TRIGGER='push' \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" bash "$SCRIPT" "$ASSET" 2>&1)"
assert_contains "$(cat "$LOG")" "--certificate-github-workflow-ref refs/tags/v7.1.0"
assert_contains "$(cat "$LOG")" "--certificate-github-workflow-trigger push"

# 13. Half a pin is ambiguous: a pinned trigger with an unpinned ref (or the
#     reverse) is the "I thought I constrained it" shape this guard refuses.
assert_fails "workflow-ref without workflow-trigger" \
  run_strict --identity-regexp "$BRANCH_IDENTITY" --workflow-ref refs/heads/main "$ASSET"
assert_fails "workflow-trigger without workflow-ref" \
  run_strict --identity-regexp "$BRANCH_IDENTITY" --workflow-trigger workflow_dispatch "$ASSET"

# 14. Claims checked against the tag-only default would contradict them.
#     Refuse up front instead of failing later inside cosign, where the reason
#     for the rejection is invisible.
assert_fails "claims with the tag-only default identity" \
  run_strict --workflow-ref refs/heads/main --workflow-trigger workflow_dispatch "$ASSET"

# 15. A ref that is not a git ref is a typo, not a pin.
assert_fails "workflow-ref without the refs/ prefix" \
  run_strict --identity-regexp "$BRANCH_IDENTITY" \
  --workflow-ref main --workflow-trigger workflow_dispatch "$ASSET"

# 16. --tag is the historical identity era; it cannot borrow claim pinning.
assert_fails "--tag with claim pinning" \
  run_strict --tag v5.0.0 --workflow-ref refs/tags/v5.0.0 --workflow-trigger push "$ASSET"

# 17. A pin that is DEFINED BUT EMPTY is a misconfiguration, not an intent to
#     skip the check. Reading it as "no pin" would silently downgrade the gate
#     to a shape-only regexp — the exact widening this change exists to avoid.
assert_fails "empty RELEASE_CERT_WORKFLOW_REF" \
  env -u GITHUB_REPOSITORY \
  RELEASE_CERT_IDENTITY_REGEXP="$BRANCH_IDENTITY" \
  RELEASE_CERT_WORKFLOW_REF='' \
  RELEASE_CERT_WORKFLOW_TRIGGER='workflow_dispatch' \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" bash "$SCRIPT" "$ASSET"
assert_fails "empty RELEASE_CERT_WORKFLOW_TRIGGER" \
  env -u GITHUB_REPOSITORY \
  RELEASE_CERT_IDENTITY_REGEXP="$BRANCH_IDENTITY" \
  RELEASE_CERT_WORKFLOW_REF='refs/heads/main' \
  RELEASE_CERT_WORKFLOW_TRIGGER='' \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" bash "$SCRIPT" "$ASSET"

# ─────────────────────────────────────────────────────────────────────────────
# The default's rejection must be actionable (webfang#2479). The tag-only
# default is correct and stays: it is what makes forgetting the flags fail
# closed. But a consumer holding a backstop-signed release gets cosign's bare
# exit, with no way to learn that the ref, not the signature, is what failed.
# ─────────────────────────────────────────────────────────────────────────────

# 18. Default identity + a rejection => the hint names the cause and the fix.
out="$(env -u GITHUB_REPOSITORY -u RELEASE_CERT_IDENTITY_REGEXP \
  -u RELEASE_CERT_WORKFLOW_REF -u RELEASE_CERT_WORKFLOW_TRIGGER \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" FAKE_COSIGN_EXIT=1 \
  bash "$SCRIPT" "$ASSET" 2>&1 || true)"
assert_contains "$out" "la identidad por defecto es TAG-ONLY"
assert_contains "$out" "Signed Ref"
assert_contains "$out" "--workflow-ref"
assert_contains "$out" "--workflow-trigger"

# 18b. A rejection that is NOT a ref-shape mismatch (a genuinely invalid
#      signature) must NOT be reassured with the default-identity hint.
#      The relaxed bundle+issuer check also fails, so REF_SHAPE_REJECTED
#      stays 0 and the hint is withheld (webfang#2487).
out="$(env -u GITHUB_REPOSITORY -u RELEASE_CERT_IDENTITY_REGEXP \
  -u RELEASE_CERT_WORKFLOW_REF -u RELEASE_CERT_WORKFLOW_TRIGGER \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" \
  FAKE_COSIGN_EXIT=1 FAKE_COSIGN_RELAXED_EXIT=1 \
  bash "$SCRIPT" "$ASSET" 2>&1 || true)"
assert_not_contains "$out" "la identidad por defecto es TAG-ONLY"
assert_contains "$out" "firma inválida"

# 19. A pinned identity suppresses the hint: the rejection is then about the
#     caller's own pin, and pointing at the default would be noise.
out="$(env -u GITHUB_REPOSITORY \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" FAKE_COSIGN_EXIT=1 \
  bash "$SCRIPT" --identity-regexp "$BRANCH_IDENTITY" \
  --workflow-ref refs/heads/main --workflow-trigger workflow_dispatch "$ASSET" 2>&1 || true)"
assert_not_contains "$out" "la identidad por defecto es TAG-ONLY"

# 20. A missing bundle is a different failure and must NOT get the hint: the
#     default's ref shape is not why an unsigned asset failed, and pointing a
#     reader there sends them to the wrong cause.
out="$(env -u GITHUB_REPOSITORY -u RELEASE_CERT_IDENTITY_REGEXP \
  -u RELEASE_CERT_WORKFLOW_REF -u RELEASE_CERT_WORKFLOW_TRIGGER \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" \
  bash "$SCRIPT" "$NO_BUNDLE" 2>&1 || true)"
assert_contains "$out" "SIN FIRMAR"
assert_not_contains "$out" "la identidad por defecto es TAG-ONLY"

if [[ "$FAIL" -ne 0 ]]; then
  echo "release signature identity harness: $FAIL failure(s)" >&2
  exit 1
fi

echo "release signature identity harness: all checks passed"
