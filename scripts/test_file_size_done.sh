#!/usr/bin/env bash
# test_file_size_done.sh -- semantics harness for file_size_done.sh, the
# close-out tool of the file-size policy (umbrella #2584, phase F1b; ODD
# odd/tasks/file-size-done.md).
#
# WHAT THIS HARNESS PROVES (1..10, mirroring the sections):
#   1. the tool under test exists (a missing tool is a named RED, never silence)
#   2. the usage contract: 64 (EX_USAGE) on malformed input, and --help
#      documents EVERY exit code of the closed map (0/1/2/64 -- the
#      per-tool-map contract, issue #2928)
#   3. environment failures fail CLOSED with a named cause (rc 1), never
#      silently green
#   4. one green close (rc 0): a plain file at/below the limit, a grandfathered
#      descent (--remove done), an exception already present at the base
#      commit, a split whose parts all measure <= hard_limit, and a
#      pre-existing allow passing (the multiset preserved)
#   5. parked (rc 2): over the limit and not in exceptions at the base commit,
#      a remove-pending descent, an over-limit part; the parked output carries
#      reason, date and base commit (policy 7); plus the REAL tree's
#      wreq_downloader.rs parked case with the count MEASURED, never written
#   6. one defect per exit-1 discriminator (policy 11 F1b): hard_limit changed,
#      an exceptions entry added, removed (emptied), value-emptied/changed, a
#      net-new allow, a grandfathered entry removed while its file is still
#      over the limit -- and the NEGATIVES that keep the map honest (a
#      legitimate descent, a MOVED allow)
#   7. hermeticity: a close run writes NOTHING into the measured tree
#   8. --list is informational and says so ("not a guard mode", policy 4.6)
#   9. the emitted keys are asserted against a CLOSED ENGLISH LIST fed by real
#      outputs of every branch, with the source-side emit-site lock (v3.3
#      lesson: the contract is enumerated, not detected)
#  10. the harness watches its own house: --negation proves it can fail
#      (#2056: a harness that cannot fail proves nothing), the family lint
#      (no `grep -q` under pipes, issue #2925), and the live wiring lock
#      (.tsv note + ci.yml + ci_fast_gate.sh name both scripts -- #2056: a
#      guard reachable from no workflow is a guard that does not exist)
#
# HERMETIC: every scenario runs in a mktemp sandbox that holds a COPY of the
# tool and the guard; the sandboxes carry a real git repo (one base commit)
# because reading the base commit IS the tool's contract. Nothing outside
# $WORK is written. The three live-tree checks (5.11-5.14, 8.x, 9.x feed) are
# read-only.
#
# NEGATION: `bash scripts/test_file_size_done.sh --negation` flips one
# expectation so the harness FAILS on a correct tree -- the proof that it can
# fail (#2056).
#
# Usage: bash scripts/test_file_size_done.sh [--negation]

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$SCRIPT_DIR/.."
DONE="$SCRIPT_DIR/file_size_done.sh"
GUARD="$SCRIPT_DIR/check_file_size.sh"
TSV="$REPO_ROOT/scripts/ci_fast_gate_coverage.tsv"
CI_YML="$REPO_ROOT/.github/workflows/ci.yml"
FAST_GATE="$REPO_ROOT/scripts/ci_fast_gate.sh"

NEGATION=0
while [ $# -gt 0 ]; do
  case "$1" in
    --negation) NEGATION=1 ;;
    -h | --help)
      sed -n '2,42p' "$0"
      exit 0
      ;;
    *)
      printf 'test_file_size_done: unknown argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
  shift
done

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0
ERRN=0

