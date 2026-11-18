#!/usr/bin/env bash
#
# Semantics harness for scripts/prune-release-branches.sh (webfang#2918).
#
# The predicate is behavioral, not textual, so grepping the script proves
# nothing. Every property that matters is asserted against a fixture repo and a
# fake `gh` on PATH: no network, no real branch, no real repository state.
#
#   1  an old branch with no open PR is deleted
#   2  a branch owned by an OPEN PR survives even when old (load-bearing)
#   3  a branch younger than the minimum age survives (release-plz push→PR race)
#   4  names outside the strict pattern are never even inspected
#   5  250 branches are all considered — the 191-branch case, and the reason
#      `--paginate` is not optional (the fake refuses to answer without it)
#   6  --dry-run lists candidates and deletes nothing
#   7  a failed branch listing is fail-closed: non-zero, zero deletions
#   8  a failed pull-request listing is fail-closed: non-zero, zero deletions
#   9  no candidates is a no-op that exits 0 (idempotent)
#  10  one failed delete is reported as non-zero and does NOT stop the rest
#  11  a branch that vanished between the two phases is skipped, not a failure
#  12  --min-age-hours moves the threshold in both directions
#  13  an invalid --min-age-hours is rejected before any API call

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$REPO_ROOT/scripts/prune-release-branches.sh"
unset GITHUB_STEP_SUMMARY

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
mkdir -p "$BIN"

# ─── Fake `gh` ───────────────────────────────────────────────────────────────
# Models exactly the four invocations the script makes, and nothing else: any
# other endpoint, flag or --jq expression exits 99 so the harness fails loudly
# instead of silently agreeing with a contract the real gh does not have.
#
# `emit_list` refuses to answer a >100-entry fixture without --paginate. That
# is the 191-branch lesson encoded as a test: a sweep that reads only page one
# would look green and delete a fifth of the problem.
cat >"$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail

state="$FAKE_STATE"

[ "${1:-}" = "api" ] || { echo "fake gh: expected 'api', got: $*" >&2; exit 99; }
shift

paginate=0
jq_expr=""
method="GET"
endpoint=""
while [ $# -gt 0 ]; do
  case "$1" in
    --paginate) paginate=1 ;;
    --jq)
      jq_expr="${2:-}"
      shift
      ;;
    --method)
      method="${2:-}"
      shift
      ;;
    -*)
      echo "fake gh: unhandled flag: $1" >&2
      exit 99
      ;;
    *)
      endpoint="$1"
      ;;
  esac
  shift
done

echo "$method $endpoint" >>"$state/calls"

emit_list() {
  # $1 = fixture file, $2 = the only --jq expression this endpoint accepts
  local file="$1" want_jq="$2" count
  count=$(wc -l <"$file")
  if [ "$count" -gt 100 ] && [ "$paginate" -eq 0 ]; then
    echo "fake gh: $endpoint has $count entries but --paginate was not passed" >&2
    exit 99
  fi
  if [ "$jq_expr" != "$want_jq" ]; then
    echo "fake gh: $endpoint got --jq '$jq_expr', expected '$want_jq'" >&2
    exit 99
  fi
  cat "$file"
}

case "$endpoint" in
  */branches?per_page=100)
    if [ -f "$state/fail_branches" ]; then
      echo "gh: Forbidden (HTTP 403)" >&2
      exit 1
    fi
    emit_list "$state/branches" '.[].name'
    ;;
  */pulls?state=open\&per_page=100)
    if [ -f "$state/fail_pulls" ]; then
      echo "gh: Forbidden (HTTP 403)" >&2
      exit 1
    fi
    emit_list "$state/open_prs" '.[].head.ref'
    ;;
  */git/refs/heads/*)
    if [ "$method" != "DELETE" ]; then
      echo "fake gh: unexpected method $method for $endpoint" >&2
      exit 99
    fi
    name="${endpoint#*/git/refs/heads/}"
    if [ -f "$state/fail_delete" ] && grep -Fxq -- "$name" "$state/fail_delete"; then
      echo "gh: Unprocessable Entity (HTTP 422)" >&2
      exit 1
    fi
    printf '%s\n' "$name" >>"$state/deleted"
    ;;
  */branches/*)
    name="${endpoint#*/branches/}"
    key="$(printf '%s' "$name" | tr '/' '~')"
    if [ ! -f "$state/dates/$key" ]; then
      echo "gh: Not Found (HTTP 404)" >&2
      exit 1
    fi
    if [ "$jq_expr" != '.commit.commit.committer.date' ]; then
      echo "fake gh: unexpected --jq '$jq_expr'" >&2
      exit 99
    fi
    cat "$state/dates/$key"
    ;;
  *)
    echo "fake gh: unhandled endpoint: $endpoint" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

