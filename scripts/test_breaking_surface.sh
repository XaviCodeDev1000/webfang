#!/usr/bin/env bash
#
# Semantics harness for scripts/check_breaking_surface.sh (#2328).
#
# The required CI check `breaking-surface` (pr-validation.yml) applies the
# breaking-change invariant on the PR head and on the merge_group entry. Its
# decisions come from scripts/breaking_surface_lib.sh — the same library
# merge-when-green.sh consumes (pinned by test_merge_when_green_breaking.sh),
# so this harness pins what is NEW here, on top of the shared matrix:
#
#   1. THE NEGATION: a title-only marker under a SQUASH queue is red even
#      though a naive check ("the title has the !") would pass it — the
#      kept surface is the commit subject (#2078's shape, now in CI).
#   2. The full fixture matrix, both strategies, every verdict token.
#   3. Fail closed on incomplete/invalid/zero-commit metadata.
#   4. The 100-commit cap: `gh pr view --json commits` truncates the GraphQL
#      connection, so a marker on commit 101 used to be invisible. The REST
#      read is paginated, and a marker on commit 101 must change the verdict.
#   5. The queue merge method comes from the tracked mirror
#      (.github/merge-queue.json): GITHUB_TOKEN cannot read the live queue
#      configuration (GraphQL answers FORBIDDEN for integrations, #2412).
#      SQUASH judges the squash surface, MERGE/REBASE the merge surface, and
#      an absent base, an unreadable mirror or an unknown method is red
#      (fail closed). The local guard cross-checks the mirror against the
#      LIVE configuration at the merge boundary (pinned in
#      test_merge_when_green_queue.sh).
#   6. The workflow wiring: the job exists, resolves the queue's synthetic ref
#      the same way `validate` does, and the script is referenced.
#
# Everything runs offline against mktemp fixtures and a fake `gh`. No network,
# no real repository state, no cargo.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves exactly the two reads the check performs: `gh pr view --json
# title,headRefOid,baseRefName` and the REST commit list. The queue method
# comes from the mirror fixture, not from gh (#2412). It applies --jq like the
# real gh, so the script's projections are exercised verbatim.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"

emit() { # emit <json> [<jq-expr>]
  if [[ -n "${2:-}" ]]; then printf '%s' "$1" | jq -r "$2"; else printf '%s\n' "$1"; fi
}

case "${1:-}" in
  pr)
    sub="${2:-}"
    shift 2
    [[ $# -gt 0 && "${1:-}" != --* ]] && shift   # drop the PR number
    fields=""; jqexpr=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
      esac
      shift
    done
    case "$sub" in
      view)
        [[ -f "$state/pr-meta-fail" ]] && { echo "gh: boom" >&2; exit 1; }
        case "$fields" in
          title,headRefOid,baseRefName) emit "$(cat "$state/pr-meta.json")" "$jqexpr" ;;
          state) emit '{"state":"OPEN"}' "$jqexpr" ;;
          *) echo "fake gh: unhandled pr view --json $fields" >&2; exit 99 ;;
        esac
        ;;
      *) echo "fake gh: unhandled pr $sub" >&2; exit 99 ;;
    esac
    ;;
  api)
    shift
    [[ "${1:-}" == "--paginate" ]] && shift
    case "${1:-}" in
      repos/*/pulls/*/commits*)
        [[ -f "$state/rest-fail" ]] && { echo "gh: boom" >&2; exit 1; }
        [[ -f "$state/rest-empty" ]] && { printf '' >&1; exit 0; }
        jqexpr=""
        while [[ $# -gt 0 ]]; do
          case "$1" in --jq) shift; jqexpr="${1:-}" ;; esac
          shift
        done
        emit "$(cat "$state/rest-commits.json")" "$jqexpr"
        ;;
      *) echo "fake gh: unhandled api path ${1:-}" >&2; exit 99 ;;
    esac
    ;;
  *)
    echo "fake gh: unhandled invocation: $*" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

