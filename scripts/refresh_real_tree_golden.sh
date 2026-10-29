#!/usr/bin/env bash
# ============================================================================
# REAL-TREE GOLDEN SET — writer: regenerates the intra-crate-lint golden
# fixture from the current crates/webfang_core/src tree
# ============================================================================
# Companion to tools/intra-crate-lint/tests/real_tree.rs and to
# scripts/check_intra_crate_direction.sh. Where the differential test COMPARES,
# this script REGENERATES.
#
# WHY THIS EXISTS
# The golden set pins the extractor's complete output over the real tree, and
# its contract (the module doc of real_tree.rs) demands the refresh land in the
# SAME commit that changes the tree. Until #2499 there was no writer: every
# contributor rediscovered the procedure by hand — build the tool, run it with
# --root, redirect into the fixture — from a red "Intra-crate lint tool tests"
# step. The fixture moved 43 times in 30 days (#2178, #2381 and #2353 all paid
# the toll), and #2338 showed the hand-rolled step is even merge-conflict-prone.
#
# CONTRACT
#   - writes ONLY tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt;
#   - the bytes are exactly what real_tree.rs asserts: the extractor's stdout
#     over crates/webfang_core/src (the CLI contract of main.rs — one TSV
#     record per site, `<relative-file>\t<line>\t<kind>\t<path>`);
#   - fails CLOSED without touching the fixture when the build fails or the
#     extractor exits non-zero (a parse error is never a silently skipped
#     file): a partial golden set written from a broken tree is a lie the
#     differential test would inherit;
#   - refuses an EMPTY extraction: a zero-byte golden set would silently
#     unpin every site while reporting success;
#   - is idempotent: a second run over an unchanged tree says "nothing to do"
#     and exits 0 without rewriting;
#   - honours $INTRA_CRATE_LINT_BIN EXCLUSIVELY when set — the same resolution
#     rule the gate uses (resolve_lint_bin in check_intra_crate_direction.sh):
#     a request that cannot be honoured must not silently become a different
#     binary.
#
# Usage: scripts/refresh_real_tree_golden.sh
# Output: 0 ok · 1 failure (`::error::` on stderr, like the gate)
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

err() { printf '::error::%s\n' "$*" >&2; exit 1; }

TOOL_DIR="$REPO_ROOT/tools/intra-crate-lint"
ROOT_DIR="$REPO_ROOT/crates/webfang_core/src"
FIXTURE="$TOOL_DIR/tests/fixtures/real_tree_sites.txt"
FIXTURE_REL="tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt"

[[ -d "$ROOT_DIR" ]] || err "missing source dir $ROOT_DIR — run this from a checkout that has crates/webfang_core/src"
[[ -d "$TOOL_DIR" ]] || err "missing tool dir $TOOL_DIR"

# --- extractor resolution (the gate's own rule, no fallback) -----------------
if [[ -n "${INTRA_CRATE_LINT_BIN:-}" ]]; then
  if [[ ! -x "$INTRA_CRATE_LINT_BIN" ]]; then
    err "INTRA_CRATE_LINT_BIN is set to '$INTRA_CRATE_LINT_BIN' but that path is not an executable file — failing closed (no implicit fallback binary)"
  fi
  BIN="$INTRA_CRATE_LINT_BIN"
else
  # The build must land in tools/intra-crate-lint/target (where the gate looks
  # too), so CARGO_TARGET_DIR is unset for the build: a worktree's pinned
  # shared target dir would send the binary where neither this script nor the
  # gate can find it — the same warning resolve_lint_bin prints.
  if ! (cd "$REPO_ROOT" && env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR \
      cargo build --manifest-path tools/intra-crate-lint/Cargo.toml --locked); then
    err "the intra-crate-lint tool failed to build — the golden set was left untouched"
  fi
  BIN="$TOOL_DIR/target/debug/intra-crate-lint"
  [[ -x "$BIN" ]] || err "the extractor binary is missing after a successful build: $BIN — failing closed"
fi

# --- measure (decide-then-write: nothing touches the fixture until the
# extractor has scanned the WHOLE tree successfully) --------------------------
OUT="$(mktemp)"
ERR="$(mktemp)"
trap 'rm -f "$OUT" "$ERR"' EXIT

lint_rc=0
"$BIN" --root "$ROOT_DIR" >"$OUT" 2>"$ERR" || lint_rc=$?
if [[ $lint_rc -ne 0 ]]; then
  printf '::error::the extractor failed (exit %d) — the golden set was left untouched\n' "$lint_rc" >&2
  while IFS= read -r err_line; do
    [[ -n "$err_line" ]] && printf '::error::%s\n' "$err_line" >&2
  done < "$ERR"
  exit 1
fi

[[ -s "$OUT" ]] \
  || err "the extractor reported zero sites over $ROOT_DIR — refusing to write an empty golden set"

# --- write -------------------------------------------------------------------
if cmp -s "$OUT" "$FIXTURE"; then
  printf 'OK: the golden set already matches the tree (%s) — nothing to do\n' "$FIXTURE_REL"
  exit 0
fi

# A same-directory temp keeps the write local, and `cat >` (not `mv`) keeps the
# fixture's own mode and inode — the same choice bump_docs_version_pins.sh
# makes. Every failure above has already exited: this is the first byte that
# reaches the file.
TMP_FIXTURE="$(mktemp "$TOOL_DIR/tests/fixtures/.real_tree_sites.XXXXXX")"
trap 'rm -f "$OUT" "$ERR" "$TMP_FIXTURE"' EXIT
cat "$OUT" > "$TMP_FIXTURE"
cat "$TMP_FIXTURE" > "$FIXTURE"

# --- show the result ---------------------------------------------------------
if git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git -C "$REPO_ROOT" diff --stat -- "$FIXTURE_REL" || true
fi
printf 'OK: golden set refreshed — commit %s in the SAME commit that changes crates/webfang_core/src (the contract in tools/intra-crate-lint/tests/real_tree.rs)\n' "$FIXTURE_REL"
