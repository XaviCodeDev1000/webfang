#!/usr/bin/env bash
# ============================================================================
# AI-PROVIDERS DOC DRIFT GUARD — docs/architecture/ai-providers-design.md
# ============================================================================
# What it protects: `docs/architecture/ai-providers-design.md` is a NORMATIVE spec
# (13 code files cite it) that lives OUTSIDE the mdBook book
# (`docs/book.toml` → `src = "src"`), so no gate read it. Measured consequence:
# a signature that did NOT compile —`AuthSource::resolve()` documented as
# `Result<ApiKey, AuthError>` when the real one is `Result<Option<ApiKey,
# AuthError>`— survived for months (#2136). The doc was fixed by hand (#2027/#2137);
# what was missing was the mechanism that notices the next one.
#
# This guard is the READER: the doc is audited against the code tree, not
# against its own prose. The manifest lives INSIDE the doc
# (`<!-- claims:begin -->` … `<!-- claims:end -->`), because the doc is the source
# of truth of its own claims and the guard only reads it. It is the same
# model as the version-pins guard: document with literals + source
# of truth + fail-closed check.
#
# Checks (ALL fail-closed):
#   C1 manifest   block present, parseable and non-empty fields, existing files,
#                 lines within the file
#   C2 claim      the cited line (trimmed) contains the expected substring,
#                 literal match (not regex)
#   C3 shape      the members of the declared type == the expected members, in
#                 BOTH directions (variants for enum, fields for struct)
#   C4 impl       the doc fence containing the declaration is contained in
#                 the cited file (containment, not set equality)
#   C5 absent     the symbol does NOT appear in the file (or in all of `crates/`)
#   C6 coverage   every `file:line` citation in the PROSE of the doc is registered
#                 in the manifest — a citation without a verifiable claim is drift
#   C7 §          every `§N` / `§Na` in the doc AND in the code that cites it resolves to
#                 a numbered section (`### N.`) or a lettered item (`- **(a)**`)
#
# Path normalization (S5), in order: (a) against the repo root, (b) against
# `crates/webfang_core/src/`, (c) by unique basename under `crates/`. If it
# does not resolve or is ambiguous → drift. It is never guessed.
#
# PATH ISOLATION: every path the guard READS is confined to the repo
# root. Without that isolation, a citation like `../../etc/passwd` would resolve to
# a real file OUTSIDE the tree and the guard would print its content in the
# CI log (C2/C3 diagnostic). The path is normalized against its base and the one
# that escapes is reported as drift WITHOUT being read.
#
# Usage: scripts/check_ai_providers_doc.sh [--root DIR] [--advisory]
# Output: 0 clean · 1 drift (fail-closed). `--advisory` NEVER fails on drift
# or on absent structure: it prints the finding and exits 0. An invocation
# invocation (unknown flag, `--root` that does not exist) still fails: it is not
# a finding, it is a malformed call.
# ============================================================================
set -uo pipefail

DOC_REL="docs/architecture/ai-providers-design.md"
CORE_SRC_REL="crates/webfang_core/src"

usage() {
  cat >&2 <<'EOF'
Usage: scripts/check_ai_providers_doc.sh [--root DIR] [--advisory]

  --root DIR   repo root (default: the root derived from BASH_SOURCE)
  --advisory   prints the same findings and ALWAYS exits 0

Output: 0 clean · 1 drift (fail-closed). With --advisory, drift and
absent structure (doc, manifest block, 'claims' fence, empty
manifest) exit 0 and the finding is printed anyway. An invocation error
still exits 1.
EOF
}

ROOT=""
ADVISORY=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --root)
      if [[ $# -lt 2 ]]; then
        printf '::error::falta el valor de --root\n' >&2
        exit 1
      fi
      ROOT="$2"
      shift 2
      ;;
    --root=*)
      ROOT="${1#--root=}"
      shift
      ;;
    --advisory)
      ADVISORY=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf '::error file=%s,line=1::argumento desconocido: %s\n' "$DOC_REL" "$1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$ROOT" ]]; then
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
if [[ ! -d "$ROOT" ]]; then
  printf '::error file=%s,line=1::the root %s does not exist\n' "$DOC_REL" "$ROOT" >&2
  exit 1
fi
ROOT="$(cd "$ROOT" && pwd)"