ok() {
  printf '  ok   %-62s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-62s %s\n' "$1" "$2"
  if [ -n "${3:-}" ] && [ -f "$3" ]; then
    sed 's/^/      /' "$3" | head -20
  elif [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/      /' | head -20
  fi
  failed=$((failed + 1))
}

expect_rc() {  # <desc> <expected> <real> [detail]
  if [ "$2" -eq "$3" ]; then
    ok "$1" "rc=$3"
  else
    bad "$1" "expected rc=$2, got rc=$3" "${4:-${ERRF:-}}"
  fi
}

expect_eq() {  # <desc> <real> <expected>
  if [ "$2" = "$3" ]; then
    ok "$1" "$2"
  else
    bad "$1" "expected '$3', got '$2'"
  fi
}

expect_match() {  # <desc> <text> <regex>  -- full consumption, NEVER grep -q
                  # (issue #2925: -q early-exits and SIGPIPEs the producer
                  # under pipefail; the assertion fails despite holding)
  if printf '%s\n' "$2" | grep -E -- "$3" > /dev/null; then
    ok "$1"
  else
    bad "$1" "no match: $3" "$2"
  fi
}

# field <output> <key>: field 2 of the first row keyed <key> (emit2 rows:
# base, descent, parked_date, defect causes).
field() {
  printf '%s\n' "$1" | awk -F'\t' -v k="$2" '$1 == k { print $2; exit }'
}

# fv <output> <key> <file>: field 3 of the first row keyed <key> whose file
# column is <file> (per-file rows: file, lines, state, cap, verdict, reason).
fv() {
  printf '%s\n' "$1" | awk -F'\t' -v k="$2" -v f="$3" '$1 == k && $2 == f { print $3; exit }'
}

# --- sandboxes: a copy of the tool + the guard, and a real one-commit git repo.

SB=""
BASE_SHA=""
ERRF=""

new_sandbox() {
  SB="$WORK/$1"
  ERRN=$((ERRN + 1))
  ERRF="$WORK/err.$ERRN"
  mkdir -p "$SB/scripts" "$SB/crates/fx/src"
  cp "$DONE" "$SB/scripts/file_size_done.sh"
  cp "$GUARD" "$SB/scripts/check_file_size.sh"
  git -C "$SB" init -q
  git -C "$SB" config user.email done-harness@example
  git -C "$SB" config user.name done-harness
  git -C "$SB" config commit.gpgsign false
}

# write_baseline <limit> <path=count>... [exc:<path>=<reason>...]
write_baseline() {
  python3 -I - "$SB/scripts/quality-baselines.json" "$@" <<'PY'
import json
import sys

path = sys.argv[1]
limit = int(sys.argv[2])
gf = {}
exc = {}
for arg in sys.argv[3:]:
    if arg.startswith("exc:"):
        pair = arg[len("exc:"):]
        key, _, why = pair.partition("=")
        exc[key] = why
    else:
        key, _, count = arg.partition("=")
        gf[key] = int(count)
data = {
    "file_size_rust": {
        "hard_limit": limit,
        "grandfathered": gf,
        "exceptions": exc,
    }
}
with open(path, "w") as fh:
    fh.write(json.dumps(data, indent=2))
PY
}

# write_code <rel> <n> [extra-line...]: n one-line fns, then verbatim extras
# (allow attributes and the like).
write_code() {
  local rel="$1" n="$2"
  shift 2
  {
    for ((i = 1; i <= n; i++)); do
      printf 'pub fn f%d() {}\n' "$i"
    done
    for line in "$@"; do
      printf '%s\n' "$line"
    done
  } > "$SB/$rel"
}

snap_base() {
  git -C "$SB" add -A
  git -C "$SB" commit -qm "base"
  BASE_SHA="$(git -C "$SB" rev-parse HEAD)"
}

run_done() {
  OUT="$(cd "$SB" && bash scripts/file_size_done.sh "$@" 2>"$ERRF")"
  RC=$?
}

# --- 1. the tool under test exists ---------------------------------------------
# NEGATION: without this precheck a missing tool would fail as 127 in every
# later section -- noise, not a named verdict.

if [ ! -f "$DONE" ]; then
  printf '  FAIL %-62s %s\n' "1.1 the tool under test exists" "file_size_done.sh not written yet (expected RED)"
  exit 1
fi
ok "1.1 the tool under test exists"

# --- 2. the usage contract (rc 64, EX_USAGE) ------------------------------------
# NEGATION: without it the tool could answer garbage input with a verdict
# instead of refusing it, and a typo would read as a mission decision.

new_sandbox u1
write_baseline 500 "crates/fx/src/a.rs=520"
write_code "crates/fx/src/a.rs" 520
snap_base

run_done
expect_rc "2.1 no arguments is usage (64)" 64 "$RC" "$ERRF"
run_done --frobnicate
expect_rc "2.2 an unknown flag is usage (64)" 64 "$RC" "$ERRF"
run_done --base
expect_rc "2.3 --base without a value is usage (64)" 64 "$RC" "$ERRF"
run_done --base "deadbeef" "crates/fx/src/a.rs"
expect_rc "2.4 an unresolvable --base is usage (64)" 64 "$RC" "$ERRF"
run_done --base "$BASE_SHA" "crates/fx/src/nope.rs"
expect_rc "2.5 a missing target file is usage (64)" 64 "$RC" "$ERRF"
printf '# sandbox\n' > "$SB/README.md"
run_done --base "$BASE_SHA" "README.md"
expect_rc "2.6 a path outside the corpus is usage (64)" 64 "$RC" "$ERRF"

OUT="$(bash "$DONE" --help 2>/dev/null)"
HELP_RC=$?
expect_rc "2.7 --help exits 0" 0 "$HELP_RC"
expect_match "2.8 --help documents 0 (closed)" "$OUT" '0 +closed'
expect_match "2.9 --help documents 1 (defect)" "$OUT" '1 +defect'
expect_match "2.10 --help documents 2 (parked)" "$OUT" '2 +parked'
expect_match "2.11 --help documents 64 (usage)" "$OUT" '64 +usage'

# --- 3. environment failures fail closed (rc 1, named cause) ---------------------
# NEGATION: an unreadable baseline that exited 0 would close missions on a
# broken measurement.

new_sandbox e1
write_code "crates/fx/src/a.rs" 10
snap_base
run_done --base "$BASE_SHA" "crates/fx/src/a.rs"
expect_rc "3.1 no baseline at all fails closed (1)" 1 "$RC" "$ERRF"

new_sandbox e2
write_baseline 500 "crates/fx/src/a.rs=520"
write_code "crates/fx/src/a.rs" 520
snap_base
printf 'not json at all' > "$SB/scripts/quality-baselines.json"
run_done --base "$BASE_SHA" "crates/fx/src/a.rs"
expect_rc "3.2 a corrupt current baseline fails closed (1)" 1 "$RC" "$ERRF"

new_sandbox e3
write_baseline 500 "crates/fx/src/a.rs=520"
write_code "crates/fx/src/a.rs" 520
rm -rf "$SB/.git"
run_done "crates/fx/src/a.rs"
expect_rc "3.3 no git repo and no --base fails closed (1)" 1 "$RC" "$ERRF"

new_sandbox e4
write_code "crates/fx/src/a.rs" 520
snap_base
write_baseline 500 "crates/fx/src/a.rs=520"
run_done --base "$BASE_SHA" "crates/fx/src/a.rs"
expect_rc "3.4 a baseline the base commit does not have fails closed (1)" 1 "$RC" "$ERRF"

# --- 4. one green close (rc 0) ---------------------------------------------------
# NEGATION: a close-out that answers 0 without measuring would bless unfinished
# missions -- the exact failure mode the policy was written for.

new_sandbox g1
write_baseline 500 "crates/fx/src/big.rs=700"
write_code "crates/fx/src/big.rs" 480
write_code "crates/fx/src/plain.rs" 120
snap_base

run_done --base "$BASE_SHA" "crates/fx/src/plain.rs"
expect_rc "4.1 a plain file at/below the limit closes (0)" 0 "$RC" "$ERRF"
GREEN_OUT="$OUT"
# the negation flip: the same healthy tree must FAIL under --negation
if [ "$NEGATION" -eq 1 ]; then
  if [ "$RC" -eq 0 ]; then
    bad "4.1n negation flip" "the harness passed a correct tree with the expectation inverted -- it cannot fail (#2056)"
  else
    ok "4.1n negation flip" "the inverted expectation failed as required"
  fi
fi
expect_eq "4.2 the plain state is measured" "$(fv "$OUT" state "crates/fx/src/plain.rs")" "plain"
expect_match "4.3 lines carry file, unit and command" "$OUT" $'lines\tcrates/fx/src/plain\.rs\t120\tlines\t'
expect_match "4.4 the verdict names the file" "$OUT" $'verdict\tcrates/fx/src/plain\.rs\tclosed'

new_sandbox g2
write_baseline 500 "crates/fx/src/big.rs=700"
write_code "crates/fx/src/big.rs" 480
snap_base
# the mission: shrank below the limit AND removed the baseline entry
write_baseline 500
run_done --base "$BASE_SHA" "crates/fx/src/big.rs"
expect_rc "4.5 a grandfathered descent with --remove done closes (0)" 0 "$RC" "$ERRF" "$ERRF"
expect_match "4.6 the descent is reported" "$OUT" $'descent\tcrates/fx/src/big\.rs'
DESCENT_OUT="$OUT"

new_sandbox g3
write_baseline 500 "crates/fx/src/kept.rs=900" "exc:crates/fx/src/kept.rs=a single declarative unit, argued in the baseline"
write_code "crates/fx/src/kept.rs" 900
snap_base
run_done --base "$BASE_SHA" "crates/fx/src/kept.rs"
expect_rc "4.7 an exception present at the base commit closes (0)" 0 "$RC" "$ERRF"
expect_eq "4.8 the exception state" "$(fv "$OUT" state "crates/fx/src/kept.rs")" "exception"

new_sandbox g4
write_baseline 500 "crates/fx/src/hub.rs=900"
write_code "crates/fx/src/hub.rs" 900
snap_base
# the mission: the hub shrinks to 300 and two parts are born at 150 each;
# the baseline entry goes with the win (--remove done)
write_code "crates/fx/src/hub.rs" 300
write_code "crates/fx/src/part_one.rs" 150
write_code "crates/fx/src/part_two.rs" 150
write_baseline 500
run_done --base "$BASE_SHA" "crates/fx/src/hub.rs" "crates/fx/src/part_one.rs" "crates/fx/src/part_two.rs"
expect_rc "4.9 a split whose parts all fit closes (0)" 0 "$RC" "$ERRF"
VERDICT_N="$(printf '%s\n' "$OUT" | grep -c $'^verdict\t')" || VERDICT_N=0
expect_eq "4.10 every named file got a verdict" "$VERDICT_N" "3"

new_sandbox g5
write_baseline 500
write_code "crates/fx/src/withallow.rs" 600 "#[allow(clippy::too_many_lines)]" "#![allow(dead_code)]"
snap_base
# a plain (never grandfathered) file: the allow existed at base and survives
# unchanged -- the pre-existing allow passes (F3's fetch_inner criterion)
write_code "crates/fx/src/withallow.rs" 400 "#[allow(clippy::too_many_lines)]" "#![allow(dead_code)]"
run_done --base "$BASE_SHA" "crates/fx/src/withallow.rs"
expect_rc "4.11 a pre-existing allow passes (multiset preserved)" 0 "$RC" "$ERRF" "$ERRF"

# --- 5. parked (rc 2), with reason, date and base commit (policy 7) --------------
# NEGATION: a mission left half-done that read as closed would defeat the whole
# program; parked is the honest third answer.

new_sandbox p1
write_baseline 500 "crates/fx/src/big.rs=700"
write_code "crates/fx/src/big.rs" 650
snap_base
run_done --base "$BASE_SHA" "crates/fx/src/big.rs"
expect_rc "5.1 over the limit, not in exceptions: parked (2)" 2 "$RC" "$ERRF"
PARKED_OUT="$OUT"
expect_eq "5.2 the grandfathered state" "$(fv "$OUT" state "crates/fx/src/big.rs")" "grandfathered"
expect_eq "5.3 the baseline cap is measured" "$(fv "$OUT" cap "crates/fx/src/big.rs")" "700"
expect_match "5.4 the parked reason carries the measured count" "$(fv "$OUT" reason "crates/fx/src/big.rs")" '650'
expect_match "5.5 the base commit is in the output" "$OUT" $'base\t'
expect_match "5.6 the parked date is an ISO-8601 UTC stamp" "$(field "$OUT" parked_date)" '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$'

new_sandbox p2
write_baseline 500 "crates/fx/src/big.rs=700"
write_code "crates/fx/src/big.rs" 480
snap_base
# shrank below the limit but forgot --remove: the guard is RED right now
run_done --base "$BASE_SHA" "crates/fx/src/big.rs"
expect_rc "5.7 remove pending is parked, not closed (2)" 2 "$RC" "$ERRF"
expect_match "5.8 the reason names the pending removal" "$(fv "$OUT" reason "crates/fx/src/big.rs")" 'remove pending'

new_sandbox p3
write_baseline 500 "crates/fx/src/hub.rs=900"
write_code "crates/fx/src/hub.rs" 300
write_code "crates/fx/src/fat_part.rs" 520
snap_base
run_done --base "$BASE_SHA" "crates/fx/src/hub.rs" "crates/fx/src/fat_part.rs"
expect_rc "5.9 a part still over the limit parks the mission (2)" 2 "$RC" "$ERRF"
expect_eq "5.10 the parked part is named" "$(fv "$OUT" verdict "crates/fx/src/fat_part.rs")" "parked"

# 5.11 the REAL tree's parked case (policy 11 F1b: "one parked
# (wreq_downloader, 764)"): read-only, base = HEAD (an empty diff), and the
# count is ASSERTED EQUAL TO THE MEASURED wc -l -- never a written constant
# (the 764 of the policy text was its writing-day figure; the ratchet moves).
REAL_REL="crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs"
REAL_N="$(wc -l < "$REPO_ROOT/$REAL_REL")"
ERRN=$((ERRN + 1))
ERRF="$WORK/err.real"
OUT="$(bash "$DONE" --base HEAD "$REAL_REL" 2>"$ERRF")"
RC=$?
expect_rc "5.11 the real wreq_downloader parks (2)" 2 "$RC" "$ERRF"
expect_eq "5.12 the emitted count equals the measured wc -l" "$(fv "$OUT" lines "$REAL_REL")" "$REAL_N"
expect_eq "5.13 the real state is grandfathered" "$(fv "$OUT" state "$REAL_REL")" "grandfathered"
expect_eq "5.14 the real verdict is parked" "$(fv "$OUT" verdict "$REAL_REL")" "parked"

# --- 6. one defect per exit-1 discriminator (policy 11 F1b) ----------------------
# NEGATION: a defect that read as parked would hide a governance violation
# (a mission granting itself an exception is the exact abuse the base-commit
# read exists to stop).

new_sandbox d1
write_baseline 500 "crates/fx/src/a.rs=520"
write_code "crates/fx/src/a.rs" 520
snap_base
# the mission's diff changed hard_limit: 500 -> 501
write_baseline 501 "crates/fx/src/a.rs=520"
run_done --base "$BASE_SHA" "crates/fx/src/a.rs"
expect_rc "6.1 a changed hard_limit is a defect (1)" 1 "$RC" "$ERRF"
expect_eq "6.2 the cause names the limit change" "$(field "$OUT" defect)" "hard-limit-changed"

new_sandbox d2
write_baseline 500 "crates/fx/src/a.rs=520"
write_code "crates/fx/src/a.rs" 520
snap_base
# the mission added an exceptions entry for itself
write_baseline 500 "crates/fx/src/a.rs=520" "exc:crates/fx/src/a.rs=self-declared inside the mission"
run_done --base "$BASE_SHA" "crates/fx/src/a.rs"
expect_rc "6.3 an added exceptions entry is a defect (1)" 1 "$RC" "$ERRF"
DEFECT_OUT="$OUT"
expect_eq "6.4 the cause names the added entry" "$(field "$OUT" defect)" "exceptions-added"

new_sandbox d3
write_baseline 500 "crates/fx/src/a.rs=520" "exc:crates/fx/src/kept.rs=argued before the mission"
write_code "crates/fx/src/a.rs" 520
write_code "crates/fx/src/kept.rs" 900
snap_base
# the mission emptied the exceptions map
write_baseline 500 "crates/fx/src/a.rs=520"
run_done --base "$BASE_SHA" "crates/fx/src/a.rs"
expect_rc "6.5 a removed exceptions entry is a defect (1)" 1 "$RC" "$ERRF"
expect_eq "6.6 the cause names the removed entry" "$(field "$OUT" defect)" "exceptions-removed"

new_sandbox d4
write_baseline 500 "crates/fx/src/kept.rs=900" "exc:crates/fx/src/kept.rs=argued before the mission"
write_code "crates/fx/src/kept.rs" 900
snap_base
# the entry survives but its WHY was emptied: an exception without an argument
write_baseline 500 "crates/fx/src/kept.rs=900" "exc:crates/fx/src/kept.rs="
run_done --base "$BASE_SHA" "crates/fx/src/kept.rs"
expect_rc "6.7 an emptied exceptions value is a defect (1)" 1 "$RC" "$ERRF"
expect_eq "6.8 the cause names the emptied entry" "$(field "$OUT" defect)" "exceptions-value-changed"

new_sandbox d5
write_baseline 500 "crates/fx/src/a.rs=520"
write_code "crates/fx/src/a.rs" 520
snap_base
# the mission added a suppression that did not exist at base
write_code "crates/fx/src/a.rs" 520 "#[allow(clippy::cognitive_complexity)]"
run_done --base "$BASE_SHA" "crates/fx/src/a.rs"
expect_rc "6.9 a net-new allow is a defect (1)" 1 "$RC" "$ERRF"
expect_eq "6.10 the cause names the added allow" "$(field "$OUT" defect)" "allow-added"

new_sandbox d6
write_baseline 500 "crates/fx/src/a.rs=520"
write_code "crates/fx/src/a.rs" 520
snap_base
# the entry was removed while the file STILL measures over the limit
write_baseline 500
run_done --base "$BASE_SHA" "crates/fx/src/a.rs"
expect_rc "6.11 the removal with the file over the limit is a defect (1)" 1 "$RC" "$ERRF"
expect_eq "6.12 the cause names the over-limit removal" "$(field "$OUT" defect)" "grandfathered-removed-over-limit"

new_sandbox d6c
write_baseline 500 "crates/fx/src/ok.rs=520"
write_code "crates/fx/src/ok.rs" 480
snap_base
# the entry's file legitimately went under the limit: removal is the win
write_baseline 500
run_done --base "$BASE_SHA" "crates/fx/src/ok.rs"
expect_rc "6.13 a removal whose file landed at/below the limit is the descent (0)" 0 "$RC" "$ERRF" "$ERRF"
expect_match "6.14 the descent is reported for the legitimate removal" "$OUT" $'descent\tcrates/fx/src/ok\.rs'

new_sandbox d6t
write_baseline 500 "crates/fx/src/mod_tests.rs=520"
write_code "crates/fx/src/mod_tests.rs" 520
snap_base
# the tests-majority split: the file MOVES OUT of the corpus (into tests/) and
# the entry goes with it -- the removal is the guard's own orphan rule, not a
# defect
mkdir -p "$SB/crates/fx/tests"
write_code "crates/fx/tests/mod_tests.rs" 520
rm "$SB/crates/fx/src/mod_tests.rs"
write_code "crates/fx/src/part_one.rs" 100
write_baseline 500
run_done --base "$BASE_SHA" "crates/fx/src/part_one.rs"
expect_rc "6.17 a removal whose file left the corpus is not a defect" 0 "$RC" "$ERRF" "$ERRF"

new_sandbox d7
write_baseline 500 "crates/fx/src/hub.rs=900"
write_code "crates/fx/src/hub.rs" 900 "#[allow(clippy::result_large_err)]"
snap_base
# the allow MOVES with the code: the source loses it, the part carries it
write_code "crates/fx/src/hub.rs" 300
write_code "crates/fx/src/part.rs" 600 "#[allow(clippy::result_large_err)]"
run_done --base "$BASE_SHA" "crates/fx/src/hub.rs" "crates/fx/src/part.rs"
expect_rc "6.15 a moved allow is not a net addition (multiset preserved)" 2 "$RC" "$ERRF" "$ERRF"
DEFECT_N="$(printf '%s\n' "$OUT" | grep -c $'^defect\t')" || DEFECT_N=0
expect_eq "6.16 no allow defect was raised" "$DEFECT_N" "0"

# --- 7. hermeticity ---------------------------------------------------------------
# NEGATION: a close-out that wrote into the tree would mutate the state it
# judges.

new_sandbox h1
write_baseline 500 "crates/fx/src/big.rs=700"
write_code "crates/fx/src/big.rs" 650
snap_base
BEFORE="$(cd "$SB" && find . -type f -not -path './.git/*' -print0 | sort -z | xargs -0 sha256sum | sha256sum)"
run_done --base "$BASE_SHA" "crates/fx/src/big.rs"
AFTER="$(cd "$SB" && find . -type f -not -path './.git/*' -print0 | sort -z | xargs -0 sha256sum | sha256sum)"
expect_eq "7.1 a close run wrote nothing into the measured tree" "$AFTER" "$BEFORE"

# --- 8. --list is informational ("not a guard mode", policy 4.6) ------------------
# NEGATION: a listing mode that gated would duplicate the ratchet and drift
# from it.

ERRN=$((ERRN + 1))
ERRF="$WORK/err.list"
OUT="$(bash "$DONE" --list 2>"$ERRF")"
LIST_RC=$?
LIST_OUT="$OUT"
expect_rc "8.1 --list exits 0 on the real tree" 0 "$LIST_RC" "$ERRF"
expect_match "8.2 --list labels itself informational" "$OUT" 'not a guard mode'
VERDICT_N="$(printf '%s\n' "$OUT" | grep -c $'^verdict\t')" || VERDICT_N=0
if [ "$VERDICT_N" -gt 0 ]; then
  ok "8.3 --list emits per-entry verdict rows" "rows=$VERDICT_N"
else
  bad "8.3 --list emits per-entry verdict rows" "no verdict rows" "$ERRF"
fi
EXC_ROW="$(printf '%s\n' "$OUT" | awk -F'\t' '$1 == "state" && $3 == "exception"' | head -1)"
expect_match "8.4 --list annotates exceptions" "$EXC_ROW" 'exception'

# --- 9. the emitted keys against a CLOSED ENGLISH LIST ----------------------------
# NEGATION: a key invented in an untested branch would ship unreviewed prose
# through the contract (v3.3: the contract is enumerated, not detected).

CLOSED_KEYS='^(base|cap|defect|descent|hard_limit|lines|mode|parked_date|reason|state|verdict)$'
UNION="$(
  {
    printf '%s\n' "$GREEN_OUT"
    printf '%s\n' "$DESCENT_OUT"
    printf '%s\n' "$PARKED_OUT"
    printf '%s\n' "$DEFECT_OUT"
    printf '%s\n' "$LIST_OUT"
  } | awk -F'\t' 'NF >= 2 && $1 ~ /^[a-z_]+$/ { print $1 }' | sort -u
)"
KEY_COUNT="$(printf '%s\n' "$UNION" | grep -cvE "$CLOSED_KEYS")" || KEY_COUNT=0
expect_eq "9.1 every emitted key is on the closed list" "$KEY_COUNT" "0" "$UNION"
expect_match "9.2 the union is fed by real outputs (verdict present)" "$UNION" '^verdict$'
expect_match "9.3 the union is fed by real outputs (defect present)" "$UNION" '^defect$'
expect_match "9.4 the union is fed by real outputs (mode present)" "$UNION" '^mode$'
SRC_KEYS="$(grep -oE 'emit[0-9]?[[:space:]]+"[a-z_]+"' "$DONE" | sed -E 's/emit[0-9]?[[:space:]]+"([a-z_]+)"/\1/' | sort -u)"
expect_eq "9.5 the source emit sites match the emitted union" "$SRC_KEYS" "$UNION" "$UNION"

