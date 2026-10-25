#!/usr/bin/env bash
#
# Negation harness for the breaking-marker guard in merge-when-green.sh
# (#2081, #2320).
#
# The defect is a property of the MERGE PATH, not of any line of text: a merge
# can silently destroy a breaking declaration, and the damage only becomes
# visible after the tag is public. So it cannot be proven by grepping the
# script. Every case below runs the real script against a fake 'gh' and asserts
# on what it DID — refused or merged, and with which strategy.
#
# The effective squash subject depends on the commit count (#2321 measured it):
#   1 commit   -> the commit subject
#   2+ commits -> the PR title
# A merge commit keeps every branch commit and discards the PR title. The guard
# must accept a squash whose kept surface carries the marker and refuse the
# other combinations, the malformed `type!(scope):`, and a footer-only
# declaration.
#
# Properties pinned here:
#   1. N=1 title-only + squash is refused (the #2078 shape; the commit is the
#      kept surface, so the title marker dies);
#   2. N=1 commit marker + squash is ALLOWED (the #2321 measurement; the old
#      harness expected a refusal here and that expectation was the false model);
#   3. N>=2 title marker + squash is allowed; N>=2 commit-only + squash is
#      refused (the title is the kept surface);
#   4. --merge keeps the commits and discards the title: N>=2 commit-only is
#      allowed, title-only is refused;
#   5. malformed `type!(scope):` is refused under both strategies;
#   6. a 'BREAKING CHANGE:' footer alone is refused under both strategies;
#   7. a '!' in prose does not block anything;
#   8. an unreadable PR fails CLOSED (rc 4), never open;
#   9. a title/head change during the check wait is caught by the second guard
#      at the merge boundary (#2320 AC7).
#
# Everything runs offline against mktemp fixtures and a fake 'gh'. No network,
# no real repository state, no cargo.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves the reads merge-when-green.sh performs before merging, and records
# the merge it was asked to perform instead of performing one. The head owner is
# deliberately a fork owner so the post-merge remote cleanup is skipped: that
# path shells out to 'git ls-remote' and this harness must stay offline.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"

emit() { # emit <json> [<jq-expr>]
  if [[ -n "${2:-}" ]]; then printf '%s' "$1" | jq -r "$2"; else printf '%s\n' "$1"; fi
}

