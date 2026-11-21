#!/usr/bin/env bash
# file_size_done.sh -- the close-out tool of the file-size policy v3 (umbrella
# #2584, phase F1b; ODD odd/tasks/file-size-done.md).
#
# The ratchet guard (check_file_size.sh) asks "is this file LEGAL". The
# preflight (file_size_preflight.sh) asks "what IS it, and what will a cut
# COST". The close-out asks "is this mission DONE" -- and answers with a
# verdict a script decided, never an opinion (policy 2: "Done" is decided by a
# script, not an opinion).
#
# WHAT IT DECIDES (the closed map, policy 11 F1b):
#   exit 0  closed: every named file (the mission's target and its split
#           parts) measures <= hard_limit, OR carries an exceptions entry
#           already present at the base commit; the ratchet descent is
#           consistent (--remove done when a grandfathered file landed at or
#           below the limit); and the mission's diff raises none of the
#           defects below.
#   exit 2  parked, with a reason, a date and the base commit (policy 7):
#           a named file is still over the limit and not in exceptions at the
#           base commit, or the --remove is still pending after a partial
#           reduction.
#   exit 1  a defect with its cause -- the mission's diff:
#             - adds or empties an exceptions entry (an exception is a
#               maintainer decision in its own PR BEFORE the mission that
#               benefits from it, policy 5; the map is read from the base
#               commit so a mission cannot grant itself one),
#             - adds an allow (a net-new suppression across the named files;
#               a pre-existing allow passes, an allow that MOVED with its code
#               passes -- the multiset is what is judged),
#             - removes a grandfathered entry whose file still measures over
#               the limit AND still sits in the measured corpus (a file that
#               left the corpus is the guard's own orphan rule, not a defect),
#             - or changes hard_limit (raising it is a baseline edit, a
#               maintainer decision, policy 6).
#   exit 64 usage (EX_USAGE) -- the preflight's own v3.4.0 decision (#2928);
#           2 belongs to "parked", never to a usage error.
#   Tool and environment failures are rc 1 FAIL-CLOSED with a named cause
#   (`error:` lines); governance defects say `defect:`. A broken environment
#   is never a green close.
#
# MEASUREMENT, NOT PROSE (policy 1): exceptions, grandfathered and hard_limit
# are read from BOTH the base commit (git show <base>:scripts/quality-
# baselines.json) and the working tree; the diff between them is the mission's
# authority surface. --base defaults to the merge-base of HEAD with
# origin/main; a mission passes its own base commit explicitly.
#
# HERMETIC: nothing is ever written into the measured tree (all scratch lives
# in a mktemp dir). The corpus is NOT re-implemented: the find expression is
# extracted from check_file_size.sh at runtime; one corpus, one owner.
#
# Delegated to other gates (measured there, not here): function lints and the
# allow ratchet's full semantics (F3), jscpd cx (check_duplication.sh in CI),
# the real-tree golden (check_file_size.sh in repo-guards), the mission's test
# set (the declared runner's <runner> list, policy 4.6).
#
# Exit codes: 0 closed · 1 defect or tool failure (the line names which) ·
#             2 parked (with a reason) · 64 usage (EX_USAGE).

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE="$ROOT/scripts/quality-baselines.json"
GUARD="$ROOT/scripts/check_file_size.sh"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

declare -A GF_NOW=()   # ROOT-relative path -> grandfathered cap (working tree)
declare -A EXC_NOW=()  # ROOT-relative path -> declared exception why (working tree)
declare -A GF_BASE=()  # ROOT-relative path -> grandfathered cap (base commit)
declare -A EXC_BASE=() # ROOT-relative path -> declared exception why (base commit)

usage() {
  cat <<'USAGE'
Usage: bash scripts/file_size_done.sh [--base <commit>] <target-path> [part-path ...]
       bash scripts/file_size_done.sh --list
       bash scripts/file_size_done.sh --help

Closes a file-size mission (policy 4.6 and 7, phase F1b of umbrella #2584).
The named files are the mission's target and its split parts (repo-relative
paths, as the guard spells them). Everything the verdict needs is measured;
nothing is invented. --list is an informational dashboard over the baseline's
grandfathered entries; it is NOT a guard mode (its verdicts are not gate
decisions).

  --base <commit>   the mission's base commit (default: the merge-base of HEAD
                    with origin/main). exceptions, grandfathered and
                    hard_limit are read from BOTH the base commit and the
                    working tree; the diff between them is the mission's
                    authority surface.
  --list            informational per-entry status; exits 0
  --help            this help