# ─── Helpers ────────────────────────────────────────────────────────────────
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
check_grep() {
  local label="$1" pattern="$2" file="$3"
  if grep -qE "$pattern" "$file"; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] not found in %s\n' "$label" "$pattern" "$file"; FAIL=$((FAIL + 1))
  fi
}
check_no_grep() {
  local label="$1" pattern="$2" file="$3"
  if ! grep -qE "$pattern" "$file"; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] unexpectedly present in %s\n' "$label" "$pattern" "$file"; FAIL=$((FAIL + 1))
  fi
}

RUN_RC=0
RUN_OUT=""
run_fixture() { # run_fixture <meta-file> <strategy>
  RUN_RC=0
  RUN_OUT="$( cd "$REPO_ROOT" && \
    bash scripts/check_breaking_surface.sh --fixture "$1" --strategy "$2" 2>&1 )" || RUN_RC=$?
}
run_check() { # production mode against the fake gh
  RUN_RC=0
  RUN_OUT="$( cd "$REPO_ROOT" && \
    PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
    BREAKING_SURFACE_QUEUE_FILE="$STATE/queue-mirror.json" \
    PR_NUMBER=1 REPO=owner/repo GH_TOKEN=fake \
    bash scripts/check_breaking_surface.sh 2>&1 )" || RUN_RC=$?
}

# fixture_meta <title> <commits-json> [<head-oid>] -> assembled meta file
FIXTURE="$WORK/fixture.json"
fixture_meta() {
  jq -n --arg t "$1" --argjson c "$2" --arg oid "${3:-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}" \
    '{title: $t, headRefOid: $oid, commits: $c}' > "$FIXTURE"
}
# rest_meta <title> <rest-array-json>: production fixtures. `rest` is the raw
# GitHub REST shape ({sha, commit:{message}}) the paginated call returns.
write_pr_meta() { # <title> <head-oid>
  jq -n --arg t "$1" --arg oid "$2" --arg base "main" \
    '{title: $t, headRefOid: $oid, baseRefName: $base}' > "$STATE/pr-meta.json"
  rm -f "$STATE/pr-meta-fail" "$STATE/graphql-fail" "$STATE/rest-fail" \
        "$STATE/rest-empty" "$STATE/queue-absent"
}
write_rest() { # <rest-array-json>
  printf '%s' "$1" > "$STATE/rest-commits.json"
}
rest_commit() { # rest_commit <sha> <full-message>
  jq -n --arg sha "$1" --arg msg "$2" '{sha: $sha, commit: {message: $msg}}'
}

echo "test_breaking_surface: the CI check keeps the marker on the kept surface (#2328)"

# ─── 1. THE NEGATION: N=1 title-only marker under a SQUASH queue ────────────
# NEGATION: a title-only marker under a SQUASH queue must be RED — the kept
# surface is the single commit's subject, so a check that passed on the title
# alone would re-arm the #2078 hole in CI.
# The #2078 shape, now in CI: the title carries the `!`, the (single) commit
# does not. A naive title check would pass; the kept surface is the commit
# subject, so the check must be RED.
fixture_meta 'fix(cli)!: el aviso de ONNX describe la retirada ya ejecutada' \
  '[{"oid":"07e81f8f0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":""}]'
run_fixture "$FIXTURE" squash
check "N=1 title-only + squash -> red" "1" "$RUN_RC"
check_contains "N=1 title-only -> names the commit subject as the kept surface" "SUBJECT del commit" "$RUN_OUT"

# ─── 2. The fixture matrix, both strategies ─────────────────────────────────
fixture_meta 'chore(release): declarar la ruptura del paso 6' \
  '[{"oid":"437357e70000","messageHeadline":"chore(release)!: declarar la ruptura del paso 6","messageBody":""}]'
run_fixture "$FIXTURE" squash
check "N=1 commit marker + squash -> green" "0" "$RUN_RC"
run_fixture "$FIXTURE" merge
check "N=1 commit marker + merge -> green" "0" "$RUN_RC"