failures=0
ok() { printf 'OK: %s\n' "$1"; }
bad() {
  printf 'FAIL: %s\n' "$1"
  failures=$((failures + 1))
}

new_state() {
  local d
  d="$WORK/$1"
  mkdir -p "$d/dates"
  : >"$d/branches"
  : >"$d/open_prs"
  printf '%s' "$d"
}

key() { printf '%s' "$1" | tr '/' '~'; }

# add_branch <state> <branch> <date -d expression>
add_branch() {
  printf '%s\n' "$2" >>"$1/branches"
  date -u -d "$3" +%Y-%m-%dT%H:%M:%SZ >"$1/dates/$(key "$2")"
}

OUT=""
RC=0
run_script() {
  local state="$1"
  shift
  OUT="$(PATH="$BIN:$PATH" FAKE_STATE="$state" bash "$SCRIPT" --repo acme/widgets "$@" 2>&1)"
  RC=$?
}

deleted_list() {
  if [ -f "$1/deleted" ]; then
    sort "$1/deleted"
  fi
}

printf '== prune-release-branches (%s)\n' "$SCRIPT"

# ─── 1. an old branch with no open PR is deleted ─────────────────────────────
s="$(new_state t1)"
add_branch "$s" 'chore/release-2026-11-17T18-08-10Z' '3 days ago'
run_script "$s"
if [ "$RC" -eq 0 ] && [ "$(deleted_list "$s")" = 'chore/release-2026-11-17T18-08-10Z' ]; then
  ok "1 old orphan is deleted"
else
  bad "1 old orphan is deleted (rc=$RC)"
fi

# ─── 2. an open PR protects its branch ───────────────────────────────────────
s="$(new_state t2)"
add_branch "$s" 'chore/release-2026-11-17T18-08-10Z' '3 days ago'
printf '%s\n' 'chore/release-2026-11-17T18-08-10Z' >"$s/open_prs"
run_script "$s"
if [ "$RC" -eq 0 ] && [ ! -f "$s/deleted" ]; then
  ok "2 branch of an open PR survives"
else
  bad "2 branch of an open PR survives (rc=$RC)"
fi

# ─── 3. the push→PR race is closed by the age floor ──────────────────────────
s="$(new_state t3)"
add_branch "$s" 'chore/release-2026-11-18T03-00-00Z' '1 hour ago'
run_script "$s"
if [ "$RC" -eq 0 ] && [ ! -f "$s/deleted" ]; then
  ok "3 branch younger than 24h survives"
else
  bad "3 branch younger than 24h survives (rc=$RC)"
fi

# ─── 4. names outside the strict pattern are never inspected ─────────────────
# The assertion is that no per-branch detail call happens, not merely that
# nothing is deleted: a 404 on the detail call would also delete nothing, and
# that would prove nothing about the pattern.
s="$(new_state t4)"
for n in main master gh-pages support/1.0 release/1.0 hotfix/1.0.1 \
  docs/0021-proxy ci/foo 'chore/release-2026-11-17' \
  'chore/release-2026-11-17T18-08-10' 'chore/release-2026-11-17T18:08:10Z'; do
  printf '%s\n' "$n" >>"$s/branches"
done
run_script "$s"
inspected="$(grep -c 'GET repos/acme/widgets/branches/[^?]*$' "$s/calls" 2>/dev/null || true)"
if [ "$RC" -eq 0 ] && [ "$inspected" = "0" ] && [ ! -f "$s/deleted" ]; then
  ok "4 non-matching names are never inspected"
else
  bad "4 non-matching names are never inspected (rc=$RC, inspected=$inspected)"
fi

# ─── 5. pagination: 250 branches, the shape of the 191-branch incident ───────
s="$(new_state t5)"
i=0
while [ "$i" -lt 250 ]; do
  mm="$(printf '%02d' $((i / 28 + 1)))"
  dd="$(printf '%02d' $((i % 28 + 1)))"
  add_branch "$s" "chore/release-2026-${mm}-${dd}T00-00-00Z" '3 days ago'
  i=$((i + 1))
done
run_script "$s"
if [ "$RC" -eq 0 ] && [ "$(wc -l <"$s/deleted")" -eq 250 ]; then
  ok "5 all 250 branches across pages are considered"
else
  bad "5 all 250 branches across pages are considered (rc=$RC, deleted=$(wc -l <"$s/deleted" 2>/dev/null || echo 0))"
fi