Exit codes (the closed map, documented and tested):
  0    closed: every named file measures <= hard_limit (or carries an
       exceptions entry already present at the base commit), the ratchet
       descent is consistent, and the diff raises none of the defects
  1    defect or tool failure -- the line names which (defect: the diff
       added/emptied an exceptions entry, added an allow, removed a
       grandfathered entry whose file is still over the limit, or changed
       hard_limit · error: the tool cannot answer, fail-closed)
  2    parked (with a reason, a date and the base commit): still over the
       limit and not in exceptions, or the --remove is still pending
  64   usage (EX_USAGE)
USAGE
}

die_tool() {   # rc=1: the tool cannot answer, with a NAMED cause (fail-closed)
  printf 'error: %s\n' "$1" >&2
  exit 1
}

usage_error() {  # rc=64 (EX_USAGE): 2 is reserved by the policy for the
                 # "parked" verdict (issue #2928)
  printf 'error: %s\n' "$1" >&2
  usage >&2
  exit 64
}

emit2() { printf '%s\t%s\n' "$1" "$2"; }
emit3() { printf '%s\t%s\t%s\n' "$1" "$2" "$3"; }
emit4() { printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4"; }
emit5() { printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5"; }

# --- the corpus lives in the guard ----------------------------------------------
#
# extract_corpus: pulls the find expression out of check_file_size.sh VERBATIM
# (the same line the preflight extracts), so the three tools can never drift.
# A guard whose line changed shape is a tool failure, never a re-implementation.
extract_corpus() {
  [ -f "$GUARD" ] || die_tool "guard $GUARD not found: the close-out's corpus lives in the guard and is never re-implemented"
  # the sed programs below keep \$ROOT literal ON PURPOSE: it is the guard's
  # own text being extracted, not an expansion.
  # shellcheck disable=SC2016
  EXEC_FIND="$(sed -n 's/^ *if ! \(find "\$ROOT\/crates" .*-print0\) \\$/\1/p' "$GUARD" | head -1)"
  [ -n "${EXEC_FIND:-}" ] || die_tool "could not extract the find expression from $GUARD (its shape changed): the corpus is never re-implemented; update the extraction together with the guard"
}

CORPUS_READY=0

ensure_corpus() {
  [ "$CORPUS_READY" -eq 1 ] && return 0
  eval "$EXEC_FIND" > "$WORK/corpus.nul" 2>/dev/null || die_tool "find failed over crates/: the corpus cannot be measured (fail-closed)"
  CORPUS_READY=1
}

corpus_contains() {
  ensure_corpus
  local f
  while IFS= read -r -d '' f; do
    [ "${f#"$ROOT"/}" = "$1" ] && return 0
  done < "$WORK/corpus.nul"
  return 1
}

# --- the single baseline reader/differ -------------------------------------------
#
# One embedded python3 block (the guard's own pattern: one reader, named
# statuses, fail-closed) validates BOTH baselines, writes the four sorted TSV
# maps, sets LIMIT_NOW/LIMIT_BASE, and leaves the mission's authority surface
# in $WORK/diff.facts:
#   LIMIT_CHANGED\t<base>\t<now>
#   EXC_ADDED\t<key>\t<why>            an exceptions entry born in the diff
#   EXC_REMOVED\t<key>\t<why>          an exceptions entry emptied by the diff
#   EXC_CHANGED\t<key>\t<old>\t<new>   an exceptions entry whose why changed
#   GF_REMOVED\t<key>\t<old cap>       a grandfathered entry the diff removes
# Reading <baseline> twice (base = now) is the --list reader: no diff facts,
# the current maps only.
baseline_diff() {  # <baseline-now> <baseline-base>
  local now_path="$1" base_path="$2" raw py3_rc=0 first status
  raw="$(python3 -I - "$now_path" "$base_path" \
    "$WORK/gf.now.tsv" "$WORK/exc.now.tsv" "$WORK/gf.base.tsv" "$WORK/exc.base.tsv" <<'PY'
import json
import sys


def emit(status, value=""):
    print("%s\t%s" % (status, value))
    sys.exit(0)


def canon(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


def load(path):
    try:
        with open(path) as fh:
            data = json.load(fh)
    except FileNotFoundError:
        emit("MISSING_FILE", path)
    except OSError as exc:
        emit("UNREADABLE", str(exc))
    except ValueError as exc:
        emit("BAD_JSON", str(exc))
    if not isinstance(data, dict) or "file_size_rust" not in data:
        emit("MISSING_KEY", "file_size_rust")
    ratchet = data["file_size_rust"]
    if not isinstance(ratchet, dict):
        emit("NOT_AN_OBJECT", "file_size_rust")
    if "hard_limit" not in ratchet:
        emit("MISSING_HARD_LIMIT", "file_size_rust.hard_limit")
    limit = ratchet["hard_limit"]
    if isinstance(limit, bool) or not isinstance(limit, int):
        emit("HARD_LIMIT_NOT_INT", repr(limit))
    gf = ratchet.get("grandfathered")
    if not isinstance(gf, dict):
        emit("GRANDFATHERED_NOT_OBJECT", "file_size_rust.grandfathered")
    for key, value in gf.items():
        if isinstance(value, bool) or not isinstance(value, int):
            emit("ENTRY_NOT_INT", "%s=%r" % (key, value))
    exc = ratchet.get("exceptions", {})
    if not isinstance(exc, dict):
        emit("EXCEPTIONS_NOT_OBJECT", "file_size_rust.exceptions")
    return limit, gf, exc


def dump_map(path, mapping, encode):
    with open(path, "w") as out:
        for key in sorted(mapping):
            out.write("%s\t%s\n" % (key, encode(mapping[key])))


now_path, base_path = sys.argv[1], sys.argv[2]
limit_now, gf_now, exc_now = load(now_path)
limit_base, gf_base, exc_base = load(base_path)
dump_map(sys.argv[3], gf_now, str)
dump_map(sys.argv[4], exc_now, canon)
dump_map(sys.argv[5], gf_base, str)
dump_map(sys.argv[6], exc_base, canon)
print("OK\t%d\t%d" % (limit_now, limit_base))
if limit_base != limit_now:
    print("LIMIT_CHANGED\t%d\t%d" % (limit_base, limit_now))
for key in sorted(set(exc_now) - set(exc_base)):
    print("EXC_ADDED\t%s\t%s" % (key, canon(exc_now[key])))
for key in sorted(set(exc_base) - set(exc_now)):
    print("EXC_REMOVED\t%s\t%s" % (key, canon(exc_base[key])))
for key in sorted(set(exc_base) & set(exc_now)):
    if canon(exc_base[key]) != canon(exc_now[key]):
        print("EXC_CHANGED\t%s\t%s\t%s" % (key, canon(exc_base[key]), canon(exc_now[key])))
for key in sorted(set(gf_base) - set(gf_now)):
    print("GF_REMOVED\t%s\t%d" % (key, gf_base[key]))
PY
)" || py3_rc=$?
  if [ "$py3_rc" -ne 0 ] && [ -z "$raw" ]; then
    die_tool "python3 failed silently (rc=$py3_rc): the close-out never invents a verdict without its reader (fail-closed)"
  fi
  first="${raw%%$'\n'*}"
  # the trailing newline is NOT cosmetic: $(...) stripped python's final \n,
  # and `while read` discards a last line that lacks one (read populates the
  # variables but returns 1) -- the mission's authority surface would vanish.
  printf '%s\n' "${raw#"$first"$'\n'}" > "$WORK/diff.facts"
  status="${first%%$'\t'*}"
  case "$status" in
    OK)
      LIMIT_NOW="$(printf '%s' "$first" | cut -f2)"
      LIMIT_BASE="$(printf '%s' "$first" | cut -f3)"
      ;;
    MISSING_FILE) die_tool "baseline file not found: ${first#*$'\t'}" ;;
    UNREADABLE) die_tool "cannot read the baseline ${first#*$'\t'}" ;;
    BAD_JSON) die_tool "the baseline ${first#*$'\t'} is not valid JSON" ;;
    *) die_tool "the baseline cannot be read ($status): ${first#*$'\t'}" ;;
  esac
}

