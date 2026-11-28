#!/usr/bin/env bash
# Semantics harness for the bootstrap a producer performs on the tree it
# creates — T20c, #3125.
#
# THE DEFECT THIS PINS. `scripts/ci_batch_branch.sh` created a worktree and then
# PRINTED a recipe instead of performing it: four lines carrying a literal
# `$TREE`, plus a note telling the operator to run `scripts/worktree_identity.sh`
# themselves. `git worktree add` materialises neither file, so a fresh tree is
# uncommittable until a human remembers two steps — and following the printed
# text literally (a quoted heredoc, or skipping the `TREE=$(basename)` line)
# writes an `.envrc` declaring `.../cargo-target/$TREE`, a path no tree can own.
#
# That is not hypothetical. A real `.envrc` in this plan shipped with the
# literal, `githooks/identity-gate.sh` denied every commit in that tree, and
# `scripts/test_target_isolation_envrc.sh` stayed green throughout — because it
# proves the POLICY over fixtures it writes itself and never reads an artifact a
# producer produced. A guard that proves the copy leaves the copy free to rot.
#
# So this harness runs the mechanism and judges the ARTIFACT: the `.envrc` and
# the identity manifest the producer must leave behind, read back through the two
# real guards that decide whether a tree may be built and committed.
#
# NEGATION: case 5 rewrites `.envrc` in the historical defective form and
# requires BOTH guards to refuse it. If the artifact assertions could not go red,
# they would be the same decoration this file exists to remove.
#
# HERMETIC: one throwaway worktree under a scratch dir, removed on exit. No
# network, no cargo, no test suite.
#
# Usage: bash scripts/tests/test_batch_branch_bootstrap.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BASH_DIR="$ROOT/scripts/lib/worktree_bootstrap.sh"
ISOLATION="$ROOT/scripts/check_target_isolation.sh"
IDENTITY="$ROOT/scripts/githooks/identity-gate.sh"
SCRATCH="$(mktemp -d)"
SUFFIX="$$-$RANDOM"
BRANCH="chore/t20c-boot-$SUFFIX"
WT="$SCRATCH/chore-t20c-boot-$SUFFIX"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "ok   $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL $1"; }

cleanup() {
  git -C "$ROOT" worktree remove --force "$WT" >/dev/null 2>&1 || true
  git -C "$ROOT" branch -D "$BRANCH" >/dev/null 2>&1 || true
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

# --- case 1: the mechanism the producer is supposed to share exists ----------
if [ -f "$BASH_DIR" ]; then
  pass "the bootstrap mechanism exists at scripts/lib/worktree_bootstrap.sh"
else
  fail "no scripts/lib/worktree_bootstrap.sh — the producer still prints a recipe instead of performing it"
fi

# --- case 2: the producer calls that mechanism -------------------------------
# A mechanism nobody invokes is a library, not a fix.
if grep -qE 'worktree_bootstrap\.sh' "$ROOT/scripts/ci_batch_branch.sh" 2>/dev/null; then
  pass "ci_batch_branch.sh performs the bootstrap instead of printing it"
else
  fail "ci_batch_branch.sh never calls the bootstrap mechanism: the recipe stays a human step"
fi

if [ ! -f "$BASH_DIR" ]; then
  echo ""
  echo "FAIL: the bootstrap mechanism is missing, so the artifact cases cannot run."
  echo "      $PASS passed, $((FAIL + 1)) failed"
  exit 1
fi

# --- a real tree, created the way a producer creates one --------------------
git -C "$ROOT" worktree add -q -b "$BRANCH" "$WT" HEAD >/dev/null 2>&1
if [ ! -d "$WT" ]; then
  fail "could not create a scratch worktree ($WT)"
  echo "      $PASS passed, $FAIL failed"
  exit 1
fi

bash "$BASH_DIR" "$WT" >/dev/null 2>&1
rc=$?

# --- case 3: the produced .envrc declares the tree's OWN name ---------------
if [ "$rc" -ne 0 ]; then
  fail "the bootstrap mechanism exited $rc on a normal tree"
elif [ ! -f "$WT/.envrc" ]; then
  fail "the bootstrap wrote no .envrc: the tree inherits the caller's target dir"
elif grep -q '\$TREE' "$WT/.envrc"; then
  fail "the produced .envrc still carries the literal \$TREE — no tree can own that path"
else
  declared="$(sed -n 's/^export CARGO_TARGET_DIR=//p' "$WT/.envrc" | head -1)"
  if [ "$(basename "$WT")" = "$(basename "${declared%/}")" ] || [[ "$declared" == *"/$(basename "$WT")" ]]; then
    pass "the produced .envrc declares this tree's own name"
  else
    fail "the produced .envrc declares '$declared', not this tree"
  fi
fi

# --- case 4: the identity manifest is produced, not remembered --------------
if [ -f "$WT/.worktree-identity" ]; then
  pass "the bootstrap wrote the identity manifest"
else
  fail "no .worktree-identity: the tree is uncommittable until a human remembers"
fi

# --- case 5: both real guards accept the produced artifact ------------------
# shellcheck disable=SC1090
set -a; . "$WT/.envrc" 2>/dev/null; set +a
out="$(bash "$ISOLATION" --root "$WT" 2>&1)"; iso_rc=$?
if [ "$iso_rc" -eq 0 ] && grep -q 'own-registered-target' <<<"$out"; then
  pass "check_target_isolation.sh accepts the tree the producer bootstrapped"
else
  fail "check_target_isolation.sh refused the produced tree (rc=$iso_rc): $(tail -1 <<<"$out")"
fi

( cd "$WT" && touch .harness-probe && git add -A ) >/dev/null 2>&1
( cd "$WT" && bash "$IDENTITY" ) >/dev/null 2>&1; id_rc=$?
if [ "$id_rc" -eq 0 ]; then
  pass "the identity gate accepts a commit in the produced tree"
else
  fail "the identity gate denied the produced tree (rc=$id_rc): the artifact is not committable"
fi

# --- case 6 (NEGATION): the historical defective form must be refused -------
# This is the part that makes the file a harness. The literal `$TREE` form is
# exactly what the printed recipe produced in a real tree; both guards must
# reject it, or the assertions above prove nothing.
printf 'export CARGO_TARGET_DIR=$HOME/.cache/cargo-target/$TREE\nexport CARGO_INCREMENTAL=0\nunset RUSTC_WRAPPER\nexport CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/cargo-target/$TREE-llvm-cov\n' > "$WT/.envrc"
out="$(bash "$ISOLATION" --root "$WT" 2>&1)"; iso_rc=$?
if [ "$iso_rc" -ne 0 ]; then
  pass "the isolation guard refuses the literal \$TREE form"
else
  fail "the isolation guard accepted the literal \$TREE form: it cannot fail"
fi

( cd "$WT" && CARGO_TARGET_DIR="$HOME/.cache/cargo-target/$(basename "$WT")" bash "$IDENTITY" ) >/dev/null 2>&1; id_rc=$?
if [ "$id_rc" -ne 0 ]; then
  pass "the identity gate refuses the literal \$TREE form"
else
  fail "the identity gate accepted the literal \$TREE form: it cannot fail"
fi

# --- summary ----------------------------------------------------------------
echo ""
if [ "$FAIL" -gt 0 ]; then
  echo "FAIL: $FAIL case(s) failed, $PASS passed"
  exit 1
fi
echo "OK: $PASS case(s) passed"
exit 0