# ─── 6. --dry-run deletes nothing ────────────────────────────────────────────
s="$(new_state t6)"
add_branch "$s" 'chore/release-2026-11-17T18-08-10Z' '3 days ago'
run_script "$s" --dry-run
if [ "$RC" -eq 0 ] && [ ! -f "$s/deleted" ] && printf '%s' "$OUT" | grep -q 'dry-run: would delete 1 branch'; then
  ok "6 --dry-run lists and deletes nothing"
else
  bad "6 --dry-run lists and deletes nothing (rc=$RC)"
fi

# ─── 7. failed branch listing is fail-closed ─────────────────────────────────
s="$(new_state t7)"
add_branch "$s" 'chore/release-2026-11-17T18-08-10Z' '3 days ago'
: >"$s/fail_branches"
run_script "$s"
if [ "$RC" -ne 0 ] && [ ! -f "$s/deleted" ]; then
  ok "7 failed branch listing deletes nothing and exits non-zero"
else
  bad "7 failed branch listing deletes nothing and exits non-zero (rc=$RC)"
fi

# ─── 8. failed PR listing is fail-closed ─────────────────────────────────────
s="$(new_state t8)"
add_branch "$s" 'chore/release-2026-11-17T18-08-10Z' '3 days ago'
: >"$s/fail_pulls"
run_script "$s"
if [ "$RC" -ne 0 ] && [ ! -f "$s/deleted" ]; then
  ok "8 failed PR listing deletes nothing and exits non-zero"
else
  bad "8 failed PR listing deletes nothing and exits non-zero (rc=$RC)"
fi

# ─── 9. no candidates is a no-op ─────────────────────────────────────────────
s="$(new_state t9)"
run_script "$s"
if [ "$RC" -eq 0 ] && [ ! -f "$s/deleted" ]; then
  ok "9 no candidates is a green no-op"
else
  bad "9 no candidates is a green no-op (rc=$RC)"
fi

# ─── 10. one failed delete does not stop the rest ────────────────────────────
s="$(new_state t10)"
add_branch "$s" 'chore/release-2026-11-17T18-08-10Z' '3 days ago'
add_branch "$s" 'chore/release-2026-11-16T18-08-10Z' '3 days ago'
printf '%s\n' 'chore/release-2026-11-17T18-08-10Z' >"$s/fail_delete"
run_script "$s"
if [ "$RC" -ne 0 ] && [ "$(deleted_list "$s")" = 'chore/release-2026-11-16T18-08-10Z' ]; then
  ok "10 a failed delete is reported and the rest proceed"
else
  bad "10 a failed delete is reported and the rest proceed (rc=$RC, deleted=$(deleted_list "$s"))"
fi

# ─── 11. a branch that vanished between phases is not a failure ──────────────
s="$(new_state t11)"
add_branch "$s" 'chore/release-2026-11-17T18-08-10Z' '3 days ago'
add_branch "$s" 'chore/release-2026-11-16T18-08-10Z' '3 days ago'
rm -f "$s/dates/$(key 'chore/release-2026-11-17T18-08-10Z')"
run_script "$s"
if [ "$RC" -eq 0 ] && [ "$(deleted_list "$s")" = 'chore/release-2026-11-16T18-08-10Z' ]; then
  ok "11 a branch that vanished is skipped, not fatal"
else
  bad "11 a branch that vanished is skipped, not fatal (rc=$RC, deleted=$(deleted_list "$s"))"
fi

# ─── 12. --min-age-hours moves the threshold both ways ───────────────────────
s="$(new_state t12)"
add_branch "$s" 'chore/release-2026-11-18T02-00-00Z' '2 hours ago'
run_script "$s" --min-age-hours 1
first=$RC
first_deleted="$([ -f "$s/deleted" ] && echo yes || echo no)"
s2="$(new_state t12b)"
add_branch "$s2" 'chore/release-2026-11-18T02-00-00Z' '2 hours ago'
run_script "$s2" --min-age-hours 5
second=$RC
second_deleted="$([ -f "$s2/deleted" ] && echo yes || echo no)"
if [ "$first" -eq 0 ] && [ "$first_deleted" = yes ] && [ "$second" -eq 0 ] && [ "$second_deleted" = no ]; then
  ok "12 --min-age-hours moves the threshold both ways"
else
  bad "12 --min-age-hours moves the threshold both ways (1h:$first/$first_deleted 5h:$second/$second_deleted)"
fi

# ─── 13. an invalid threshold is rejected before any API call ────────────────
s="$(new_state t13)"
run_script "$s" --min-age-hours soon
if [ "$RC" -eq 2 ] && [ ! -f "$s/calls" ]; then
  ok "13 an invalid --min-age-hours exits 2 before touching the API"
else
  bad "13 an invalid --min-age-hours exits 2 before touching the API (rc=$RC)"
fi

printf '\n== %s failure(s)\n' "$failures"
[ "$failures" -eq 0 ]