load_gf_map() {  # <tsv> <array-name>
  local p v
  while IFS=$'\t' read -r p v; do
    [ -n "$p" ] && printf -v "$2[$p]" '%s' "$v"
  done < "$1"
}

# measure <rel>: prints the wc -l of a ROOT-relative file, or "indeterminate"
# when the file is gone (a fact, never a guess).
measure() {
  local n
  if [ -f "$ROOT/$1" ] && n="$(wc -l < "$ROOT/$1" 2>/dev/null)"; then
    printf '%s' "$n"
  else
    printf 'indeterminate'
  fi
}

# allows_of <abs-path>: the allow multiset as "count attribute" lines (sorted).
# Line-granular `#!?[allow(...)]`: a declared limit (mission L3); F3 owns the
# function-level semantics. Under pipefail the producer is consumed in full --
# never `grep -q` (issue #2925).
allows_of() {
  grep -oE '#!?\[allow\([^]]*\)\]' "$1" 2>/dev/null \
    | sort | uniq -c | sed -E 's/^ +([0-9]+) /\1 /' | sort
}

# allow_multiset <out-file> <mode: base|now>: aggregates the named files' allow
# multisets of one side. Base content comes from the base commit (a file born
# in the diff has an empty base multiset -- that is the point).
allow_multiset() {
  local out="$1" mode="$2" rel content
  : > "$out"
  for rel in "${FILES[@]}"; do
    if [ "$mode" = "base" ]; then
      if ! content="$(git -C "$ROOT" show "$BASE_SHA:$rel" 2>/dev/null)"; then
        continue
      fi
      printf '%s\n' "$content" | grep -oE '#!?\[allow\([^]]*\)\]' | sort | uniq -c \
        | sed -E 's/^ +([0-9]+) /\1 /' >> "$out"
    else
      [ -f "$ROOT/$rel" ] && allows_of "$ROOT/$rel" >> "$out"
    fi
  done
  sort -o "$out" "$out"
}

