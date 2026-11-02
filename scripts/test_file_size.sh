#!/usr/bin/env bash
# test_file_size.sh
#
# Semantics harness for scripts/check_file_size.sh (ODD
# `odd/tasks/file-size-ratchet.md`).
#
# The guard answers one question: does the tree still satisfy the file-size
# ratchet (new files <= hard_limit; grandfathered files frozen at their
# baseline count and driven DOWN, never silently forgotten)? Before this
# harness the guard could answer wrong in exactly the ways a hand-rolled
# comparison does; every case below pins one decision, offline.
#
# WHAT THIS HARNESS PROVES, offline (mktemp sandboxes; no cargo, no network):
#
#   1. a NEW file at 501 lines FAILS, naming the file and the count;
#   2. a NEW file at exactly 500 lines PASSES (the comparison is strictly
#      `wc -l > hard_limit` — the day-one-discussion case);
#   3. a grandfathered file that GROWS by one line FAILS;
#   4. a grandfathered file at its baseline count PASSES;
#   5. a grandfathered file that SHRANK but stays above the limit PASSES and
#      prints the descending --update notice;
#   6. a grandfathered file that measured <= hard_limit FAILS, asking for the
#      entry's removal (the win must be consolidated, not parked);
#   7. a baseline entry whose file is not in the measured corpus FAILS with
#      the removal command (a silent orphan would let the ratchet rot);
#   8. fail-closed reading: a MISSING baseline file, a baseline WITHOUT the
#      `file_size_rust` key, and an EMPTY corpus (crates/ with zero .rs) all
#      FAIL — none of them can read as "all green";
#   9. --generate builds the sorted grandfathered map (only files above the
#      limit), feeds a passing check, and REFUSES to run twice;
#  10. --update is descending-only: it applies a decrease, refuses an
#      increase, refuses an addition, refuses counts <= hard_limit, and never
#      writes anything on refusal;
#  11. --remove deletes the entry, and the file then counts as NEW
#      (unguarded) — removal exposes it to the hard limit again.
#
# HERMETIC BY DESIGN. The guard is exercised as a COPY inside a sandbox tree
# (its ROOT is derived from its own location), so the harness never writes to
# this repository — the same sandbox shape test_duplication.sh uses.
#
# Usage: bash scripts/test_file_size.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
GUARD="$REPO_ROOT/scripts/check_file_size.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0