fixture_meta 'feat!(ai): retirar la feature ai' \
  '[{"oid":"e9b7217a0000","messageHeadline":"feat!(ai): retirar la feature ai","messageBody":""}]'
run_fixture "$FIXTURE" squash
check "malformed type!(scope): + squash -> red" "1" "$RUN_RC"
check_contains "malformed -> names the canonical form" "type(scope)!:" "$RUN_OUT"
run_fixture "$FIXTURE" merge
check "malformed + merge -> red" "1" "$RUN_RC"

fixture_meta 'fix(cli): el aviso describe la retirada' \
  '[{"oid":"07e81f8f0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":"Detalle.\n\nBREAKING CHANGE: la feature ai ya no existe"}]'
run_fixture "$FIXTURE" squash
check "footer-only + squash -> red" "1" "$RUN_RC"
check_contains "footer-only -> names the footer rule" "BREAKING CHANGE:" "$RUN_OUT"
run_fixture "$FIXTURE" merge
check "footer-only + merge -> red" "1" "$RUN_RC"

fixture_meta 'feat(ai)!: retirar la feature ai' \
  '[{"oid":"aaaa00000000","messageHeadline":"feat(ai): retirar la feature ai","messageBody":""},{"oid":"bbbb00000000","messageHeadline":"test(ai): actualizar el harness","messageBody":""}]'
run_fixture "$FIXTURE" squash
check "N=2 title marker + squash -> green" "0" "$RUN_RC"
run_fixture "$FIXTURE" merge
check "N=2 title-only + merge -> red" "1" "$RUN_RC"
check_contains "N=2 title-only + merge -> says merge discards the title" "DESCARTA" "$RUN_OUT"

fixture_meta 'feat(ai): retirar la feature ai' \
  '[{"oid":"aaaa00000000","messageHeadline":"feat(ai)!: retirar la feature ai","messageBody":""},{"oid":"bbbb00000000","messageHeadline":"test(ai): actualizar el harness","messageBody":""}]'
run_fixture "$FIXTURE" squash
check "N=2 commit-only + squash -> red" "1" "$RUN_RC"
check_contains "N=2 commit-only -> names the title as the kept surface" "TITULO" "$RUN_OUT"
run_fixture "$FIXTURE" merge
check "N=2 commit-only + merge -> green" "0" "$RUN_RC"

fixture_meta "fix(cli): don't reintentar en 5xx" \
  '[{"oid":"cccc00000000","messageHeadline":"fix(cli): dont reintentar en 5xx","messageBody":"El trailer BREAKING CHANGE: tiene que ir al inicio de linea para contar."}]'
run_fixture "$FIXTURE" squash
check "prose '!' and mid-line footer -> green" "0" "$RUN_RC"

# ─── 3. Fail closed on incomplete/invalid metadata ──────────────────────────
printf 'not-json' > "$FIXTURE"
run_fixture "$FIXTURE" squash
check "invalid JSON -> red" "1" "$RUN_RC"

fixture_meta 'feat(ai)!: retirada' '[]'
run_fixture "$FIXTURE" squash
check "zero commits -> red" "1" "$RUN_RC"

fixture_meta 'feat(ai)!: retirada' \
  '[{"oid":"aaaa00000000","messageHeadline":"feat(ai)!: retirada","messageBody":""}]'
jq 'del(.headRefOid)' "$FIXTURE" > "$FIXTURE.next" && mv "$FIXTURE.next" "$FIXTURE"
run_fixture "$FIXTURE" squash
check "missing headRefOid -> red" "1" "$RUN_RC"

fixture_meta 'feat(ai)!: retirada' \
  '[{"oid":"aaaa00000000","messageHeadline":"feat(ai)!: retirada","messageBody":""}]'
jq '.title = 42' "$FIXTURE" > "$FIXTURE.next" && mv "$FIXTURE.next" "$FIXTURE"
run_fixture "$FIXTURE" squash
check "non-string title -> red" "1" "$RUN_RC"

