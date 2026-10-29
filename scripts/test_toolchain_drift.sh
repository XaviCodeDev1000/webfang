#!/usr/bin/env bash
# Semantics harness for the toolchain pin drift gate (#2475).
#
# Three properties are behavioral and cannot be proven by grepping:
#
#   1. the gate FAILS on the drift it exists to catch. A drift check that
#      passes on a drifted tree is indistinguishable from no check at all, and
#      no check at all is the state this issue was filed against;
#   2. it PASSES on a coherent tree, including the nightly pins that carry
#      their own version and must not be flagged;
#   3. it fails CLOSED on what it cannot see: a toolchain file with no
#      channel, no workflow files, or no toolchain step at all. A guard that
#      degrades to "skip" when blind reports safety it never established.
#
# Everything runs against mktemp fixtures with their own git init. No network,
# no mutation of the repository this harness runs from.

set -euo pipefail
unset GITHUB_STEP_SUMMARY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

make_fixture() {
  local d="$WORK/$1"
  mkdir -p "$d/.github/workflows" "$d/scripts"
  git -C "$d" init -q
  cp "$REPO_ROOT/scripts/check_toolchain_drift.sh" "$d/scripts/"
  printf '%s\n' "$d"
}

expect_pass() {
  local name="$1" dir="$2"
  if ( cd "$dir" && ./scripts/check_toolchain_drift.sh >/dev/null 2>&1 ); then
    PASSED=$((PASSED + 1))
  else
    echo "FAIL: $name -- expected PASS, gate failed" >&2
    ( cd "$dir" && ./scripts/check_toolchain_drift.sh ) >&2 || true
    FAILED=$((FAILED + 1))
  fi
}

expect_fail() {
  local name="$1" dir="$2" needle="${3:-}"
  local out rc=0
  out="$( cd "$dir" && ./scripts/check_toolchain_drift.sh 2>&1 )" || rc=$?
  if [[ "$rc" -eq 0 ]]; then
    echo "FAIL: $name -- expected FAILURE, gate passed" >&2
    FAILED=$((FAILED + 1))
    return
  fi
  if [[ -n "$needle" && "$out" != *"$needle"* ]]; then
    echo "FAIL: $name -- failure message does not name the surface ('$needle')" >&2
    echo "$out" >&2
    FAILED=$((FAILED + 1))
    return
  fi
  PASSED=$((PASSED + 1))
}

# --- 1. coherent tree -------------------------------------------------------
D="$(make_fixture good)"
printf '[toolchain]\nchannel = "1.91.1"\n' > "$D/rust-toolchain.toml"
cat > "$D/.github/workflows/ok.yml" <<'YAML'
jobs:
  build:
    steps:
      - uses: dtolnay/rust-toolchain@6bed0761d98439e5a578e2877258200ad565ba87
        with:
          toolchain: 1.91.1
          targets: aarch64-unknown-linux-gnu
YAML
expect_pass "coherent tree passes" "$D"

# --- 2. targets: without toolchain: -- the v7.0.0 failure --------------------
D="$(make_fixture targets_no_toolchain)"
printf '[toolchain]\nchannel = "1.91.1"\n' > "$D/rust-toolchain.toml"
cat > "$D/.github/workflows/bad.yml" <<'YAML'
jobs:
  build:
    steps:
      - uses: dtolnay/rust-toolchain@688313b0823df1393bcebb1b4add0438a6d36884
        with:
          targets: aarch64-unknown-linux-gnu
YAML
# NEGATION: this is the v7.0.0 shape -- the action installed the 1.88.0 cross
# target while cargo resolved 1.91.1 from the toolchain file, and every static
# check stayed green because the step never named a toolchain at all.
expect_fail "targets without toolchain fails" "$D" "without an explicit toolchain:"
expect_fail "failure names file and line" "$D" ".github/workflows/bad.yml:"

# --- 3. explicit toolchain: that disagrees with the channel -----------------
D="$(make_fixture version_mismatch)"
printf '[toolchain]\nchannel = "1.91.1"\n' > "$D/rust-toolchain.toml"
cat > "$D/.github/workflows/bad.yml" <<'YAML'
jobs:
  build:
    steps:
      - uses: dtolnay/rust-toolchain@6bed0761d98439e5a578e2877258200ad565ba87
        with:
          toolchain: 1.88.0
          targets: aarch64-unknown-linux-gnu
YAML
# NEGATION: a step naming a version that disagrees with rust-toolchain.toml
# restores the split source of truth this gate exists to collapse.
expect_fail "toolchain mismatch fails" "$D" "rust-toolchain.toml declares [1.91.1]"

# --- 4. nightly pins carry their own version and are exempt -----------------
D="$(make_fixture nightly_exempt)"
printf '[toolchain]\nchannel = "1.91.1"\n' > "$D/rust-toolchain.toml"
cat > "$D/.github/workflows/miri.yml" <<'YAML'
jobs:
  miri:
    steps:
      - uses: dtolnay/rust-toolchain@02cb101ec7c40f2c49e1d9714d64511d8e1b74de
        with:
          toolchain: nightly-2026-08-27
          components: miri
YAML
expect_pass "nightly pins are exempt" "$D"

# --- 5. fail closed ---------------------------------------------------------
D="$(make_fixture no_channel)"
printf '[toolchain]\n' > "$D/rust-toolchain.toml"
cat > "$D/.github/workflows/ok.yml" <<'YAML'
jobs:
  build:
    steps:
      - uses: dtolnay/rust-toolchain@6bed0761d98439e5a578e2877258200ad565ba87
        with:
          toolchain: 1.91.1
YAML
# NEGATION: a toolchain file with no channel is unreadable, not consistent;
# the gate must not report safety it never established.
expect_fail "missing channel fails closed" "$D" "declares no"

D="$(make_fixture no_workflows)"
printf '[toolchain]\nchannel = "1.91.1"\n' > "$D/rust-toolchain.toml"
# NEGATION: no workflows is the blind-tree case -- a skip here would read as
# a pass and hide the pin entirely.
expect_fail "no workflow files fails closed" "$D" "no workflow files found"

D="$(make_fixture no_steps)"
printf '[toolchain]\nchannel = "1.91.1"\n' > "$D/rust-toolchain.toml"
cat > "$D/.github/workflows/ok.yml" <<'YAML'
jobs:
  build:
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09
YAML
# NEGATION: zero toolchain steps means the pin cannot be located at all; the
# vacuous pass is exactly what let the drift ship.
expect_fail "zero toolchain steps fails closed" "$D" "found zero"

echo
echo "toolchain drift harness: $PASSED passed, $FAILED failed"
if [[ "$FAILED" -gt 0 ]]; then
  exit 1
fi