ok() {
  printf '  ok   %-58s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-58s %s\n' "$1" "$2"
  if [ -n "${3:-}" ] && [ -f "$3" ]; then
    sed 's/^/        /' "$3" | head -20
  elif [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/        /' | head -20
  fi
  failed=$((failed + 1))
}

# --- sandbox helpers -----------------------------------------------------------

SB=""
OUT="$WORK/out.txt"
RC=0

# new_sandbox <name>: a minimal repo shape the guard's ROOT-relative view
# accepts: scripts/ (the guard copy + the baseline) and crates/*/src/.
new_sandbox() {
  SB="$WORK/sandbox-$1"
  mkdir -p "$SB/scripts" "$SB/crates/webfang_x/src"
  cp "$GUARD" "$SB/scripts/check_file_size.sh"
}

# write_baseline <sandbox>: reads the JSON from stdin.
write_baseline() {
  cat > "$1/scripts/quality-baselines.json"
}

# make_rs <path> <lines>: a deterministic .rs file with exactly N lines.
make_rs() {
  seq 1 "$2" | sed 's/^/line /' > "$1"
}

# run_guard [args...]: runs the SANDBOX's guard copy, capturing rc + output.
run_guard() {
  RC=0
  bash "$SB/scripts/check_file_size.sh" "$@" > "$OUT" 2>&1 || RC=$?
}

# baseline_json <hard_limit> <entries...>: "path count" pairs on the command
# line, valid file_size_rust JSON on stdout.
baseline_json() {
  local limit="$1"
  shift
  local body="" p c
  for pair in "$@"; do
    p="${pair%% *}"
    c="${pair#* }"
    body+="    \"${p}\": ${c},"
  done
  body="${body%,}"
  printf '{\n  "file_size_rust": {\n    "hard_limit": %s,\n    "grandfathered": {\n%s\n    }\n  }\n}' \
    "$limit" "$body"
}

# --- 1. new file at 501 fails, naming file and count ----------------------------
# NEGATION: before this guard the documented rule had no enforcement — a
# 501-line file merged silently. The gate must go red and name the file.

new_sandbox c1
make_rs "$SB/crates/webfang_x/src/a.rs" 501
write_baseline "$SB" < <(baseline_json 500)
run_guard
if [[ $RC -eq 1 ]] && grep -q "a.rs" "$OUT" && grep -q "501" "$OUT"; then
  ok "new file at 501 lines FAILS (names file + count)"
else
  bad "new file at 501 lines FAILS (names file + count)" "rc=$RC" "$OUT"
fi

# --- 2. new file at exactly 500 passes (strict >) --------------------------------
# NEGATION: the positive control of the case above — 500 exactly passes,
# proving the comparison is strictly `>` and the limit needs no debate.

new_sandbox c2
make_rs "$SB/crates/webfang_x/src/a.rs" 500
write_baseline "$SB" < <(baseline_json 500)
run_guard
if [[ $RC -eq 0 ]] && grep -q "OK: el ratchet" "$OUT"; then
  ok "new file at exactly 500 PASSES (strict >)"
else
  bad "new file at exactly 500 PASSES (strict >)" "rc=$RC" "$OUT"
fi

# --- 3. grandfathered grows by one line -> fails ----------------------------------
# NEGATION: a grandfathered cap without growth rejection is not a freeze —
# +1 line must go red.

new_sandbox c3
make_rs "$SB/crates/webfang_x/src/b.rs" 601
write_baseline "$SB" < <(baseline_json 500 "crates/webfang_x/src/b.rs 600")
run_guard
if [[ $RC -eq 1 ]] && grep -q "CRECI" "$OUT"; then
  ok "grandfathered +1 line FAILS"
else
  bad "grandfathered +1 line FAILS" "rc=$RC" "$OUT"
fi

# --- 4. grandfathered at baseline count -> passes ----------------------------------
# NEGATION: the positive control — an untouched grandfathered file leaves the
# gate green (the guard decides, it does not always fail).

new_sandbox c4
make_rs "$SB/crates/webfang_x/src/b.rs" 600
write_baseline "$SB" < <(baseline_json 500 "crates/webfang_x/src/b.rs 600")
run_guard
if [[ $RC -eq 0 ]] && grep -q "OK: el ratchet" "$OUT"; then
  ok "grandfathered at baseline PASSES"
else
  bad "grandfathered at baseline PASSES" "rc=$RC" "$OUT"
fi

# --- 5. grandfathered shrinks, still above limit -> passes with notice -------------
# NEGATION: a descent that leaves the file above the limit must not force a
# baseline edit — it passes, and the notice proposes the exact --update.

new_sandbox c5
make_rs "$SB/crates/webfang_x/src/b.rs" 550
write_baseline "$SB" < <(baseline_json 500 "crates/webfang_x/src/b.rs 600")
run_guard
if [[ $RC -eq 0 ]] && grep -q -- "--update 'crates/webfang_x/src/b.rs=550'" "$OUT"; then
  ok "grandfathered shrunk (>500) PASSES with --update notice"
else
  bad "grandfathered shrunk (>500) PASSES with --update notice" "rc=$RC" "$OUT"
fi

# --- 6. grandfathered <= limit -> fails asking removal -------------------------------
# NEGATION: a parked <=500 entry would let the file grow back to its old cap
# unnoticed — the win must be consolidated, so the gate demands --remove.

new_sandbox c6
make_rs "$SB/crates/webfang_x/src/b.rs" 480
write_baseline "$SB" < <(baseline_json 500 "crates/webfang_x/src/b.rs 600")
run_guard
if [[ $RC -eq 1 ]] && grep -q -- "--remove 'crates/webfang_x/src/b.rs'" "$OUT"; then
  ok "grandfathered <= limit FAILS asking --remove"
else
  bad "grandfathered <= limit FAILS asking --remove" "rc=$RC" "$OUT"
fi

# --- 7. orphan baseline entry -> fails with removal command ---------------------------
# NEGATION: a silent orphan lets the ratchet rot — the entry must demand its
# own removal.

new_sandbox c7
make_rs "$SB/crates/webfang_x/src/a.rs" 100
write_baseline "$SB" < <(baseline_json 500 "crates/webfang_x/src/ghost.rs 700")
run_guard
if [[ $RC -eq 1 ]] && grep -q "ghost.rs" "$OUT" && grep -q -- "--remove" "$OUT"; then
  ok "orphan entry FAILS with the removal command"
else
  bad "orphan entry FAILS with the removal command" "rc=$RC" "$OUT"
fi

# --- 8. fail-closed reading ------------------------------------------------------------
# NEGATION: "could not measure" once translated into "all green" (the
# check_duplication.sh defect class) — a missing file, a missing key and an
# empty corpus each go red with their own named cause.

new_sandbox c8a
make_rs "$SB/crates/webfang_x/src/a.rs" 100
run_guard
if [[ $RC -eq 1 ]] && grep -q "Missing baseline file" "$OUT"; then
  ok "missing baseline FAILS"
else
  bad "missing baseline FAILS" "rc=$RC" "$OUT"
fi

new_sandbox c8b
make_rs "$SB/crates/webfang_x/src/a.rs" 100
write_baseline "$SB" <<'JSON'
{
  "description": "no ratchet key here"
}
JSON
run_guard
if [[ $RC -eq 1 ]] && grep -q "file_size_rust" "$OUT"; then
  ok "baseline without the ratchet key FAILS"
else
  bad "baseline without the ratchet key FAILS" "rc=$RC" "$OUT"
fi

new_sandbox c8c
# crates/ exists but holds zero .rs files: an unmeasured tree is a red gate,
# never "0 files = all green".
write_baseline "$SB" < <(baseline_json 500)
run_guard
if [[ $RC -eq 1 ]] && grep -q "0 archivos" "$OUT"; then
  ok "empty corpus FAILS (fail-closed)"
else
  bad "empty corpus FAILS (fail-closed)" "rc=$RC" "$OUT"
fi

# --- 9. --generate: sorted map, feeds a passing check, refuses twice --------------------
# NEGATION: a --generate that overwrote an existing key would let a
# regeneration silently re-freeze whatever the tree measures — it refuses.

new_sandbox c9
make_rs "$SB/crates/webfang_x/src/a.rs" 501
make_rs "$SB/crates/webfang_x/src/b.rs" 600
make_rs "$SB/crates/webfang_x/src/c.rs" 400
write_baseline "$SB" <<'JSON'
{
  "description": "pre-existing baseline without the ratchet key"
}
JSON
run_guard --generate
GEN_JSON_OK=true
if [[ $RC -eq 0 ]] && grep -q "entradas grandfathered: 2" "$OUT"; then
  : # count asserted below via the JSON itself
else
  GEN_JSON_OK=false
  bad "--generate builds the map (2 offenders)" "rc=$RC" "$OUT"
fi
if $GEN_JSON_OK; then
  GEN_CHECK="$(python3 - "$SB/scripts/quality-baselines.json" <<'PY'
import json, sys
with open(sys.argv[1]) as fh:
    data = json.load(fh)
gf = data["file_size_rust"]["grandfathered"]
keys = list(gf.keys())
ok_shape = (
    keys == sorted(keys)
    and data["file_size_rust"]["hard_limit"] == 500
    and gf == {
        "crates/webfang_x/src/a.rs": 501,
        "crates/webfang_x/src/b.rs": 600,
    }
)
print("OK" if ok_shape else "BAD")
PY
)"
  if [[ "$GEN_CHECK" == "OK" ]]; then
    ok "--generate: sorted keys, only >limit files, c.rs excluded"
  else
    bad "--generate: sorted keys, only >limit files, c.rs excluded" "shape=$GEN_CHECK"
  fi
  run_guard
  if [[ $RC -eq 0 ]]; then
    ok "--generate output feeds a PASSING check"
  else
    bad "--generate output feeds a PASSING check" "rc=$RC" "$OUT"
  fi
  run_guard --generate
  if [[ $RC -eq 1 ]] && grep -q "un solo uso" "$OUT"; then
    ok "--generate refuses when the key already exists"
  else
    bad "--generate refuses when the key already exists" "rc=$RC" "$OUT"
  fi