# --- argument parsing --------------------------------------------------------------

MODE="close"
BASE_ARG=""
FILES=()

while [ $# -gt 0 ]; do
  case "$1" in
    --base)
      [ $# -ge 2 ] || usage_error "--base requires the mission's base commit"
      BASE_ARG="$2"
      shift 2
      ;;
    --list)
      MODE="list"
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      usage_error "unknown argument: '$1'"
      ;;
    *)
      FILES+=("$1")
      shift
      ;;
  esac
done

command -v python3 >/dev/null 2>&1 || die_tool "python3 is not available: the baseline reader needs it (install it or run on a station that has python3)"
extract_corpus

if [ "$MODE" = "list" ]; then
  ensure_corpus
  # --list is informational: the CURRENT baseline only, no base commit, no
  # defects, exit 0 (policy 4.6: "--list is not a guard mode"). The reader is
  # the same baseline_diff with base = now: one reader, one owner.
  baseline_diff "$BASELINE" "$BASELINE"
  load_gf_map "$WORK/gf.now.tsv" GF_NOW
  load_gf_map "$WORK/exc.now.tsv" EXC_NOW

  emit3 "mode" "list" "informational, not a guard mode: verdicts here are not gate decisions"
  emit4 "hard_limit" "$LIMIT_NOW" "lines" "jq '.file_size_rust.hard_limit' scripts/quality-baselines.json"
  for rel in "${!GF_NOW[@]}"; do
    N="$(measure "$rel")"
    if [ "$N" = "indeterminate" ]; then
      emit5 "lines" "$rel" "indeterminate" "lines" "wc -l < '$rel'"
      emit3 "state" "$rel" "grandfathered"
      emit3 "verdict" "$rel" "parked"
      emit3 "reason" "$rel" "the entry's file is gone from the measured corpus (orphan): bash scripts/check_file_size.sh --remove '$rel'"
      continue
    fi
    emit5 "lines" "$rel" "$N" "lines" "wc -l < '$rel'"
    if [ -n "${EXC_NOW["$rel"]+x}" ]; then
      emit3 "state" "$rel" "exception"
      emit3 "cap" "$rel" "${GF_NOW["$rel"]}"
      emit3 "verdict" "$rel" "closed"
      emit3 "reason" "$rel" "an exceptions entry is present in the baseline (the why lives in the baseline's exceptions map)"
    else
      emit3 "state" "$rel" "grandfathered"
      emit3 "cap" "$rel" "${GF_NOW["$rel"]}"
      emit3 "verdict" "$rel" "parked"
      if [ "$N" -le "$LIMIT_NOW" ]; then
        emit3 "reason" "$rel" "measures $N lines, at/below the hard limit ($LIMIT_NOW): consolidate the win -- bash scripts/check_file_size.sh --remove '$rel' (remove pending)"
      else
        emit3 "reason" "$rel" "measures $N lines, over the hard limit ($LIMIT_NOW) and not in exceptions (grandfathered cap ${GF_NOW["$rel"]})"
      fi
    fi
  done
  for rel in "${!EXC_NOW[@]}"; do
    [ -n "${GF_NOW["$rel"]+x}" ] && continue
    N="$(measure "$rel")"
    emit5 "lines" "$rel" "$N" "lines" "wc -l < '$rel'"
    emit3 "state" "$rel" "exception"
    emit3 "verdict" "$rel" "closed"
    emit3 "reason" "$rel" "an exceptions entry is present in the baseline (the why lives in the baseline's exceptions map)"
  done
  exit 0
