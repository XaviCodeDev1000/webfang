#!/usr/bin/env bash
# scripts/tests/test_llm_artifact_degrade.sh — the per-crate API rendering of
# scripts/build-llm-artifact.sh degrades EXPLICITLY instead of aborting the
# whole LLM artifact.
#
# Why: the Docs workflow is red because rustdoc-markdown 0.91.0 pins its own
# toolchain, nightly-2025-11-27 (rustc 1.93.0-nightly), with no override, while
# the workspace declares rust-version = 1.100. `cargo rustdoc` rejects the
# workspace before rustdoc runs:
#   error: rustc 1.93.0-nightly is not supported by the following packages:
#     webfang_core@9.2.4 requires rustc 1.100
# Owner decision (S4): the API section leaves the Docs gate, but the artifact
# must stay HONEST — a directory without 01-/03-/04- must never look complete,
# which is exactly the "partial directory that LOOKS publishable" failure the
# script's ATOMIC PUBLICATION contract exists to prevent.
#
# Deterministic and hermetic: no network, no real rustdoc, no mdbook build. A
# stub `rustdoc-markdown` on PATH reproduces both states:
#   degraded — exits non-zero with the cargo MSRV rejection on stderr
#   rendered — prints a minimal "# <crate> API" document
# The narrative chapters are the REAL docs/src ones, so the harness cannot pass
# on a fixture that hides a chapter dependency.
#
# Pins:
#   1. the degraded run exits 0 and publishes the base file set only;
#   2. no 01-/03-/04- API file exists and llms.txt references none of them;
#   3. llms.txt and webfang-docs-full.md carry the availability note and the
#      upstream issue URL;
#   4. the degrade path is CONDITIONAL: a succeeding renderer produces the
#      whole file set and no note.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/build-llm-artifact.sh"
SRC_DOCS="$ROOT/docs/src"
UPSTREAM_URL="https://github.com/puddlers/rustdoc-markdown/issues/1"
NOTE_MARKER="Crate API chapters are unavailable"
BASE_FILES=(llms.txt 00-narrative.md 06-cli-reference.md webfang-docs-full.md)
API_FILES=(01-webfang_core.md 03-webfang_mcp.md 04-webfang_test_utils.md)

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0
ok() { printf '  PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf '  FAIL  %s\n' "$1"; fail=$((fail + 1)); }

[[ -f "$SCRIPT" ]] || {
  echo "FAIL: missing $SCRIPT" >&2
  exit 1
}
[[ -f "$SRC_DOCS/SUMMARY.md" ]] || {
  echo "FAIL: missing $SRC_DOCS/SUMMARY.md" >&2
  exit 1
}

make_workspace() { # <dir>
  mkdir -p "$1/docs/src" "$1/docs/book" "$1/stub"
  cp "$SRC_DOCS"/*.md "$1/docs/src/"
}

# The failure the real tool produces under the pinned nightly: cargo refuses
# the workspace on its MSRV check, nothing is rendered, no output file exists.
make_failing_renderer() { # <dir>
  cat >"$1/stub/rustdoc-markdown" <<'STUBEOF'
#!/usr/bin/env bash
echo "error: rustc 1.93.0-nightly is not supported by the following packages:" >&2
echo "  webfang_core@9.2.4 requires rustc 1.100" >&2
echo "  wreq@0.16.0 requires rustc 1.98" >&2
exit 101
STUBEOF
  chmod +x "$1/stub/rustdoc-markdown"
}

make_succeeding_renderer() { # <dir>
  cat >"$1/stub/rustdoc-markdown" <<'STUBEOF'
#!/usr/bin/env bash
out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --output)
      out="$2"
      shift 2
      ;;
    *) shift ;;
  esac
done
printf '# 0.1.0\n\n## Structs\n\nMinimal stub API document.\n' >"$out"
STUBEOF
  chmod +x "$1/stub/rustdoc-markdown"
}

# A fixed SOURCE_DATE_EPOCH keeps both runs byte-comparable and independent of
# the machine clock (the script's REPRODUCIBLE DATE contract).
run_artifact() { # <workspace> — stdout+stderr are captured by the caller
  ( cd "$1" && PATH="$1/stub:$PATH" SOURCE_DATE_EPOCH=1700000000 bash "$SCRIPT" )
}

# Anything in docs/book besides the published artifact itself is scratch that
# should not survive the run (a staging directory or the renderer's captured
# stderr). It would make the published directory ambiguous.
other_entries() { # <workspace>
  local e
  for e in "$1"/docs/book/* "$1"/docs/book/.*; do
    [ -e "$e" ] || continue
    case "${e##*/}" in
      webfang-docs-llm | . | ..) continue ;;
    esac
    printf '%s ' "${e##*/}"
  done
}

echo "build-llm-artifact.sh — degraded API rendering"

# --- Degraded run: the renderer cannot run under the workspace MSRV --------
degraded="$TMP/degraded"
make_workspace "$degraded"
make_failing_renderer "$degraded"
art="$degraded/docs/book/webfang-docs-llm"

rc=0
output="$(run_artifact "$degraded" 2>&1)" || rc=$?
if [ "$rc" -eq 0 ]; then
  ok "degraded run exits 0 (the whole Docs gate no longer fails on the API renderer)"
