#!/usr/bin/env bash
# split_tests.sh -- the mod-tests extraction with the multiset proof (F2 of the
# file-size policy v3, umbrella #2584; ODD odd/tasks/file-size-split-tests.md).
#
# Policy 4.3 step 5: "split_tests.sh [F2] will extract mod tests with a
# multiset proof; until then the verbatim move is proven by hand." This tool
# mechanizes exactly that step: it re-parents ONE named top-level
# #[cfg(test)]-attributed `mod <name> { ... }` block into a child file, byte
# for byte (mod-level dedent only), replaces the block with its declaration,
# and proves the move with the test-item PARTITION IDENTITY:
#
#     multiset(source_before) == multiset(source_after) + multiset(dest)
#
# over test items (a `fn` whose contiguous attribute run carries a test-running
# attribute: #[test], #[tokio::test], #[rstest], #[test_case]), scanned with
# the same deterministic rule on all three sides. Nothing is lost, nothing is
# invented, nothing is duplicated; a header that adds test items breaks the
# partition and is refused -- the move must be pure. Name-resolution fixes
# after the move belong to the mission's declared "Corregido / mejorado" hat
# (the two-hats protocol, policy 7): this tool moves bytes and proves the
# partition.
#
# ONE RUST LEXER IN THE PROGRAM: the span question is answered by
# file_size_preflight.sh --lex-span (the preflight owns the lexer since F1a;
# strings, raw strings, comments, char literals and lifetimes never move a
# span). This tool carries no parser of its own and never cuts a block it
# cannot see: an INDETERMINATE span is refused fail-closed.
#
# WHAT IT EMITS (TAB separated, CLOSED English key list):
#   extracted  <s:e>            span  mod <name> (cfg(test)); N test items moved
#   dest       <path>           file  N test items + H header lines; <decl kind>
#   decl       <path>           file  the declaration written into the source
#   proof      before=B after=A dest=D  partition  multiset(source_before) ==
#                                multiset(source_after) + multiset(dest)
#
# Decl shape (the #2950 verified manual extraction): the block's attribute
# lines are kept verbatim; plain `mod <name>;` when the dest IS the standard
# child file of the declaring source (<source-stem>/<name>.rs beside it),
# `#[path = "<dest relative to the source's dir>"] mod <name>;` otherwise.
#
# Exit codes: 0 extraction done and the partition holds · 1 tool failure
# (named: python3/span-lexer unreachable, a cut that loses items) · 4
# defective plan (named: unknown mod, ambiguous name, target without
# cfg(test), INDETERMINATE span, missing source, dest == source, dest exists,
# a header that adds test items) · 64 usage (EX_USAGE).
#
# HERMETIC: writes exactly two paths -- the dest and the source -- and only
# after the partition holds (fail-closed: a broken extraction writes nothing).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFLIGHT="$SCRIPT_DIR/file_size_preflight.sh"

usage() {
  cat <<'USAGE'
Usage: bash scripts/split_tests.sh <source-file> --mod <name> --dest <dest-file> [--header-file <f>]

  <source-file>       the file carrying the inline #[cfg(test)] mod (readable, rewritten in place)
  --mod <name>        the top-level braced test mod to extract (a Rust identifier)
  --dest <dest-file>  the child file the mod's items move into (must not exist)
  --header-file <f>   verbatim lines prepended to the dest (imports; adding test
                      items breaks the partition and is refused)

The span is answered by file_size_preflight.sh --lex-span: ONE Rust lexer in
the program. The proof: multiset(source_before) == multiset(source_after) +
multiset(dest) over test items; nothing lost, nothing invented, nothing
duplicated.

Exit codes: 0 ok · 1 tool failure (named) · 4 defective plan (named) · 64 usage
USAGE
}

die_tool() {   # rc=1: the tool cannot answer, with a NAMED cause
  printf 'error: %s\n' "$1" >&2
  exit 1
}