fi

# --- 10. --update is descending-only, and never writes on refusal -----------------------
# NEGATION: an --update that accepted increases or additions would reopen the
# ratchet upward — refused, and nothing is written on refusal.

new_sandbox c10
make_rs "$SB/crates/webfang_x/src/b.rs" 600
write_baseline "$SB" < <(baseline_json 500 "crates/webfang_x/src/b.rs 600")

run_guard --update "crates/webfang_x/src/b.rs=550"
UPDATE_DOWN_RC=$RC
VAL550="$(python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]))['file_size_rust']['grandfathered']['crates/webfang_x/src/b.rs'])" "$SB/scripts/quality-baselines.json")"
if [[ $UPDATE_DOWN_RC -eq 0 ]] && [[ "$VAL550" == "550" ]]; then
  ok "--update applies a decrease (600 -> 550)"
else
  bad "--update applies a decrease (600 -> 550)" "rc=$UPDATE_DOWN_RC val=$VAL550" "$OUT"
fi

run_guard --update "crates/webfang_x/src/b.rs=551"
VAL_AFTER="$(python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]))['file_size_rust']['grandfathered']['crates/webfang_x/src/b.rs'])" "$SB/scripts/quality-baselines.json")"
if [[ $RC -eq 1 ]] && [[ "$VAL_AFTER" == "550" ]]; then
  ok "--update refuses an increase WITHOUT writing (still 550)"