case "${1:-}" in
  repo)
    shift
    fields=""; jqexpr=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
      esac
      shift
    done
    case "$fields" in
      nameWithOwner) emit '{"nameWithOwner":"owner/repo"}' "$jqexpr" ;;
      owner) emit '{"owner":{"login":"owner"}}' "$jqexpr" ;;
      *) echo "fake gh: unhandled repo view --json $fields" >&2; exit 99 ;;
    esac
    ;;
  api)
    case "${2:-}" in
      repos/*/branches/main/protection)
        emit '{"required_status_checks":{"checks":[{"context":"Validate PR metadata"},{"context":"CI Gate"}]}}' \
          '.required_status_checks.checks[]?.context // empty'
        ;;
      *) echo "fake gh: unhandled api ${2:-}" >&2; exit 99 ;;
    esac
    ;;
  pr)
    sub="${2:-}"
    shift 2
    if [[ $# -gt 0 && "${1:-}" != --* ]]; then shift; fi   # drop the PR number
    fields=""; jqexpr=""; strategy=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
        --squash|--merge) strategy="$1" ;;
      esac
      shift
    done
    case "$sub" in
      checks)
        cat "$state/checks.json"
        ;;
      view)
        case "$fields" in
          # The script reads `state` and `mergeStateStatus` in ONE call (the
          # already-merged detection, #1970). Serving only that combined
          # projection keeps ONE reader: a regression to a single-field read
          # fails loudly here instead of silently working.
          state,mergeStateStatus) emit '{"state":"OPEN","mergeStateStatus":"CLEAN"}' "$jqexpr" ;;
          headRefName) emit '{"headRefName":"fixture-branch"}' "$jqexpr" ;;
          headRepositoryOwner) emit '{"headRepositoryOwner":{"login":"fork-owner"}}' "$jqexpr" ;;
          title,commits,headRefOid)
            [[ -f "$state/pr-meta-fail" ]] && { echo "gh: boom" >&2; exit 1; }
            if [[ -f "$state/pr-meta-advance" ]]; then
              cat "$state/pr-meta.json"
              if [[ -f "$state/pr-meta-next.json" ]]; then
                mv "$state/pr-meta-next.json" "$state/pr-meta.json"
              fi
              rm -f "$state/pr-meta-advance"
            else
              cat "$state/pr-meta.json"
            fi
            ;;
          *) echo "fake gh: unhandled pr view --json $fields" >&2; exit 99 ;;
        esac
        ;;
      merge)
        printf '%s\n' "$strategy" >> "$state/merged"
        ;;
      *) echo "fake gh: unhandled pr $sub" >&2; exit 99 ;;
    esac
    ;;
  *)
    echo "fake gh: unhandled invocation: $*" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

printf '%s\n' '[{"name":"Validate PR metadata","bucket":"pass"},{"name":"CI Gate","bucket":"pass"}]' \
  > "$STATE/checks.json"

# ─── Fixtures ───────────────────────────────────────────────────────────────
# write_pr <title> <commits-json> [<head-oid>]: the exact shape
# 'gh pr view --json title,commits,headRefOid' returns, which is what the guard
# reads.
write_pr() {
  jq -n --arg t "$1" --argjson c "$2" --arg oid "${3:-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}" \
    '{title: $t, commits: $c, headRefOid: $oid}' > "$STATE/pr-meta.json"
  rm -f "$STATE/pr-meta-fail" "$STATE/pr-meta-advance" "$STATE/pr-meta-next.json"
}

RUN_OUT=""
RUN_RC=0
run_merge() {
  rm -f "$STATE/merged"
  RUN_RC=0
  RUN_OUT="$( cd "$REPO_ROOT" && \
    PATH="$BIN:$PATH" \
    FAKE_STATE="$STATE" \
    bash scripts/merge-when-green.sh "$@" 2>&1 )" || RUN_RC=$?
}
merged_strategy() { if [[ -f "$STATE/merged" ]]; then tr -d '\n' < "$STATE/merged"; fi; }

PASS=0
FAIL=0
check() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — expected [%s], got [%s]\n' "$label" "$expected" "$actual"; FAIL=$((FAIL + 1))
  fi
}
check_contains() {
  local label="$1" needle="$2" haystack="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] not found in output\n' "$label" "$needle"; FAIL=$((FAIL + 1))
  fi
}
check_absent() {
  local label="$1" needle="$2" haystack="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] unexpectedly present\n' "$label" "$needle"; FAIL=$((FAIL + 1))
  fi
}
check_no_merge() {
  local label="$1" got
  got="$(merged_strategy)"
  if [[ -z "$got" ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — gh pr merge was invoked with [%s]\n' "$label" "$got"; FAIL=$((FAIL + 1))
  fi
}

echo "test_merge_when_green_breaking: the effective squash surface keeps the marker (webfang#2320)"

# ─── 1. THE NEGATION: N=1 title marker + squash ─────────────────────────────
# This is #2078 exactly: 'fix(cli)!:' in the title, clean commit, --squash.
# With one commit GitHub builds the squash subject from the COMMIT, so the title
# marker is lost. The guard refuses before the check wait.
write_pr 'fix(cli)!: el aviso de ONNX describe la retirada ya ejecutada' \
  '[{"oid":"07e81f8f0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":""}]'
run_merge 2081 --squash
check "N=1 title-only + --squash -> exit 5" "5" "$RUN_RC"
check_no_merge "N=1 title-only + --squash -> nothing merged"
check_contains "N=1 title-only -> names the commit as the kept surface" "COMMIT subject" "$RUN_OUT"
check_absent "N=1 title-only -> refused BEFORE the check wait" \
  "Waiting for required checks" "$RUN_OUT"

# ─── 2. N=1 commit marker + squash is ALLOWED (#2321) ──────────────────────
# The measurement: with one commit the squash subject IS the commit subject.
# The pre-#2321 guard refused this; that refusal was the false model.
write_pr 'chore(release): declarar la ruptura del paso 6' \
  '[{"oid":"437357e7aaaa","messageHeadline":"chore(release)!: declarar la ruptura del paso 6","messageBody":""}]'
run_merge 2081 --squash
check "N=1 commit marker + --squash -> exit 0" "0" "$RUN_RC"
check "N=1 commit marker + --squash -> merged with --squash" "--squash" "$(merged_strategy)"

# ─── 3. N=1 commit marker + --merge is allowed too ─────────────────────────
run_merge 2081 --merge
check "N=1 commit marker + --merge -> exit 0" "0" "$RUN_RC"
check "N=1 commit marker + --merge -> merged with --merge" "--merge" "$(merged_strategy)"

# ─── 4. N=1 title+commit marker + squash ───────────────────────────────────
# The commit is the kept surface and it is canonical; the title is redundant.
write_pr 'feat(ai)!: retirar la feature ai' \
  '[{"oid":"aaaabbbb0000","messageHeadline":"feat(ai)!: retirar la feature ai","messageBody":""}]'
run_merge 2081 --squash
check "N=1 title+commit marker + --squash -> exit 0" "0" "$RUN_RC"
check "N=1 title+commit marker + --squash -> merged with --squash" "--squash" "$(merged_strategy)"

# ─── 5. the malformed 'feat!(ai):' that #2072 shipped ──────────────────────
# Not valid Conventional Commits: release-plz does not parse it. Refused with
# the canonical form, not silently accepted as a declaration.
write_pr 'feat!(ai): retirar la feature ai y el crate webfang_ai' \
  '[{"oid":"e9b7217a0000","messageHeadline":"feat!(ai): retirar la feature ai","messageBody":""}]'
run_merge 2081 --squash
check "malformed feat!(ai): + --squash -> exit 5" "5" "$RUN_RC"
check_no_merge "malformed feat!(ai): -> nothing merged"
check_contains "malformed -> names the canonical form" "type(scope)!:" "$RUN_OUT"

# ─── 5b. malformed title with a clean commit + squash ──────────────────────
# The title is dropped by a 1-commit squash, but a malformed marker there still
# signals an intent that will not reach main. Refused with the canonical form.
write_pr 'feat!(ai): retirar la feature ai' \
  '[{"oid":"e9b7217a0000","messageHeadline":"feat(ai): retirar la feature ai","messageBody":""}]'
run_merge 2081 --squash
check "N=1 malformed title + clean commit + --squash -> exit 5" "5" "$RUN_RC"
check_no_merge "N=1 malformed title + clean commit -> nothing merged"
check_contains "N=1 malformed title -> names the canonical form" "type(scope)!:" "$RUN_OUT"

# ─── 6. BREAKING CHANGE: footer alone + squash ─────────────────────────────
# The footer is not a substitute for the '!': the gate requires the subject
# marker, regardless of what release-plz does with the footer.
write_pr 'fix(cli): el aviso describe la retirada' \
  '[{"oid":"07e81f8f0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":"Detalle.\n\nBREAKING CHANGE: la feature ai ya no existe"}]'
run_merge 2081 --squash
check "footer-only + --squash -> exit 5" "5" "$RUN_RC"
check_no_merge "footer-only + --squash -> nothing merged"
check_contains "footer-only -> says the footer is not accepted" "footer alone is not accepted" "$RUN_OUT"

# ─── 7. N>=2 title marker + squash is allowed ──────────────────────────────
# With 2+ commits the squash subject is the PR title; the commits' subjects are
# not part of it. A canonical title is the correct surface.
write_pr 'feat(ai)!: retirar la feature ai' \
  '[{"oid":"aaaabbbb0000","messageHeadline":"feat(ai): retirar la feature ai","messageBody":""},{"oid":"bbbbcccc0000","messageHeadline":"test(ai): actualizar el harness","messageBody":""}]'
run_merge 2081 --squash
check "N=2 title marker + --squash -> exit 0" "0" "$RUN_RC"
check "N=2 title marker + --squash -> merged with --squash" "--squash" "$(merged_strategy)"

# ─── 8. N>=2 title-only + --merge is refused ───────────────────────────────
# A merge commit keeps the branch commits and DISCARDS the PR title, so a title
# marker never reaches main under --merge.
run_merge 2081 --merge
check "N=2 title-only + --merge -> exit 5" "5" "$RUN_RC"
check_no_merge "N=2 title-only + --merge -> nothing merged"
check_contains "N=2 title-only + --merge -> says the merge discards the title" \
  "DISCARDS the PR title" "$RUN_OUT"

# ─── 9. N>=2 commit-only + squash is refused ───────────────────────────────
# The squash keeps the PR title, not the commits' subjects, so a commit-only
# marker dies silently.
write_pr 'feat(ai): retirar la feature ai' \
  '[{"oid":"aaaabbbb0000","messageHeadline":"feat(ai)!: retirar la feature ai","messageBody":""},{"oid":"bbbbcccc0000","messageHeadline":"test(ai): actualizar el harness","messageBody":""}]'
run_merge 2081 --squash
check "N=2 commit-only + --squash -> exit 5" "5" "$RUN_RC"
check_no_merge "N=2 commit-only + --squash -> nothing merged"
check_contains "N=2 commit-only + --squash -> names the title as the kept surface" "PR TITLE" "$RUN_OUT"

# ─── 10. N>=2 commit-only + --merge is allowed ─────────────────────────────
# A merge commit preserves the commits, so the canonical marker reaches main.
run_merge 2081 --merge
check "N=2 commit-only + --merge -> exit 0" "0" "$RUN_RC"
check "N=2 commit-only + --merge -> merged with --merge" "--merge" "$(merged_strategy)"

# ─── 11. a clean PR is untouched ───────────────────────────────────────────
write_pr 'fix(cli): el aviso describe la retirada' \
  '[{"oid":"aaaabbbb0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":"Detalle sin footer."}]'
run_merge 2081 --squash
check "clean PR + --squash -> exit 0" "0" "$RUN_RC"
check "clean PR + --squash -> merged with --squash" "--squash" "$(merged_strategy)"

# ─── 12. prose is not a marker ─────────────────────────────────────────────
# '!' after the colon and 'BREAKING CHANGE:' mid-line are prose. Blocking on
# them would make the guard noise, and noise gets bypassed.
write_pr "fix(cli): don't reintentar en 5xx" \
  '[{"oid":"ccccdddd0000","messageHeadline":"fix(cli): dont reintentar en 5xx","messageBody":"El trailer BREAKING CHANGE: tiene que ir al inicio de linea para contar."}]'
run_merge 2081 --squash
check "prose '!' and mid-line footer -> exit 0" "0" "$RUN_RC"
check "prose case -> merged with --squash" "--squash" "$(merged_strategy)"

# ─── 13. unreadable PR fails CLOSED ────────────────────────────────────────
write_pr 'fix(cli): cualquiera' '[]'
: > "$STATE/pr-meta-fail"
run_merge 2081 --squash
check "unreadable PR -> exit 4 (not a silent merge)" "4" "$RUN_RC"
check_no_merge "unreadable PR -> nothing merged"
check_contains "unreadable PR -> says it refuses to merge blind" "blind" "$RUN_OUT"
rm -f "$STATE/pr-meta-fail"

# ─── 13b. incomplete or malformed metadata also fails CLOSED ───────────────
# A non-empty but INCOMPLETE payload is not evidence that the marker is
# absent. It must exit 4, never fall into the "2+ commits, clean title" default
# that would merge a canonical title silently (#2320 AC6/AC10).
write_pr 'feat(ai)!: retirada' '[]'
run_merge 2081 --squash
check "zero commits -> exit 4" "4" "$RUN_RC"
check_no_merge "zero commits -> nothing merged"

jq 'del(.commits)' "$STATE/pr-meta.json" > "$STATE/pr-meta.next" && mv "$STATE/pr-meta.next" "$STATE/pr-meta.json"
run_merge 2081 --squash
check "missing commits -> exit 4" "4" "$RUN_RC"
check_no_merge "missing commits -> nothing merged"

jq '.commits = "nope"' "$STATE/pr-meta.json" > "$STATE/pr-meta.next" && mv "$STATE/pr-meta.next" "$STATE/pr-meta.json"
run_merge 2081 --squash
check "non-array commits -> exit 4" "4" "$RUN_RC"
check_no_merge "non-array commits -> nothing merged"

write_pr 'feat(ai)!: retirada' \
  '[{"oid":"abc123450000","messageHeadline":"feat(ai)!: retirada","messageBody":""}]'
jq 'del(.headRefOid)' "$STATE/pr-meta.json" > "$STATE/pr-meta.next" && mv "$STATE/pr-meta.next" "$STATE/pr-meta.json"
run_merge 2081 --squash
check "missing headRefOid -> exit 4" "4" "$RUN_RC"
check_no_merge "missing headRefOid -> nothing merged"

printf 'not-json' > "$STATE/pr-meta.json"
run_merge 2081 --squash
check "invalid JSON -> exit 4" "4" "$RUN_RC"
check_no_merge "invalid JSON -> nothing merged"
check_absent "invalid JSON -> no raw jq parse error leaked" "parse error" "$RUN_OUT"

# ─── 14. --dry-run refuses too ─────────────────────────────────────────────
write_pr 'fix(cli)!: el aviso describe la retirada' \
  '[{"oid":"aaaabbbb0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":""}]'
run_merge 2081 --dry-run
check "N=1 title-only + --dry-run -> exit 5" "5" "$RUN_RC"
check_no_merge "N=1 title-only + --dry-run -> nothing merged"

# ─── 15. a title/head change during the wait is caught at the boundary ─────
# The first read passes (clean N=1). The PR then changes: the title gains the
# '!' and the head moves. The second read must re-run the guard and refuse; the
# stale decision must not merge (#2320 AC7).
write_pr 'fix(cli): el aviso describe la retirada' \
  '[{"oid":"111122220000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":""}]' \
  '1111222200000000000000000000000000000000'
jq '.title = "fix(cli)!: el aviso describe la retirada" | .headRefOid = "2222333300000000000000000000000000000000"' \
  "$STATE/pr-meta.json" > "$STATE/pr-meta-next.json"
touch "$STATE/pr-meta-advance"
run_merge 2081 --squash
check "title/head change during the wait -> exit 5" "5" "$RUN_RC"
check_no_merge "title/head change during the wait -> nothing merged"
check_contains "title/head change during the wait -> logs the head change" "PR head changed" "$RUN_OUT"

# ─── 16. footer-only + --merge is still refused ─────────────────────────────
# The merge preserves the footer in the commit body, but the gate requires the
# '!' in the subject.
write_pr 'fix(cli): el aviso describe la retirada' \
  '[{"oid":"07e81f8f0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":"Detalle.\n\nBREAKING CHANGE: la feature ai ya no existe"}]'
run_merge 2081 --merge
check "footer-only + --merge -> exit 5" "5" "$RUN_RC"
check_no_merge "footer-only + --merge -> nothing merged"

# ─── 17. malformed + --merge is refused ────────────────────────────────────
# The merge preserves the malformed subject; release-plz still does not parse
# it, so the declaration dies.
write_pr 'feat!(ai): retirar la feature ai' \
  '[{"oid":"e9b7217a0000","messageHeadline":"feat!(ai): retirar la feature ai","messageBody":""}]'
run_merge 2081 --merge
check "malformed + --merge -> exit 5" "5" "$RUN_RC"
check_no_merge "malformed + --merge -> nothing merged"

echo "test_merge_when_green_breaking: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: the breaking marker must live in the surface the strategy keeps."