# --- 10. the harness watches its own house ----------------------------------------
# NEGATION: a harness that cannot fail proves nothing (#2056); a family script
# that greps -q under a pipe fails despite holding (#2925); a guard reachable
# from no workflow is a guard that does not exist (#2056).

if [ "$NEGATION" -eq 0 ]; then
  NEG_OUT="$(bash "$0" --negation 2>"$WORK/err.neg")"
  NEG_RC=$?
  expect_rc "10.1 --negation makes the harness fail (it can fail, #2056)" 1 "$NEG_RC" "$WORK/err.neg"
  if printf '%s\n' "$NEG_OUT" | grep -E '4\.1n negation flip' > /dev/null; then
    ok "10.2 the negation failure is the flipped check"
  else
    bad "10.2 the negation failure is the flipped check" "the negation run failed elsewhere" "$WORK/err.neg"
  fi
fi

# 10.3 the family lint: no `grep -q` under a pipe in any file-size script
# (comment-stripped: prose that mentions the pattern must not trip it).
LINT_PAT='\|[[:space:]]*grep[[:space:]]+(-[A-Za-z]*q[A-Za-z]*|--quiet)'
LINT_HITS=0
for f in "$SCRIPT_DIR/check_file_size.sh" "$SCRIPT_DIR/file_size_preflight.sh" \
  "$SCRIPT_DIR/test_file_size.sh" "$SCRIPT_DIR/test_file_size_preflight.sh" \
  "$SCRIPT_DIR/file_size_done.sh" "$SCRIPT_DIR/test_file_size_done.sh"; do
  HITS="$(sed 's/#.*$//' "$f" | grep -cE "$LINT_PAT")" || HITS=0
  LINT_HITS=$((LINT_HITS + HITS))