else
  bad "--update refuses an increase WITHOUT writing (still 550)" "rc=$RC val=$VAL_AFTER" "$OUT"
fi

run_guard --update "crates/webfang_x/src/ghost.rs=100"
if [[ $RC -eq 1 ]] && grep -q "edición manual" "$OUT"; then
  ok "--update refuses an ADDITION"
else
  bad "--update refuses an ADDITION" "rc=$RC" "$OUT"
fi

run_guard --update "crates/webfang_x/src/b.rs=500"
if [[ $RC -eq 1 ]] && grep -q -- "--remove" "$OUT"; then
  ok "--update refuses count <= hard_limit (points at --remove)"
else
  bad "--update refuses count <= hard_limit (points at --remove)" "rc=$RC" "$OUT"
fi

# --- 11. --remove deletes the entry; the file becomes unguarded (NEW) --------------------
# NEGATION: --remove is exposure, not amnesty — the removed file counts as NEW
# and goes red above the limit again.

new_sandbox c11
make_rs "$SB/crates/webfang_x/src/b.rs" 600
write_baseline "$SB" < <(baseline_json 500 "crates/webfang_x/src/b.rs 600")
run_guard --remove "crates/webfang_x/src/b.rs"
if [[ $RC -eq 0 ]]; then
  ok "--remove deletes the entry"
else
  bad "--remove deletes the entry" "rc=$RC" "$OUT"
fi
run_guard
if [[ $RC -eq 1 ]] && grep -q "No está en el baseline" "$OUT"; then
  ok "after --remove the file counts as NEW (unguarded)"
else
  bad "after --remove the file counts as NEW (unguarded)" "rc=$RC" "$OUT"
fi

# --- summary ------------------------------------------------------------------------------

echo "----------------------------------------"
echo "test_file_size: passed=$passed failed=$failed"
if [[ $failed -gt 0 ]]; then
  exit 1
fi
exit 0