fi

# --- close mode ---------------------------------------------------------------------

[ "${#FILES[@]}" -gt 0 ] || usage_error "close mode requires at least one file: the mission's target and its split parts"
command -v git >/dev/null 2>&1 || die_tool "git is not available: the base commit is the contract's authority surface (fail-closed)"
ensure_corpus

NORM_FILES=()
for rel in "${FILES[@]}"; do
  NORM_FILES+=("${rel#"$ROOT"/}")
done
FILES=("${NORM_FILES[@]}")

for rel in "${FILES[@]}"; do
  [ -f "$ROOT/$rel" ] || usage_error "file not found: $rel (is it the repo-relative path the guard uses? the mission names the surviving parts)"
  [ -r "$ROOT/$rel" ] || usage_error "file cannot be read: $rel"
done
for rel in "${FILES[@]}"; do
  if ! corpus_contains "$rel"; then
    usage_error "not in the measured corpus (the guard's find): $rel -- the close-out closes corpus files"
  fi
done

# the base commit: explicit wins; the default is the merge-base with origin/main
if [ -n "$BASE_ARG" ]; then
  BASE_SHA="$(git -C "$ROOT" rev-parse --verify --quiet "$BASE_ARG^{commit}" 2>/dev/null)" ||
    usage_error "cannot resolve the base commit: '$BASE_ARG' is not a commit this repository knows"
else
  BASE_SHA="$(git -C "$ROOT" merge-base HEAD origin/main 2>/dev/null)" ||
    die_tool "cannot determine the base commit (no merge-base with origin/main; shallow clone or not a repo?): pass --base with the mission's base commit"
fi

[ -f "$BASELINE" ] || die_tool "baseline file not found: $BASELINE"
git -C "$ROOT" show "$BASE_SHA:scripts/quality-baselines.json" > "$WORK/baseline.base.json" 2>/dev/null ||
  die_tool "git show $BASE_SHA:scripts/quality-baselines.json failed: the base commit must carry the baseline it was measured against"

LIMIT_NOW=""
LIMIT_BASE=""
baseline_diff "$BASELINE" "$WORK/baseline.base.json"
load_gf_map "$WORK/gf.now.tsv" GF_NOW
load_gf_map "$WORK/exc.now.tsv" EXC_NOW
load_gf_map "$WORK/gf.base.tsv" GF_BASE
load_gf_map "$WORK/exc.base.tsv" EXC_BASE

emit2 "base" "$BASE_SHA"
emit4 "hard_limit" "$LIMIT_NOW" "lines" "jq '.file_size_rust.hard_limit' scripts/quality-baselines.json"

# --- the defects (the mission's authority surface) -----------------------------------

DEFECTS=()

if [ "$LIMIT_BASE" != "$LIMIT_NOW" ]; then
  emit3 "defect" "hard-limit-changed" "the base commit carries hard_limit $LIMIT_BASE, the working tree $LIMIT_NOW: raising it is a baseline edit, a maintainer decision (policy 6)"
  DEFECTS+=("hard-limit-changed")
fi