die_plan() {   # rc=4: the extraction plan is defective, with a NAMED cause
  printf 'defect: %s\n' "$1" >&2
  exit 4
}

usage_error() {  # rc=64 (EX_USAGE)
  printf 'error: %s\n' "$1" >&2
  usage >&2
  exit 64
}

SOURCE_PATH=""
MOD_NAME=""
DEST_PATH=""
HEADER_PATH=""

POSITIONAL=()
while [ $# -gt 0 ]; do
  case "$1" in
    --mod)
      [ $# -ge 2 ] || usage_error "--mod requires the test mod's name"
      MOD_NAME="$2"
      shift 2
      ;;
    --dest)
      [ $# -ge 2 ] || usage_error "--dest requires the child file path"
      DEST_PATH="$2"
      shift 2
      ;;
    --header-file)
      [ $# -ge 2 ] || usage_error "--header-file requires the import block to prepend to the dest"
      HEADER_PATH="$2"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      usage_error "unknown argument: '$1'"
      ;;
    *)
      POSITIONAL+=("$1")
      shift
      ;;
  esac
done

[ "${#POSITIONAL[@]}" -eq 1 ] || usage_error "exactly one <source-file> positional is required (got ${#POSITIONAL[@]})"
SOURCE_PATH="${POSITIONAL[0]}"
[ -n "$MOD_NAME" ] || usage_error "--mod is required (the test mod to extract)"
[ -n "$DEST_PATH" ] || usage_error "--dest is required (the child file the items move into)"
[[ "$MOD_NAME" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || usage_error "--mod must be a Rust identifier (got '$MOD_NAME')"

# --- plan-level validations (fail before anything is read or written) ------------
[ -f "$SOURCE_PATH" ] || die_plan "source file not found: $SOURCE_PATH"
[ -r "$SOURCE_PATH" ] || die_plan "source file cannot be read: $SOURCE_PATH"
[ -n "$HEADER_PATH" ] && [ ! -r "$HEADER_PATH" ] && die_plan "header file not found or unreadable: $HEADER_PATH"

SRC_ABS="$(cd "$(dirname "$SOURCE_PATH")" && pwd)/$(basename "$SOURCE_PATH")"
# the dest's parent may not exist yet (the first extraction of a hub creates
# the child dir): normalize without requiring it -- python's abspath finalizes
# the dest path, and it creates the parent only after the partition holds.
case "$DEST_PATH" in
  /*) DEST_CHECK="$DEST_PATH" ;;
  *) DEST_CHECK="$(pwd)/$DEST_PATH" ;;
esac
[ "$DEST_CHECK" != "$SRC_ABS" ] || die_plan "dest == source ($SRC_ABS): moving lines from a file to itself is not a cut"
[ -e "$DEST_CHECK" ] && die_plan "dest already exists (no clobber): $DEST_CHECK"

command -v python3 >/dev/null 2>&1 || die_tool "python3 is not available: the span lexer (file_size_preflight.sh) and the partition proof need it"
[ -f "$PREFLIGHT" ] || die_tool "the span owner file_size_preflight.sh is missing beside split_tests.sh: ONE Rust lexer in the program, never a second one"

# --- the span: asked, never re-derived --------------------------------------------
SPAN_RAW="$(bash "$PREFLIGHT" --lex-span "$SRC_ABS")" || SPAN_RC=$?
SPAN_RC="${SPAN_RC:-0}"
if [ -z "$SPAN_RAW" ]; then
  if [ "${SPAN_RC}" -ne 0 ]; then
    die_tool "the preflight's --lex-span failed (rc=$SPAN_RC): the tool never cuts a span it cannot see (fail-closed)"
  fi
  die_plan "no top-level braced mod named '$MOD_NAME' in $SOURCE_PATH (the file has no braced mods)"
fi
if [[ "$SPAN_RAW" == INDETERMINATE* ]]; then
  die_plan "the span of $SOURCE_PATH is ${SPAN_RAW%%$'\t'*}: ${SPAN_RAW#*$'\t'} -- an unprovable block is never cut"
fi

SPAN_RANGES=()
while IFS=$'\t' read -r _span_tag span_range span_name; do
  [ "${span_name:-}" = "$MOD_NAME" ] && SPAN_RANGES+=("${span_range:-}")
done < <(printf '%s\n' "$SPAN_RAW")

if [ "${#SPAN_RANGES[@]}" -eq 0 ]; then
  die_plan "no top-level braced mod named '$MOD_NAME' in $SOURCE_PATH"
fi
if [ "${#SPAN_RANGES[@]}" -gt 1 ]; then
  die_plan "ambiguous mod name '$MOD_NAME': ${#SPAN_RANGES[@]} top-level braced mods carry it (${SPAN_RANGES[*]})"
fi

SPAN_RANGE="${SPAN_RANGES[0]}"
SPAN_S="${SPAN_RANGE%%:*}"
SPAN_E="${SPAN_RANGE##*:}"

# --- the extraction: cut, dedent, declare, prove -- all-or-nothing ----------------
PY_RC=0
PY_OUT="$(python3 -I - "$SRC_ABS" "$DEST_PATH" "$MOD_NAME" "$SPAN_S" "$SPAN_E" "$HEADER_PATH" <<'PY'
import os
import re
import sys

src_path, dest_path, mod_name, s_1, e_1, header_path = (
    sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]), sys.argv[6],
)

with open(src_path, encoding="utf-8", errors="surrogateescape") as fh:
    text = fh.read()
had_trailing_newline = text.endswith("\n")
lines = text.split("\n")
if lines and lines[-1] == "":
    lines.pop()

header_lines = []
if header_path:
    with open(header_path, encoding="utf-8", errors="surrogateescape") as fh:
        header_text = fh.read()
    header_lines = header_text.split("\n")
    if header_lines and header_lines[-1] == "":
        header_lines.pop()

# the span's 1-based inclusive range: attributes ... mod <name> { ... }
s_idx, e_idx = s_1 - 1, e_1 - 1
mod_re = re.compile(r"^\s*(pub(\s*\([a-zA-Z ]+\))?\s+)?mod\s+" + re.escape(mod_name) + r"\s*\{\s*$")
mod_idx = None
for i in range(s_idx, e_idx + 1):
    if mod_re.match(lines[i]):
        mod_idx = i
        break
if mod_idx is None:
    print("defect: the span %d:%d does not open with 'mod %s {'" % (s_1, e_1, mod_name), file=sys.stderr)
    sys.exit(4)

cfg_re = re.compile(r"^\s*#\[cfg\([^)]*test[^)]*\)\s*\]\s*$")
attr_lines = lines[s_idx:mod_idx]
if not any(cfg_re.match(line) for line in attr_lines):
    print(
        "defect: the target mod '%s' carries no #[cfg(test)] attribute: "
        "split_tests.sh extracts test mods only (policy 4.3 step 5)" % mod_name,
        file=sys.stderr,
    )
    sys.exit(4)

# the mod's items: byte-verbatim, dedented once (the mod's inner indent)
body = lines[mod_idx + 1 : e_idx]
indents = [len(l) - len(l.lstrip(" ")) for l in body if l.strip()]
inner = min(indents) if indents else 0
moved = [l[inner:] if l.strip() else "" for l in body]
while moved and moved[-1] == "":
    moved.pop()

# the declaration: attributes verbatim, #[path] when the dest is not the
# standard child file (<source-stem>/<name>.rs beside the source)
src_dir = os.path.dirname(os.path.abspath(src_path))
stem = os.path.splitext(os.path.basename(src_path))[0]
dest_abs = os.path.abspath(dest_path)
standard_child = os.path.join(src_dir, stem, mod_name + ".rs")
decl = list(attr_lines)
if os.path.abspath(standard_child) != dest_abs:
    rel = os.path.relpath(dest_abs, src_dir)
    decl.append('#[path = "%s"]' % rel)
decl.append("mod %s;" % mod_name)

new_source_lines = lines[:s_idx] + decl + lines[e_idx + 1 :]

# --- the multiset proof ------------------------------------------------------------
# Test items: a fn whose contiguous attribute run carries a test-running
# attribute. The SAME deterministic rule scans all three sides, so the
# partition identity catches loss (a botched cut) AND duplication (the move
# left a copy behind) AND a header that adds items.
TEST_ATTR = re.compile(r"^\s*#\[\s*(test|tokio::test|rstest|test_case)(\([^)]*\))?\s*\]\s*$")
GENERIC_ATTR = re.compile(r"^\s*#\[.*\]\s*$")
FN_LINE = re.compile(r"^\s*(pub(\s*\([a-zA-Z ]+\))?\s+)?(async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)")


def test_items(text):
    out = []
    ls = text.split("\n")
    i = 0
    while i < len(ls):
        if TEST_ATTR.match(ls[i]):
            j = i + 1
            while j < len(ls) and (GENERIC_ATTR.match(ls[j]) or not ls[j].strip()):
                j += 1
            m = FN_LINE.match(ls[j]) if j < len(ls) else None
            if m:
                out.append(m.group(4))
            i = j
        else:
            i += 1
    return out


before = test_items(text)
after = test_items("\n".join(new_source_lines))
dest_items = test_items("\n".join(header_lines + moved))
body_items = test_items("\n".join(moved))
header_items = test_items("\n".join(header_lines))


def bag(seq):
    return sorted(seq)


if bag(dest_items) != bag(body_items):
    # the only content added ahead of the body is the header: a difference
    # here is the header's, and it breaks the pure move
    print(
        "defect: the header file adds or breaks test items (%d in header, %d in dest): "
        "the move must be pure -- imports only (--header-file)" % (len(header_items), len(dest_items)),
        file=sys.stderr,
    )
    sys.exit(4)
if bag(before) != bag(after + dest_items):
    print(
        "defect: the extraction does not partition (before=%d after=%d dest=%d): "
        "a cut that loses or duplicates items writes NOTHING (fail-closed)" % (len(before), len(after), len(dest_items)),
        file=sys.stderr,
    )
    sys.exit(1)

# --- writes (only after the partition holds) ----------------------------------------
os.makedirs(os.path.dirname(dest_abs) or ".", exist_ok=True)
dest_text = "\n".join(header_lines + moved) + "\n"
with open(dest_path, "w", encoding="utf-8", errors="surrogateescape") as fh:
    fh.write(dest_text)
new_text = "\n".join(new_source_lines) + ("\n" if had_trailing_newline else "")
with open(src_path, "w", encoding="utf-8", errors="surrogateescape") as fh:
    fh.write(new_text)

# --- the emission (CLOSED English key list) ------------------------------------------
decl_kind = "plain decl (the dest is the standard child file)" if os.path.abspath(standard_child) == dest_abs else (
    "#[path = \"%s\"] decl" % os.path.relpath(dest_abs, src_dir)
)
print("extracted\t%d:%d\tspan\tmod %s (cfg(test)); %d test items moved" % (s_1, e_1, mod_name, len(dest_items)))
print("dest\t%s\tfile\t%d test items + %d header lines; %s" % (dest_path, len(dest_items), len(header_lines), decl_kind))
print(
    "decl\t%s\tfile\t%smod %s;"
    % (src_path, ("#[path = \"%s\"] " % os.path.relpath(dest_abs, src_dir)) if os.path.abspath(standard_child) != dest_abs else "", mod_name)
)
print(
    "proof\tbefore=%d after=%d dest=%d\tpartition\tmultiset(source_before) == multiset(source_after) + multiset(dest)"
    % (len(before), len(after), len(dest_items))
)
PY
)" || PY_RC=$?

if [ "$PY_RC" -ne 0 ]; then
  # the python block printed its own NAMED cause; its rc is the tool's rc
  exit "$PY_RC"
fi
printf '%s\n' "$PY_OUT"
