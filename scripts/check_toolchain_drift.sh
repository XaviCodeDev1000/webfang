#!/usr/bin/env bash
# Guard the toolchain action pin against rust-toolchain.toml (#2475).
#
# WHY THIS EXISTS
#   The Rust version is declared in two places that nothing related:
#
#     rust-toolchain.toml                 channel = "1.91.1"      <- greppable
#     .github/workflows/*.yml             dtolnay/rust-toolchain@<sha>
#
#   The version behind that `<sha>` does NOT live in this repository. It lives
#   inside the action's own action.yml, on whichever branch the commit belongs
#   to, so `grep 1.91.1` cannot see it. When #2310 bumped the channel from
#   1.88.0 to 1.91.1 the workflow pin stayed on the 1.88.0 branch: the action
#   installed the aarch64 cross target on 1.88.0, rust-toolchain.toml made
#   cargo resolve 1.91.1, and the build died with
#
#     error[E0463]: can't find crate for `core`
#
#   v7.0.0 shipped nothing because of it. #2455 repointed the SHA and wrote the
#   arrangement down as a "DRIFT TRAP" in a comment. A comment is not a gate:
#   the next bump repeats the failure and the only detector is a broken
#   release. This is that gate.
#
# THE INVARIANT, AND WHY IT IS NARROW
#   A mismatched pin is only fatal for a step that installs `targets:`. Host
#   builds are fine either way, because rustup's toolchain-file override
#   outranks the installed default -- which is exactly why only the aarch64
#   entry failed in v7.0.0 while the other three targets passed.
#
#   R1  a step that sets `targets:` MUST also set `toolchain:` explicitly.
#   R2  every explicit `toolchain:` MUST equal rust-toolchain.toml's channel,
#       except `nightly*` pins (miri, fuzz and sanitizers carry their own).
#
# FAIL-CLOSED, NEVER SKIP
#   A missing or unreadable channel, no workflow files, or zero toolchain
#   steps found are ERRORS. A guard that degrades to "skip" when it cannot see
#   is indistinguishable from a guard that does not exist, which is the state
#   this issue was filed against.
#
# Read-only: it never writes to the repository.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

ERRORS=0
err() {
  echo "ERROR: $*" >&2
  ERRORS=$((ERRORS + 1))
}

CHANNEL="$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' rust-toolchain.toml 2>/dev/null | head -1)"
if [[ -z "$CHANNEL" ]]; then
  err "rust-toolchain.toml is missing or declares no \`channel\`; nothing can be verified against it."
fi

shopt -s nullglob
FILES=(.github/workflows/*.yml .github/workflows/*.yaml)
if (( ${#FILES[@]} == 0 )); then
  err "no workflow files found under .github/workflows; the guard has nothing to read and must not report success."
fi

# Emits one TSV row per `dtolnay/rust-toolchain` step: <file:line>\t<toolchain>\t<targets>.
# The block is delimited by indentation: everything deeper than the `- uses:`
# line belongs to the step, and the next line at or above that indent closes it.
READ_STEPS=0
while IFS=$'\t' read -r LOC TC TG; do
  [[ -z "$LOC" ]] && continue
  # awk emits "-" for an absent key: a bare tab would be collapsed by IFS,
  # silently shifting the targets value into the toolchain column.
  [[ "$TC" == "-" ]] && TC=""
  [[ "$TG" == "-" ]] && TG=""
  READ_STEPS=$((READ_STEPS + 1))

  if [[ -n "$TG" && -z "$TC" ]]; then
    err "$LOC installs targets: [$TG] without an explicit toolchain:. The target would land on whatever toolchain that action rev hardcodes, while cargo resolves $CHANNEL from rust-toolchain.toml -- the E0463 failure that emptied v7.0.0."
  fi

  if [[ -n "$TC" && -n "$CHANNEL" ]]; then
    case "$TC" in
      nightly*) : ;;
      "$CHANNEL") : ;;
      *) err "$LOC pins toolchain: [$TC] but rust-toolchain.toml declares [$CHANNEL]. Bump both, or pass the channel verbatim." ;;
    esac
  fi
# awk with no file arguments reads stdin and blocks forever, so the empty
# case is short-circuited; the error above already failed the run.
done < <( (( ${#FILES[@]} > 0 )) && awk '
  function ind(s) { match(s, /^ */); return RLENGTH }
  function emit() { printf "%s\t%s\t%s\n", cur, (tc == "" ? "-" : tc), (tg == "" ? "-" : tg) }
  /^[ ]*(-[ ]+)?uses:[ ]*dtolnay\/rust-toolchain@/ {
    if (cur != "") emit()
    cur = FILENAME ":" FNR; base = ind($0); tc = ""; tg = ""
    next
  }
  {
    if (cur == "") next
    if ($0 ~ /^[ ]*$/) next
    if (ind($0) <= base) { emit(); cur = ""; next }
    if ($0 ~ /^[ ]*toolchain:[ ]*/) { v = $0; sub(/^[ ]*toolchain:[ ]*/, "", v); sub(/[ ]+#.*$/, "", v); gsub(/["\047]/, "", v); tc = v }
    if ($0 ~ /^[ ]*targets?:[ ]*/) { v = $0; sub(/^[ ]*targets?:[ ]*/, "", v); sub(/[ ]+#.*$/, "", v); gsub(/["\047]/, "", v); tg = v }
  }
  END { if (cur != "") emit() }
' "${FILES[@]}" || true)

if (( READ_STEPS == 0 )); then
  err "found zero dtolnay/rust-toolchain steps across ${#FILES[@]} workflow file(s). That is not a pass: it means the guard stopped seeing what it exists to check."
fi

if (( ERRORS > 0 )); then
  echo "::error title=toolchain pin drift::The workflow toolchain pins disagree with rust-toolchain.toml ($ERRORS problem(s))." >&2
  exit 1
fi

echo "OK: toolchain pins coherent (channel=$CHANNEL, steps=$READ_STEPS)."