else
  bad "degraded run exits 0 (rc=$rc)"
  printf '%s\n' "$output" | tail -20 | sed 's/^/        /'
fi

if printf '%s\n' "$output" | grep -q '::warning::'; then
  ok "degraded run emits a GitHub warning annotation"
else
  bad "degraded run emits a GitHub warning annotation"
fi
if printf '%s\n' "$output" | grep -qF "$UPSTREAM_URL"; then
  ok "the annotation names the upstream issue"
else
  bad "the annotation names the upstream issue"
fi

missing=""
for f in "${BASE_FILES[@]}"; do
  [ -s "$art/$f" ] || missing="$missing $f"
done
if [ -z "$missing" ]; then
  ok "the degraded artifact still publishes the narrative + CLI base file set"
else
  bad "the degraded artifact still publishes the narrative + CLI base file set (missing:$missing)"
fi

present=""
for f in "${API_FILES[@]}"; do
  [ -e "$art/$f" ] && present="$present $f"
done
if [ -z "$present" ]; then
  ok "no 01-/03-/04- API file is published by the degraded run"
else
  bad "no 01-/03-/04- API file is published by the degraded run (found:$present)"
fi

if [ -s "$art/llms.txt" ]; then
  ok "llms.txt exists in the degraded artifact"
  listed=""
  for f in "${API_FILES[@]}"; do
    grep -qF "$f" "$art/llms.txt" && listed="$listed $f"
  done
  if [ -z "$listed" ]; then
    ok "llms.txt lists none of the missing API files"
  else
    bad "llms.txt lists none of the missing API files (listed:$listed)"
  fi
  if grep -qF 'all crate APIs' "$art/llms.txt"; then
    bad "llms.txt does not claim the crate APIs are included"
  else
    ok "llms.txt does not claim the crate APIs are included"
  fi
  if grep -qF "$NOTE_MARKER" "$art/llms.txt" && grep -qF "$UPSTREAM_URL" "$art/llms.txt"; then
    ok "llms.txt carries the availability note and the upstream issue URL"
  else
    bad "llms.txt carries the availability note and the upstream issue URL"
    sed 's/^/        /' "$art/llms.txt"
  fi
else
  bad "llms.txt exists in the degraded artifact"
fi

if [ -s "$art/webfang-docs-full.md" ]; then
  if grep -qF "$NOTE_MARKER" "$art/webfang-docs-full.md" &&
    grep -qF "$UPSTREAM_URL" "$art/webfang-docs-full.md"; then
    ok "webfang-docs-full.md carries the same availability note"
  else
    bad "webfang-docs-full.md carries the same availability note"
  fi
  stray=""
  for f in "${API_FILES[@]}"; do
    grep -qF "$f" "$art/webfang-docs-full.md" && stray="$stray $f"
  done
  if [ -z "$stray" ]; then
    ok "webfang-docs-full.md embeds no missing API section"
  else
    bad "webfang-docs-full.md embeds no missing API section (found:$stray)"
  fi
else
  bad "webfang-docs-full.md exists in the degraded artifact"
fi

if [ -z "$(other_entries "$degraded")" ]; then
  ok "the degraded run leaves no staging or scratch entry in docs/book"
else
  bad "the degraded run leaves no staging or scratch entry in docs/book (found: $(other_entries "$degraded"))"
fi

# --- Rendered run: the degrade path is conditional, not unconditional ------
rendered="$TMP/rendered"
make_workspace "$rendered"
make_succeeding_renderer "$rendered"
art="$rendered/docs/book/webfang-docs-llm"

rc=0
output="$(run_artifact "$rendered" 2>&1)" || rc=$?
if [ "$rc" -eq 0 ]; then
  ok "rendered run exits 0"
else
  bad "rendered run exits 0 (rc=$rc)"
  printf '%s\n' "$output" | tail -20 | sed 's/^/        /'
fi

missing=""
for f in "${BASE_FILES[@]}" "${API_FILES[@]}"; do
  [ -s "$art/$f" ] || missing="$missing $f"
done
if [ -z "$missing" ]; then
  ok "a working renderer publishes the full file set (API chapters included)"
else
  bad "a working renderer publishes the full file set (API chapters included) (missing:$missing)"
fi

if [ -s "$art/llms.txt" ]; then
  listed=""
  for f in "${API_FILES[@]}"; do
    grep -qF "$f" "$art/llms.txt" || listed="$listed $f"
  done
  if [ -z "$listed" ]; then
    ok "the complete index lists every API file"
  else
    bad "the complete index lists every API file (missing:$listed)"
  fi
  if grep -qF "$NOTE_MARKER" "$art/llms.txt"; then
    bad "the complete artifact carries no unavailability note"
  else
    ok "the complete artifact carries no unavailability note"
  fi
else
  bad "llms.txt exists in the rendered artifact"
fi

if [ -z "$(other_entries "$rendered")" ] && [ -d "$art" ]; then
  ok "the rendered run publishes only the artifact directory in docs/book"
else
  bad "the rendered run publishes only the artifact directory in docs/book (found: $(other_entries "$rendered"))"
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "llm artifact degrade: $pass passed, 0 failed"
  exit 0
fi
echo "llm artifact degrade: $pass passed, $fail failed"
exit 1