while IFS=$'\t' read -r tag f1 f2 f3; do
  case "$tag" in
    EXC_ADDED)
      emit3 "defect" "exceptions-added" "$f1 added inside the mission's diff: an exception is a maintainer decision in its own PR BEFORE the mission (policy 5); the base commit does not carry it"
      DEFECTS+=("exceptions-added:$f1")
      ;;
    EXC_REMOVED)
      emit3 "defect" "exceptions-removed" "$f1 emptied by the mission's diff (its why was: $f2): the exceptions map is a maintainer surface, never the mission's"
      DEFECTS+=("exceptions-removed:$f1")
      ;;
    EXC_CHANGED)
      emit3 "defect" "exceptions-value-changed" "$f1's why changed inside the mission's diff: '$f2' -> '$f3'"
      DEFECTS+=("exceptions-value-changed:$f1")
      ;;
    GF_REMOVED)
      N="$(measure "$f1")"
      if [ "$N" != "indeterminate" ] && corpus_contains "$f1" && [ "$N" -gt "$LIMIT_NOW" ]; then
        emit3 "defect" "grandfathered-removed-over-limit" "$f1 still measures $N lines (> hard_limit $LIMIT_NOW) and sits in the corpus: the removal must follow the file landing at/below the limit (--remove consolidates a win, it does not open a gap)"
        DEFECTS+=("grandfathered-removed-over-limit:$f1")
      else
        emit2 "descent" "$f1"
      fi
      ;;
  esac
done < "$WORK/diff.facts"

allow_multiset "$WORK/allows.base" base
allow_multiset "$WORK/allows.now" now
NET_NEW="$(comm -13 "$WORK/allows.base" "$WORK/allows.now")"
if [ -n "$NET_NEW" ]; then
  while IFS= read -r line; do
    emit3 "defect" "allow-added" "net-new suppression across the named files: $line (a pre-existing allow passes; an allow that moved with its code passes)"
    DEFECTS+=("allow-added:$line")
  done <<< "$NET_NEW"
fi

# --- the per-file verdicts -----------------------------------------------------------

PARKED=0
for rel in "${FILES[@]}"; do
  N="$(measure "$rel")"
  if [ "$N" = "indeterminate" ]; then
    usage_error "file not found: $rel (is it the repo-relative path the guard uses? the mission names the surviving parts)"
  fi
  emit5 "lines" "$rel" "$N" "lines" "wc -l < '$rel'"
  if [ -n "${EXC_NOW["$rel"]+x}" ]; then
    emit3 "state" "$rel" "exception"
  elif [ -n "${GF_NOW["$rel"]+x}" ]; then
    emit3 "state" "$rel" "grandfathered"
    emit3 "cap" "$rel" "${GF_NOW["$rel"]}"
  else
    emit3 "state" "$rel" "plain"
  fi

  if [ -n "${EXC_BASE["$rel"]+x}" ]; then
    emit3 "verdict" "$rel" "closed"
    emit3 "reason" "$rel" "an exceptions entry already present at the base commit (policy 5: the why lives in the baseline, argued before the mission)"
    continue
  fi
  if [ "$N" -le "$LIMIT_NOW" ]; then
    if [ -n "${GF_BASE["$rel"]+x}" ] && [ -n "${GF_NOW["$rel"]+x}" ]; then
      PARKED=1
      emit3 "verdict" "$rel" "parked"
      emit3 "reason" "$rel" "measures $N lines, at/below the hard limit ($LIMIT_NOW): consolidate the win -- bash scripts/check_file_size.sh --remove '$rel' (remove pending)"
    else
      emit3 "verdict" "$rel" "closed"
      if [ -n "${GF_BASE["$rel"]+x}" ]; then
        emit3 "reason" "$rel" "measures $N lines, at/below the hard limit ($LIMIT_NOW); the baseline entry was removed (--remove done)"
      else
        emit3 "reason" "$rel" "measures $N lines, at/below the hard limit ($LIMIT_NOW)"
      fi
    fi
  else
    PARKED=1
    emit3 "verdict" "$rel" "parked"
    if [ -n "${GF_NOW["$rel"]+x}" ]; then
      CAP="${GF_NOW["$rel"]}"
      REASON="measures $N lines, over the hard limit ($LIMIT_NOW) and not in exceptions at the base commit (grandfathered cap $CAP)"
      [ "$N" -gt "$CAP" ] && REASON="$REASON; it grew past its cap -- the guard is red"
      emit3 "reason" "$rel" "$REASON"
    else
      emit3 "reason" "$rel" "measures $N lines, over the hard limit ($LIMIT_NOW) and not in exceptions at the base commit; not grandfathered: split further"
    fi
  fi
done

# --- the verdict -----------------------------------------------------------------------

if [ "${#DEFECTS[@]}" -gt 0 ]; then
  exit 1
fi
if [ "$PARKED" -eq 1 ]; then
  emit2 "parked_date" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  exit 2
fi
exit 0