done
expect_eq "10.3 no grep -q under a pipe in the file-size family" "$LINT_HITS" "0"

# 10.4 the .tsv note names both scripts (one row per job, #2559).
expect_match "10.4 the .tsv repo-guards note names file_size_done.sh" "$(cat "$TSV")" 'scripts/file_size_done\.sh'
expect_match "10.5 the .tsv repo-guards note names the harness" "$(cat "$TSV")" 'scripts/test_file_size_done\.sh'

# 10.6 ci.yml wires both (live `run:` lines -- #2056).
expect_match "10.6 ci.yml runs file_size_done.sh (--list smoke)" \
  "$(grep -E 'run: bash scripts/file_size_done\.sh --list' "$CI_YML")" 'file_size_done\.sh --list'
expect_match "10.7 ci.yml runs the harness" \
  "$(grep -E 'run: bash scripts/test_file_size_done\.sh$' "$CI_YML")" 'test_file_size_done\.sh'

# 10.8 the local gate triggers the harness on the pair's diff (the F1a pattern:
# a UNION_TMP grep naming both scripts).
FG_BLOCK="$(grep 'file_size_done' "$FAST_GATE")"
expect_match "10.8 ci_fast_gate.sh triggers on the pair's diff" "$FG_BLOCK" 'UNION_TMP'
expect_match "10.9 the trigger names the harness too" "$FG_BLOCK" 'test_file_size_done'

# --- summary ----------------------------------------------------------------------

printf -- '------------------------------------------------------------\n'
printf 'test_file_size_done: passed=%d failed=%d\n' "$passed" "$failed"
[ "$failed" -eq 0 ] || exit 1