# ─── 4. The 100-commit cap (#2328 AC) ───────────────────────────────────────
# `gh pr view --json commits` truncates the GraphQL connection at 100. The
# check reads REST with --paginate instead, so a marker on commit 101 must
# change the verdict — here it does, which is the regression pin.
n1='[{"oid":"07e81f8f0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":""}]'
many='['
for i in $(seq 1 100); do
  many+="{\"oid\":\"$(printf 'c%08d' "$i")0000\",\"messageHeadline\":\"chore: fill $i\",\"messageBody\":\"\"},"
done
many+='{"oid":"dddd00000000","messageHeadline":"feat(cli)!: el marcador vive en el commit 101","messageBody":""}]'
fixture_meta 'fix(cli): el aviso describe la retirada' "$many"
run_fixture "$FIXTURE" squash
check "marker on commit 101 + squash -> red (the cap is gone)" "1" "$RUN_RC"
check_contains "commit 101 is named in the evidence" "commit dddd0000" "$RUN_OUT"
run_fixture "$FIXTURE" merge
check "marker on commit 101 + merge -> green (commits survive)" "0" "$RUN_RC"
# And 101 CLEAN commits stay green under squash: the cap fix is about
# visibility, not about punishing large PRs.
clean_many='['
for i in $(seq 1 101); do
  clean_many+="{\"oid\":\"$(printf 'c%08d' "$i")0000\",\"messageHeadline\":\"chore: fill $i\",\"messageBody\":\"\"},"
done
clean_many="${clean_many%,}]"
fixture_meta 'fix(cli): el aviso describe la retirada' "$clean_many"
run_fixture "$FIXTURE" squash
check "101 clean commits + squash -> green" "0" "$RUN_RC"
check_no_grep "the capped read (title,commits) is not used" 'title,commits' \
  "$REPO_ROOT/scripts/check_breaking_surface.sh"
check_grep "the REST read is paginated" '[-]-paginate' \
  "$REPO_ROOT/scripts/check_breaking_surface.sh"
check_grep "the REST read requests full pages" 'per_page=100' \
  "$REPO_ROOT/scripts/check_breaking_surface.sh"

# ─── 5. Production mode: the queue method comes from the tracked mirror ─────
write_mirror() { # write_mirror <method> [<base>] — "" as method drops the main entry
  if [[ -z "$1" ]]; then
    printf '%s' '{"schema_version":1,"queues":[{"base":"release/9.9","method":"SQUASH"}]}' \
      > "$STATE/queue-mirror.json"
  else
    jq -n --arg m "$1" --arg b "${2:-main}" \
      '{schema_version: 1, queues: [{base: $b, method: $m}]}' > "$STATE/queue-mirror.json"
  fi
}
write_pr_meta 'fix(cli)!: marcador solo en el titulo' '1111000000000000000000000000000000000000'
write_rest "[$(rest_commit '07e81f8f0000' 'fix(cli): el aviso describe la retirada')]"
write_mirror SQUASH
run_check
check "SQUASH mirror + N=1 title-only -> red" "1" "$RUN_RC"
check_contains "SQUASH verdict names the commit subject" "SUBJECT del commit" "$RUN_OUT"

write_pr_meta 'chore(release): declarar la ruptura' '1111000000000000000000000000000000000000'
write_rest "[$(rest_commit '437357e70000' 'chore(release)!: declarar la ruptura')]"
run_check
check "SQUASH mirror + N=1 commit marker -> green" "0" "$RUN_RC"

write_mirror MERGE
run_check
check "MERGE mirror + N=1 commit marker -> green" "0" "$RUN_RC"

write_mirror REBASE
write_pr_meta 'fix(cli)!: marcador solo en el titulo' '1111000000000000000000000000000000000000'
write_rest "[$(rest_commit '07e81f8f0000' 'fix(cli): el aviso describe la retirada')]"
run_check
check "REBASE mirror + N=1 title-only -> red (title is dropped too)" "1" "$RUN_RC"
check_contains "REBASE verdict says the merge discards the title" "DESCARTA" "$RUN_OUT"