# Root canonicalization: path isolation compares by canonical
# form. `realpath -m` resolves the symlinks of the existing prefix and does not
# fail on nonexistent paths. Without `realpath` (macOS without coreutils) the
# isolation stays at its LEXICAL level — which already blocks `..` — and a warning
# is printed: a symlink INSIDE the repo pointing outside would not be detected.
REALPATH_OK=0
ROOT_CANON="$ROOT"
if command -v realpath >/dev/null 2>&1; then
  REALPATH_OK=1
  ROOT_CANON="$(realpath -m -- "$ROOT" 2>/dev/null || printf '%s' "$ROOT")"
else
  # The backticks are literal: it is the tool's name, not a
  # command substitution.
  # shellcheck disable=SC2016
  printf 'warning: `realpath` is not available — path isolation is only lexical (a symlink pointing outside the root is not detected)\n' >&2
fi

TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT

FINDINGS=0
report() { # file line message
  printf '::error file=%s,line=%s::%s\n' "$1" "$2" "$3" >&2
  FINDINGS=$((FINDINGS + 1))
}

# Structural output: the absent doc, the absent manifest block, the
# absent 'claims' fence and the empty manifest are FINDINGS, not a script
# malfunction. In `--advisory` mode they cannot abort the caller: the finding
# is printed and it exits 0, as in the final branch. Without this contract,
# `--advisory` only covered the drift detected at the end of the walk and an
# absent structure still exited 1.
abort_finding() { # line reason
  report "$DOC_REL" "$1" "$2"
  printf '::error::doc↔code drift guard: %d finding(s) in %s\n' "$FINDINGS" "$DOC_REL" >&2
  if [[ $ADVISORY -eq 1 ]]; then
    exit 0
  fi
  exit 1
}

# ─── normalization ───────────────────────────────────────────────────────────
# Collapses whitespace runs to a single space and removes the comma before `)` / `]`.
# It is applied equally to both sides of every comparison.
norm_stream() {
  sed -e 's/[[:space:]][[:space:]]*/ /g' -e 's/^ //' -e 's/ $//' \
      -e 's/,)/)/g' -e 's/,]/]/g'
}

norm() { printf '%s' "$1" | norm_stream; }

# A single-line comment (`//`, `///`) leaves the line empty: it is ignored.
strip_comment() { printf '%s' "${1%%//*}"; }

# The long dashes (–, —, −) are reduced to `-` so that the range
# recognizer is pure ASCII and does not depend on the locale.
EN_DASH="$(printf '\xe2\x80\x93')"
EM_DASH="$(printf '\xe2\x80\x94')"
MINUS_SIGN="$(printf '\xe2\x88\x92')"
DASH_SED="s/${EN_DASH}/-/g; s/${EM_DASH}/-/g; s/${MINUS_SIGN}/-/g"