write_mirror SQUASH "release/9.9"
run_check
check "base absent from the mirror -> red (fail closed)" "1" "$RUN_RC"
check_contains "absent base names the mirror file" "queue-mirror.json" "$RUN_OUT"

rm -f "$STATE/queue-mirror.json"
run_check
check "missing mirror -> red (fail closed)" "1" "$RUN_RC"

printf 'not-json' > "$STATE/queue-mirror.json"
run_check
check "invalid mirror -> red (fail closed)" "1" "$RUN_RC"

write_mirror FAST_FORWARD
run_check
check "unknown method in the mirror -> red (fail closed)" "1" "$RUN_RC"

write_pr_meta 'chore(release): declarar la ruptura' '1111000000000000000000000000000000000000'
write_rest "[$(rest_commit '437357e70000' 'chore(release)!: declarar la ruptura')]"
touch "$STATE/rest-fail"
run_check
check "unreadable commit list -> red (fail closed)" "1" "$RUN_RC"
rm -f "$STATE/rest-fail"
touch "$STATE/rest-empty"
run_check
check "empty commit list -> red (fail closed)" "1" "$RUN_RC"
rm -f "$STATE/rest-empty"

touch "$STATE/pr-meta-fail"
run_check
check "unreadable PR metadata -> red (fail closed)" "1" "$RUN_RC"
rm -f "$STATE/pr-meta-fail"

# ─── 6. The matrix is the SHARED library ────────────────────────────────────
# Structural equivalence pin: both consumers source the same lib, and the
# local guard decides through breaking_verdict. The behavioural pin for the
# local guard lives in test_merge_when_green_breaking.sh.
check_grep "the CI check sources the shared matrix" 'breaking_surface_lib.sh' \
  "$REPO_ROOT/scripts/check_breaking_surface.sh"
check_grep "the local guard sources the shared matrix" 'breaking_surface_lib.sh' \
  "$REPO_ROOT/scripts/merge-when-green.sh"
check_grep "the local guard decides through the verdict function" 'breaking_verdict' \
  "$REPO_ROOT/scripts/merge-when-green.sh"
# The mirror reader is shared too: the check and the guard must consume the
# same file through the same function (#2412), and the check must NOT call
# GraphQL for the queue method (GITHUB_TOKEN is FORBIDDEN there).
check_grep "the check reads the mirror through the shared reader" 'breaking_queue_method_from_mirror' \
  "$REPO_ROOT/scripts/check_breaking_surface.sh"
check_grep "the local guard reads the mirror through the shared reader" 'breaking_queue_method_from_mirror' \
  "$REPO_ROOT/scripts/merge-when-green.sh"
check_no_grep "the check does not read the queue via GraphQL" 'api graphql' \
  "$REPO_ROOT/scripts/check_breaking_surface.sh"

# ─── 7. The workflow wiring ─────────────────────────────────────────────────
WF="$REPO_ROOT/.github/workflows/pr-validation.yml"
check_grep "the breaking-surface job exists" '^  breaking-surface:' "$WF"
check_grep "the required context is the job name" 'name: breaking-surface$' "$WF"
check_grep "the workflow runs on merge_group" '^  merge_group:' "$WF"
check_grep "the job references the guard" 'run: scripts/check_breaking_surface.sh' "$WF"
check_grep "unresolvable PR is a red check" 'No se pudo resolver el PR' "$WF"
check_grep "unopen PR is a red check" 'no está abierto' "$WF"
# The synthetic-ref parse must exist in BOTH resolution steps (validate and
# breaking-surface): two occurrences of the queue-ref regex.
if [[ "$(grep -cF 'gh-readonly-queue/.+/pr-([0-9]+)-' "$WF")" -ge 2 ]]; then
  printf '  OK   both resolution steps parse the synthetic queue ref\n'; PASS=$((PASS + 1))
else
  printf '  FAIL both resolution steps parse the synthetic queue ref\n'; FAIL=$((FAIL + 1))
fi

echo "test_breaking_surface: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: the required check keeps the breaking marker on the surface the queue keeps."