# ─── path isolation and resolution (S5) ─────────────────────────────────────
# Isolation and resolution are the same door: a path the guard
# READS must stay confined to the repo root. Without that isolation, a
# citation like `../../etc/passwd` would resolve to a real file OUTSIDE the tree and
# the guard would print its content in the CI log (C2/C3 diagnostic).
#
# `confina <rel> <base>` normalizes `rel` (relative to the root) against `base`
# (relative to the root) and prints the result; it returns 1 if the path escapes
# `base`. The nesting level of the base is the FLOOR: a `..` cannot
# go below it. The isolation is LEXICAL — it does not depend on the path existing,
# and a path that escapes is not even looked at.
confina() { # rel base → normalized path relative to the root; 1 if it escapes
  local rel="$1" base="$2" seg
  local -a stack=() rel_segs=() base_segs=()
  IFS='/' read -r -a base_segs <<<"$base"
  for seg in ${base_segs[@]+"${base_segs[@]}"}; do
    case "$seg" in
      ''|'.') continue ;;
      '..')
        [[ ${#stack[@]} -gt 0 ]] || return 1
        stack=("${stack[@]:0:${#stack[@]}-1}")
        ;;
      *) stack+=("$seg") ;;
    esac
  done
  IFS='/' read -r -a rel_segs <<<"$rel"
  for seg in ${rel_segs[@]+"${rel_segs[@]}"}; do
    case "$seg" in
      ''|'.') continue ;;
      '..')
        [[ ${#stack[@]} -gt 0 ]] || return 1
        stack=("${stack[@]:0:${#stack[@]}-1}")
        ;;
      *) stack+=("$seg") ;;
    esac
  done
  [[ ${#stack[@]} -gt 0 ]] || return 1
  (IFS=/; printf '%s\n' "${stack[*]}")
}

# canon_under_root <rel>: 1 if the canonical form of the path escapes the canonical
# root — the case of the symlink INSIDE the repo pointing outside, which the
# lexical isolation cannot see.
canon_under_root() { # rel → 1 si escapa
  [[ $REALPATH_OK -eq 1 ]] || return 0
  local canon
  canon="$(realpath -m -- "$ROOT/$1" 2>/dev/null)" || return 1
  case "$canon" in
    "$ROOT_CANON"/*) return 0 ;;
    *) return 1 ;;
  esac
}

# reason for an unresolved path: 1 = it does not exist or is ambiguous, 2 = it escapes the
# root. The reason goes in the finding because they are distinct defects: the second
# is the one that can leak content from outside the tree.
motivo_ruta() { # rc → texto
  if [[ "$1" -eq 2 ]]; then
    printf 'escapes the repo root — the guard does not read any file outside it'
  else
    printf 'no existe, o el basename es ambiguo bajo crates/'
  fi
}

# ─── path resolution (S5) ────────────────────────────────────────────────────
resolve_path() { # path → prints the path relative to the repo; 1 if it does not resolve; 2 if it escapes the root
  local p="${1#./}" base count matches rel
  # (a) against the repo root
  if ! rel="$(confina "$p" "")"; then
    return 2
  fi
  if [[ -f "$ROOT/$rel" ]]; then
    canon_under_root "$rel" || return 2
    printf '%s\n' "$rel"
    return 0
  fi
  # (b) contra crates/webfang_core/src/
  if ! rel="$(confina "$p" "$CORE_SRC_REL")"; then
    return 2
  fi
  if [[ -f "$ROOT/$rel" ]]; then
    canon_under_root "$rel" || return 2
    printf '%s\n' "$rel"
    return 0
  fi
  # (c) by unique basename under crates/
  base="${p##*/}"
  matches="$(find "$ROOT/crates" -type f -name "$base" 2>/dev/null | sort)"
  count="$(printf '%s' "$matches" | grep -c . || true)"
  if [[ "$count" == "1" ]]; then
    if ! rel="$(confina "${matches#"$ROOT"/}" "")"; then
      return 2
    fi
    canon_under_root "$rel" || return 2
    printf '%s\n' "$rel"
    return 0
  fi
  return 1
}

# ─── normalized-file cache ───────────────────────────────────────────────────
# One entry PER PATH, and the cache WORKS. `norm_file` leaves the path in
# `NF_PATH` instead of printing it: a `$(...)` runs in a subshell, so
# the mutations of the arrays were lost and the cache was DEAD CODE — every
# call rewrote the file and the lookup never found anything.
#
# The cache file's key CANNOT be the path translated with
# `tr '/.' '__'`: that translation is not injective (`domain/x.a.rs` and
# `domain/x/a.rs` give the SAME name), so the second write overwrote the
# first and C4's containment was satisfied with the content of ANOTHER
# file — a false green, the worst possible direction. The entry name
# is carried by a counter (unique by construction) and the cache content is
# ALWAYS that of the cited file.
NORM_KEYS=()
NORM_VALS=()
NORM_N=0
NF_PATH=""
norm_file() { # relative path → NF_PATH: the temp file with the normalized lines
  local f="$1" i out
  for i in "${!NORM_KEYS[@]}"; do
    if [[ "${NORM_KEYS[$i]}" == "$f" ]]; then
      NF_PATH="${NORM_VALS[$i]}"
      return 0
    fi
  done
  NORM_N=$((NORM_N + 1))
  out="$TMPD/norm.$NORM_N"
  norm_stream <"$ROOT/$f" >"$out"
  NORM_KEYS+=("$f")
  NORM_VALS+=("$out")
  NF_PATH="$out"
}

# ─── member extraction from a type (C3) ──────────────────────────────────────
# The awk program carries `$0` and `$1`: they must reach the interpreter literally, so
# the single quote is mandatory.
# shellcheck disable=SC2016
AWK_MEMBERS='
NR < start { next }
NR == start {
  raw = $0
  n = gsub(/\{/, "{", raw); m = gsub(/\}/, "}", raw)
  depth = n - m
  next
}
{
  raw = $0
  if (depth == 1) {
    t = raw
    sub(/\/\/.*$/, "", t)
    gsub(/^[ \t]+/, "", t)
    if (kind == "enum") {
      if (t ~ /^[A-Za-z_][A-Za-z0-9_]*([ \t]*[,({=]|$)/) {
        name = t
        sub(/[ \t]*[,({=].*$/, "", name)
        sub(/[ \t]*$/, "", name)
        if (name != "") print name
      }
    } else {
      if (t ~ /^((pub(\([^)]*\))?[ \t]+)?)?[A-Za-z_][A-Za-z0-9_]*[ \t]*:/) {
        name = t
        sub(/^((pub(\([^)]*\))?[ \t]+)?)?/, "", name)
        sub(/[ \t]*:.*$/, "", name)
        sub(/[ \t]*$/, "", name)
        if (name != "") print name
      }
    }
  }
  n = gsub(/\{/, "{", raw); m = gsub(/\}/, "}", raw)
  depth += n - m
  if (depth <= 0) exit
}
'

extract_members() { # file line kind → prints one member per line
  awk -v start="$2" -v kind="$3" "$AWK_MEMBERS" "$1"
}

# ─── doc ─────────────────────────────────────────────────────────────────────
DOC="$ROOT/$DOC_REL"
if [[ ! -f "$DOC" ]]; then
  abort_finding 1 "the normative doc $DOC_REL does not exist under the root $ROOT"
fi
mapfile -t DOC_LINES <"$DOC"

# ─── doc fences ─────────────────────────────────────────────────────────────
FENCE_OPEN=()
FENCE_CLOSE=()
FENCE_LANG=()
FENCE_OPEN_RE='^```(.*)$'
FENCE_CLOSE_RE='^```[[:space:]]*$'
in_fence=0
fi=0
for i in "${!DOC_LINES[@]}"; do
  l="${DOC_LINES[$i]}"
  if [[ $in_fence -eq 0 ]]; then
    if [[ "$l" =~ $FENCE_OPEN_RE ]]; then
      in_fence=1
      FENCE_OPEN+=( "$i" )
      FENCE_LANG+=( "${BASH_REMATCH[1]}" )
      FENCE_CLOSE+=( -1 )
    fi
  else
    if [[ "$l" =~ $FENCE_CLOSE_RE ]]; then
      in_fence=0
      FENCE_CLOSE[fi]=$i
      fi=$((fi + 1))
    fi
  fi
done

# ─── manifest block (C1) ────────────────────────────────────────────────────
begin_idx=-1
end_idx=-1
for i in "${!DOC_LINES[@]}"; do
  t="${DOC_LINES[$i]//[[:space:]]/}"
  if [[ "$t" == "<!--claims:begin-->" && $begin_idx -lt 0 ]]; then
    begin_idx=$i
  elif [[ "$t" == "<!--claims:end-->" && $end_idx -lt 0 ]]; then
    end_idx=$i
  fi
done

if [[ $begin_idx -lt 0 || $end_idx -lt 0 || $end_idx -le $begin_idx ]]; then
  abort_finding 1 "C1: the manifest block does not exist (<!-- claims:begin --> … <!-- claims:end -->)"
fi

CLAIMS_FENCE_RE='^```claims[[:space:]]*$'
claims_start=-1
for ((i = begin_idx + 1; i < end_idx; i++)); do
  if [[ "${DOC_LINES[$i]}" =~ $CLAIMS_FENCE_RE ]]; then
    claims_start=$i
    break
  fi
done

if [[ $claims_start -lt 0 ]]; then
  abort_finding $((begin_idx + 1)) "C1: the manifest block does not contain a fence with info-string 'claims'"
fi

# ─── manifest parsing ───────────────────────────────────────────────────────
M_KIND=()
M_FILE=()
M_LINE=()
M_F4=()
M_F5=()
M_RES=()
M_AT=()
COVERED=()

is_covered() { # key "path:line"
  local key="$1" c
  for c in "${COVERED[@]+"${COVERED[@]}"}"; do
    [[ "$c" == "$key" ]] && return 0
  done
  return 1
}

i=$((claims_start + 1))
while [[ $i -lt $end_idx ]]; do
  raw="${DOC_LINES[$i]}"
  if [[ "$raw" =~ $FENCE_CLOSE_RE ]]; then
    break
  fi
  t="${raw#"${raw%%[![:space:]]*}"}"
  t="${t%"${t##*[![:space:]]}"}"
  i=$((i + 1))
  if [[ -z "$t" || "$t" == \#* ]]; then
    continue
  fi

  IFS='|' read -r -a parts <<<"$t"
  kind="${parts[0]//[[:space:]]/}"
  nfields="${#parts[@]}"

  expected=0
  case "$kind" in
    claim | impl | absent) expected=4 ;;
    shape) expected=5 ;;
    *)
      report "$DOC_REL" "$i" "C1: tipo de claim desconocido '$kind' (claim|shape|impl|absent)"
      continue
      ;;
  esac
  if [[ $nfields -ne $expected ]]; then
    report "$DOC_REL" "$i" "C1: la entrada '$kind' tiene $nfields campos y se esperaban $expected"
    continue
  fi

  file=""
  lineno=""
  f4=""
  f5=""
  empty=0
  for ((f = 1; f < nfields; f++)); do
    v="${parts[$f]}"
    v="${v#"${v%%[![:space:]]*}"}"
    v="${v%"${v##*[![:space:]]}"}"
    if [[ -z "$v" ]]; then
      empty=1
    fi
    case $f in
      1) file="$v" ;;
      2) lineno="$v" ;;
      3) f4="$v" ;;
      4) f5="$v" ;;
    esac
  done
  if [[ $empty -eq 1 ]]; then
    report "$DOC_REL" "$i" "C1: the entry '$kind' has an empty field"
    continue
  fi

  if [[ "$kind" == "absent" ]]; then
    if [[ "$lineno" != "-" ]]; then
      report "$DOC_REL" "$i" "C1: the 'absent' entry must carry '-' in the line field (it carries '$lineno')"
      continue
    fi
    M_KIND+=("$kind"); M_FILE+=("$file"); M_LINE+=("$lineno")
    M_F4+=("$f4"); M_F5+=("$f5"); M_RES+=(""); M_AT+=("$i")
    continue
  fi

  if [[ ! "$lineno" =~ ^[0-9]+$ ]] || [[ "$lineno" -lt 1 ]]; then
    report "$DOC_REL" "$i" "C1: the cited line '$lineno' is not a valid line number"
    continue
  fi

  res=""
  rc=0
  res="$(resolve_path "$file")" || rc=$?
  if [[ $rc -ne 0 ]]; then
    report "$DOC_REL" "$i" "C1: no se pudo resolver la ruta '$file' ($(motivo_ruta "$rc"))"
    continue
  fi

  total="$(awk 'END{print NR}' "$ROOT/$res")"
  if [[ "$lineno" -gt "$total" ]]; then
    report "$res" "$lineno" "C1: the cited line is outside the file ($res has $total lines)"
    continue
  fi

  M_KIND+=("$kind"); M_FILE+=("$file"); M_LINE+=("$lineno")
  M_F4+=("$f4"); M_F5+=("$f5"); M_RES+=("$res"); M_AT+=("$i")
  COVERED+=("$res:$lineno")
done

if [[ ${#M_KIND[@]} -eq 0 ]]; then
  abort_finding $((claims_start + 1)) "C1: the manifest is empty — the doc declares no verifiable claim"
fi

# ─── C2 / C3 / C4 / C5 ───────────────────────────────────────────────────────
checked_claim=0
checked_shape=0
checked_impl=0
checked_absent=0

find_fence_for_decl() { # normalized declaration → index of the fence
  local decl="$1" f c ln
  for f in "${!FENCE_OPEN[@]}"; do
    [[ "${FENCE_LANG[$f]}" == "claims" ]] && continue
    for ((c = FENCE_OPEN[f] + 1; c < FENCE_CLOSE[f]; c++)); do
      ln="$(norm "${DOC_LINES[$c]}")"
      if [[ "$ln" == "$decl" ]]; then
        printf '%s\n' "$f"
        return 0
      fi
    done
  done
  return 1
}

for ((e = 0; e < ${#M_KIND[@]}; e++)); do
  kind="${M_KIND[$e]}"
  res="${M_RES[$e]}"
  mfile="${M_FILE[$e]}"
  lineno="${M_LINE[$e]}"
  f4="${M_F4[$e]}"
  f5="${M_F5[$e]}"
  at="${M_AT[$e]}"

  case "$kind" in
    claim)
      checked_claim=$((checked_claim + 1))
      line="$(sed -n "${lineno}p" "$ROOT/$res")"
      trimmed="${line#"${line%%[![:space:]]*}"}"
      trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
      if [[ "$trimmed" != *"$f4"* ]]; then
        report "$res" "$lineno" "C2: the line does not contain '$f4' (content: '$trimmed')"
      fi
      ;;
    shape)
      checked_shape=$((checked_shape + 1))
      line="$(sed -n "${lineno}p" "$ROOT/$res")"
      trimmed="${line#"${line%%[![:space:]]*}"}"
      trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
      decl_n="$(norm "$f4")"
      if [[ "$trimmed" != *"$decl_n"* ]]; then
        report "$res" "$lineno" "C3: the line does not declare '$f4' (content: '$trimmed')"
        continue
      fi
      if [[ ! "$f4" =~ (enum|struct)[[:space:]]+([A-Za-z_][A-Za-z0-9_]*) ]]; then
        report "$res" "$lineno" "C3: no se pudo deducir el tipo declarado en '$f4'"
        continue
      fi
      tkind="${BASH_REMATCH[1]}"
      tname="${BASH_REMATCH[2]}"
      actual="$(extract_members "$ROOT/$res" "$lineno" "$tkind" | sort -u)"
      expected="$(printf '%s' "$f5" | tr ',' '\n' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' | grep -v '^$' | sort -u)"
      missing="$(comm -23 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual"))"
      extra="$(comm -13 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual"))"
      if [[ -n "$missing" ]]; then
        report "$res" "$lineno" "C3: '$tname' does not declare the member(s) $(printf '%s' "$missing" | tr '\n' ' ')— the manifest expects them"
      fi
      if [[ -n "$extra" ]]; then
        report "$res" "$lineno" "C3: '$tname' declares the member(s) $(printf '%s' "$extra" | tr '\n' ' ')— the manifest does not declare them"
      fi
      ;;
    impl)
      checked_impl=$((checked_impl + 1))
      decl_n="$(norm "$f4")"
      line="$(sed -n "${lineno}p" "$ROOT/$res")"
      code_n="$(norm "$line")"
      if [[ "$code_n" != *"$decl_n"* ]]; then
        report "$res" "$lineno" "C4: the line does not contain the declaration '$f4' (content: '$code_n')"
        continue
      fi
      fence=""
      if ! fence="$(find_fence_for_decl "$decl_n")"; then
        report "$res" "$lineno" "C4: no fence of the doc contains the declaration '$f4'"
        continue
      fi
      norm_file "$res"
      nf="$NF_PATH"
      for ((c = FENCE_OPEN[fence] + 1; c < FENCE_CLOSE[fence]; c++)); do
        dline="$(strip_comment "${DOC_LINES[$c]}")"
        dn="$(norm "$dline")"
        [[ -z "$dn" ]] && continue
        if ! grep -qF -- "$dn" "$nf"; then
          report "$res" "$lineno" "C4: the fence line '$dn' is not in $res"
        fi
      done
      ;;
    absent)
      checked_absent=$((checked_absent + 1))
      sym="$f4"
      if [[ "$mfile" == "*" ]]; then
        if [[ ! -d "$ROOT/crates" ]]; then
          report "$DOC_REL" "$at" "C5: crates/ does not exist — the absence of '$sym' cannot be verified"
          continue
        fi
        # The doc states "zero occurrences in `crates/`": it is searched in the WHOLE
        # tree, not only in the `.rs` files — a symbol mentioned in a fixture or in
        # an internal doc is also an occurrence.
        # Full-consumption form (issue #2925): head -n 1 closes the pipe after the
        # first line, so a symbol frequent enough to outgrow the 64 KiB buffer
        # EPIPEs the recursive grep mid-scan. This script has no `errexit`, so the
        # discarded status keeps the verdict correct today — but a future
        # `set -e` would abort here (or empty the hit) on a present symbol.
        # sed keeps reading to EOF and only reprints the first line, same status.
        hit="$(grep -rnF -- "$sym" "$ROOT/crates" 2>/dev/null | sed -n '1p')"
      else
        rc=0
        rfile="$(resolve_path "$mfile")" || rc=$?
        if [[ $rc -ne 0 ]]; then
          report "$DOC_REL" "$at" "C1: no se pudo resolver la ruta '$mfile' de la entrada 'absent' ($(motivo_ruta "$rc"))"
          continue
        fi
        # Same full-consumption form as above (issue #2925): head -n 1 would
        # EPIPE the producer once a single file's matches outgrow the buffer.
        hit="$(grep -nF -- "$sym" "$ROOT/$rfile" 2>/dev/null | sed -n '1p')"
      fi
      if [[ -n "$hit" ]]; then
        where="${hit%%:*}"
        where="${where#"$ROOT"/}"
        report "$where" 1 "C5: the symbol '$sym' appears in the code, but the manifest declares its absence"
      fi
      ;;
  esac
done

# ─── C6: coverage of the prose citations ─────────────────────────────────────
# A `file:line` citation (or the abbreviated `:line`, with the file as context)
# must be registered in the manifest. The abbreviated ones are only accepted
# inside backticks (`:102`) so as not to be confused with another notation.
# The recognizers go in single quotes: the backticks are literal.
# shellcheck disable=SC2016
CIT_RE='`:[0-9]+(-[0-9]+)?`|[A-Za-z0-9_][A-Za-z0-9_./-]*\.(rs|md|json|toml|sh|yaml|yml)(:[0-9]+(-[0-9]+)?)?'
# shellcheck disable=SC2016
SHORT_RE='^`:([0-9]+)(-[0-9]+)?`$'
PATHLINE_RE='^(.+):([0-9]+)(-[0-9]+)?$'

check_citation() { # path line range-end doc-line
  local p="$1" ln="$2" lnend="$3" dln="$4" res rc
  rc=0
  res="$(resolve_path "$p")" || rc=$?
  if [[ $rc -ne 0 ]]; then
    report "$DOC_REL" "$dln" "C6: the path '$p' of a doc citation could not be resolved ($(motivo_ruta "$rc"))"
    return
  fi
  if ! is_covered "$res:$ln"; then
    report "$DOC_REL" "$dln" "C6: the citation '$p:$ln' is not registered in the manifest"
  fi
  if [[ -n "$lnend" && "$lnend" != "$ln" ]]; then
    if ! is_covered "$res:$lnend"; then
      report "$DOC_REL" "$dln" "C6: the end '$p:$lnend' of the range is not registered in the manifest"
    fi
  fi
}

citations=0
ctx=""
for ((i = 0; i < ${#DOC_LINES[@]}; i++)); do
  [[ $i -ge $begin_idx && $i -le $end_idx ]] && continue
  line="${DOC_LINES[$i]}"
  [[ "$line" == *:* ]] || continue
  while IFS= read -r tok; do
    [[ -z "$tok" ]] && continue
    if [[ "$tok" =~ $SHORT_RE ]]; then
      s_start="${BASH_REMATCH[1]}"
      s_end="${BASH_REMATCH[2]}"
      if [[ -z "$ctx" ]]; then
        report "$DOC_REL" $((i + 1)) "C6: la cita abreviada ':$s_start' no tiene un archivo de contexto"
        continue
      fi
      citations=$((citations + 1))
      check_citation "$ctx" "$s_start" "$s_end" $((i + 1))
    else
      if [[ "$tok" =~ $PATHLINE_RE ]]; then
        p="${BASH_REMATCH[1]}"
        ln="${BASH_REMATCH[2]}"
        lnend="${BASH_REMATCH[3]#-}"
        ctx="$p"
        citations=$((citations + 1))
        check_citation "$p" "$ln" "$lnend" $((i + 1))
      else
        ctx="$tok"
      fi
    fi
  done < <(printf '%s\n' "$line" | grep -oE "$CIT_RE" || true)
done

# ─── C7: referencias § ──────────────────────────────────────────────────────
declare -A SEC_OK=()
declare -A ITEM_OK=()
cur_sec=""
for i in "${!DOC_LINES[@]}"; do
  l="${DOC_LINES[$i]}"
  if [[ "$l" == \#\#* ]]; then
    cur_sec=""
    if [[ "$l" =~ ^###[[:space:]]+([0-9]+)\. ]]; then
      cur_sec="${BASH_REMATCH[1]}"
      SEC_OK["$cur_sec"]=1
    fi
    continue
  fi
  # The item is declared as `- **(a) Text**`: the letter goes after the parenthesis,
  # and the `**` that closes the bold can be much further away.
  if [[ -n "$cur_sec" && "$l" =~ ^-[[:space:]]+\*\*\(([a-z])\) ]]; then
    ITEM_OK["${cur_sec}${BASH_REMATCH[1]}"]=1
  fi
done

SEC_RE='§[0-9]+[a-z]?(-§?[0-9]+[a-z]?)?'
SEC_BODY_RE='^([0-9]+[a-z]?)(-§?([0-9]+[a-z]?))?$'

check_sec_one() { # number-letter file line token
  local nl="$1" f="$2" ln="$3" tok="$4" n l
  n="${nl%%[a-z]*}"
  l="${nl#"$n"}"
  if [[ -z "${SEC_OK[$n]:-}" ]]; then
    report "$f" "$ln" "C7: the reference '$tok' does not resolve to any numbered section of the doc (§$n)"
    return
  fi
  if [[ -n "$l" && -z "${ITEM_OK["$n$l"]:-}" ]]; then
    report "$f" "$ln" "C7: the reference '$tok' does not resolve to any lettered item of section §$n"
  fi
}

check_sec_token() { # token file line
  local tok="$1" f="$2" ln="$3" body first second
  body="${tok#§}"
  first="$body"
  second=""
  if [[ "$body" =~ $SEC_BODY_RE ]]; then
    first="${BASH_REMATCH[1]}"
    second="${BASH_REMATCH[3]}"
  fi
  check_sec_one "$first" "$f" "$ln" "$tok"
  if [[ -n "$second" ]]; then
    check_sec_one "$second" "$f" "$ln" "$tok"
  fi
}

secs=0
for ((i = 0; i < ${#DOC_LINES[@]}; i++)); do
  [[ $i -ge $begin_idx && $i -le $end_idx ]] && continue
  line="${DOC_LINES[$i]}"
  [[ "$line" == *§* ]] || continue
  while IFS= read -r tok; do
    [[ -z "$tok" ]] && continue
    secs=$((secs + 1))
    check_sec_token "$tok" "$DOC_REL" $((i + 1))
  done < <(printf '%s\n' "$line" | sed -e "$DASH_SED" | grep -oE "$SEC_RE" || true)
done

# The code that cites the doc must also resolve its §.
while IFS= read -r cf; do
  [[ -n "$cf" ]] || continue
  rel="${cf#"$ROOT"/}"
  while IFS= read -r hit; do
    [[ -n "$hit" ]] || continue
    cline="${hit%%:*}"
    ctext="${hit#*:}"
    while IFS= read -r tok; do
      [[ -z "$tok" ]] && continue
      secs=$((secs + 1))
      check_sec_token "$tok" "$rel" "$cline"
    done < <(printf '%s\n' "$ctext" | sed -e "$DASH_SED" | grep -oE "$SEC_RE" || true)
  done < <(grep -nE "$SEC_RE" "$cf" | sed -e "$DASH_SED" || true)
done < <(grep -rlF --include='*.rs' "ai-providers-design.md" "$ROOT/crates" 2>/dev/null | sort)

# ─── resultado ───────────────────────────────────────────────────────────────
if [[ $FINDINGS -gt 0 ]]; then
  printf '::error::doc↔code drift guard: %d finding(s) in %s\n' "$FINDINGS" "$DOC_REL" >&2
  if [[ $ADVISORY -eq 1 ]]; then
    exit 0
  fi
  exit 1
fi

printf 'OK: %s — %d claim, %d shape, %d impl, %d absent; %d cita(s) de prosa cubiertas; %d referencia(s) § resueltas.\n' \
  "$DOC_REL" "$checked_claim" "$checked_shape" "$checked_impl" "$checked_absent" "$citations" "$secs"
exit 0
