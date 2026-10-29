#!/usr/bin/env bash
# Semantics harness for the real-tree golden WRITER.
#
# Companion to the differential test tools/intra-crate-lint/tests/real_tree.rs,
# which asserts the extractor's output over crates/webfang_core/src against the
# committed golden fixture. This file pins the semantics of the WRITER
# (scripts/refresh_real_tree_golden.sh), the piece that makes the fixture's
# contract ("refreshed in the same commit that changes the tree", #2499)
# executable without a hand-rolled procedure.
#
# Why this file exists: until #2499 there was no writer. Every contributor
# rediscovered the procedure by hand — build the tool, run it with --root,
# redirect into the fixture — from a red "Intra-crate lint tool tests" CI step.
# The fixture moved 43 times in 30 days (#2178, #2381 and #2353 all paid the
# toll) and #2338 showed the hand-rolled step is even merge-conflict-prone.
#
# The properties that cannot be proven by grepping the script:
#
#   1. it WRITES the extractor's stdout byte-identically into the fixture —
#      the same bytes real_tree.rs asserts at the differential test;
#   2. it is IDEMPOTENT: a second run over an unchanged tree changes nothing
#      and still exits 0;
#   3. it honours $INTRA_CRATE_LINT_BIN EXCLUSIVELY (the gate's own resolution
#      rule in resolve_lint_bin) and fails closed when the variable names a
#      non-executable path;
#   4. it fails CLOSED, with the fixture INTACT, when the build fails or the
#      extractor reports parse errors: a partial golden set written from a
#      broken tree is a lie the differential test would inherit;
#   5. it refuses an EMPTY extraction over a real tree: a zero-byte golden set
#      would silently unpin every site while reporting success.
#
# Everything runs against mktemp fixtures. The writer is copied into the
# fixture byte-identically (cmp-verified) and driven there; cargo is STUBBED on
# PATH, so the suite compiles nothing: no cargo, no network, and it never reads
# or writes the repository this harness runs from.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

WRITER="scripts/refresh_real_tree_golden.sh"

# A harness that cannot see its subject fails instead of passing: without this
# check the first `cp` below dies with a raw "No such file or directory" and
# the run says nothing about what was being tested.
if [[ ! -f "$REPO_ROOT/$WRITER" ]]; then
  echo "::error::the harness has no subject: $WRITER is missing" >&2
  printf '\nharness: 0 ok, 1 fail\n'
  exit 1
fi

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

FIXTURE_REL="tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt"

# The TSV the stubbed extractor emits — exactly the shape the CLI contract
# guarantees: `<relative-file>\t<line>\t<kind>\t<path>`, one record per line.
CANNED_TSV="$WORK/canned.tsv"
printf 'domain/ssrf_guard.rs\t135\tuse\tcrate::application::llm_extraction::LLM_SSRF_TEST_HATCH_ENV\nadapters/downloader/mod.rs\t106\tuse\tcrate::domain::ValidUrl\napplication/container.rs\t88\tinline\tcrate::infrastructure::observability::file_trace\n' > "$CANNED_TSV"

# new_fixture <name>: a throwaway checkout-shaped repo with the writer under
# test copied in byte-identically, the real layout the writer expects
# (tools/intra-crate-lint + crates/webfang_core/src), a STALE golden fixture,
# and a git history so the writer's diff display has something to show.
new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/scripts" \
           "$dir/tools/intra-crate-lint/tests/fixtures" \
           "$dir/tools/intra-crate-lint/src" \
           "$dir/crates/webfang_core/src/domain"
  cp "$REPO_ROOT/$WRITER" "$dir/scripts/"
  cmp -s "$REPO_ROOT/$WRITER" "$dir/scripts/refresh_real_tree_golden.sh" \
    || { echo "::error::the copied writer is not byte-identical to the original" >&2; exit 1; }
  chmod +x "$dir/scripts/refresh_real_tree_golden.sh"
  printf '[package]\nname = "intra-crate-lint"\nversion = "0.0.0"\n' > "$dir/tools/intra-crate-lint/Cargo.toml"
  printf '//! placeholder\n' > "$dir/crates/webfang_core/src/domain/mod.rs"
  printf 'adapters/old.rs\t1\tuse\tcrate::domain::Stale\n' > "$dir/$FIXTURE_REL"
  git -C "$dir" init -q
  git -C "$dir" -c user.email=harness@invalid -c user.name=harness add -A
  git -C "$dir" -c user.email=harness@invalid -c user.name=harness commit -qm "fixture"
  printf '%s\n' "$dir"
}

# The cargo STUB: validates the invocation shape the writer must use (build,
# --locked, the tool's own manifest), proves CARGO_TARGET_DIR was unset for the
# build (the binary must land in tools/intra-crate-lint/target, where the gate
# looks too), records that it ran, and "compiles" the fake extractor into
# place. FAKE_CARGO_FAIL simulates a broken build.
mkdir -p "$WORK/stub"
cat > "$WORK/stub/cargo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ -z "${FAKE_CARGO_FAIL:-}" ]] || { echo "cargo stub: simulated build failure" >&2; exit 1; }
[[ "${1:-}" == "build" ]] || { echo "cargo stub: expected 'build', got '${1:-}'" >&2; exit 64; }
[[ " $* " == *" --locked "* ]] || { echo "cargo stub: the build must be --locked" >&2; exit 64; }
[[ " $* " == *"--manifest-path tools/intra-crate-lint/Cargo.toml"* ]] || { echo "cargo stub: unexpected manifest path: $*" >&2; exit 64; }
[[ -z "${CARGO_TARGET_DIR:-}" ]] || { echo "cargo stub: CARGO_TARGET_DIR leaked into the build — the binary would land outside tools/intra-crate-lint/target" >&2; exit 64; }
: > "$STUB_CARGO_MARKER"
mkdir -p "$STUB_TOOL_TARGET/debug"
cp "$STUB_EXTRACTOR_SRC" "$STUB_TOOL_TARGET/debug/intra-crate-lint"
chmod +x "$STUB_TOOL_TARGET/debug/intra-crate-lint"
STUB
chmod +x "$WORK/stub/cargo"

# The fake extractor the stub "builds": emits the canned TSV, unless a case
# asks it to fail (FAKE_EXTRACTOR_FAIL) or to report zero sites
# (FAKE_EXTRACTOR_EMPTY).
cat > "$WORK/fake-extractor" <<'EXT'
#!/usr/bin/env bash
if [[ -n "${FAKE_EXTRACTOR_FAIL:-}" ]]; then
  printf 'intra-crate-lint: cannot parse %s: unexpected token\n' "$FAKE_EXTRACTOR_FAIL" >&2
  exit 2
fi
if [[ -n "${FAKE_EXTRACTOR_EMPTY:-}" ]]; then
  exit 0
fi
cat "$FAKE_TSV"
EXT
chmod +x "$WORK/fake-extractor"

# run_writer <dir>: stdout and stderr are captured separately (the success
# lines are the only place the writer reports WHAT it did), PATH is stubbed so
# `cargo` is the stub, and the invocation marker is reset per run.
run_writer() {
  local dir="$1"
  (
    cd "$dir" || exit 1
    export STUB_CARGO_MARKER="$WORK/cargo-invoked"
    export STUB_TOOL_TARGET="$dir/tools/intra-crate-lint/target"
    export STUB_EXTRACTOR_SRC="$WORK/fake-extractor"
    export FAKE_TSV="$CANNED_TSV"
    export PATH="$WORK/stub:$PATH"
    rm -f "$WORK/cargo-invoked"
    bash scripts/refresh_real_tree_golden.sh >"$WORK/stdout" 2>"$WORK/stderr"
  )
}

# ─── 1. a stale golden is rewritten byte-identically ─────────────────────────
# NEGATION: pre-#2499 there was no writer; the regeneration was rediscovered by
# hand from a red CI step. A writer that only reports drift leaves the
# tree/fixture pairing exactly as broken as before.
D="$(new_fixture stale-golden)"
if run_writer "$D"; then
  if cmp -s "$CANNED_TSV" "$D/$FIXTURE_REL" && [[ -f "$WORK/cargo-invoked" ]]; then
    pass "a stale golden is rewritten with the extractor's stdout, byte for byte (build ran)"
  else
    fail "a stale golden is rewritten byte for byte ($(cat "$D/$FIXTURE_REL" | head -1))"
  fi
else
  fail "a stale golden is rewritten byte for byte ($(cat "$WORK/stderr"))"
fi

# ─── 2. idempotent: the second run changes nothing and still exits 0 ─────────
# The writer may be run repeatedly over the same tree while a PR evolves; a
# second run must be a no-op, not an empty diff or a failure.
# NEGATION: pre-#2499 every refresh was a fresh hand-rolled redirect, so
# nothing guaranteed "run it again" was safe.
D="$(new_fixture idempotent)"
if run_writer "$D"; then
  cp "$D/$FIXTURE_REL" "$WORK/after-first"
  if run_writer "$D"; then
    if cmp -s "$D/$FIXTURE_REL" "$WORK/after-first" && grep -q "nothing to do" "$WORK/stdout"; then
      pass "the second run changes nothing and exits 0"
    else
      fail "the second run changes nothing ($(cat "$WORK/stdout"))"
    fi
  else
    fail "the second run changes nothing and exits 0 ($(cat "$WORK/stderr"))"
  fi
else
  fail "the second run changes nothing and exits 0 (setup: $(cat "$WORK/stderr"))"
fi

# ─── 3. $INTRA_CRATE_LINT_BIN is used EXCLUSIVELY ────────────────────────────
# The gate resolves the binary through the same variable and treats it as an
# exclusive request; the writer must honour it too, never building a different
# binary behind the caller's back.
# NEGATION: pre-#2499 the hand-rolled procedure had no such rule — whichever
# binary happened to be around was the one that measured.
D="$(new_fixture explicit-bin)"
cat > "$WORK/explicit-extractor" <<'EXT'
#!/usr/bin/env bash
printf 'domain/explicit.rs\t7\tuse\tcrate::adapters::Z\n'
EXT
chmod +x "$WORK/explicit-extractor"
if INTRA_CRATE_LINT_BIN="$WORK/explicit-extractor" run_writer "$D"; then
  if printf 'domain/explicit.rs\t7\tuse\tcrate::adapters::Z\n' | cmp -s - "$D/$FIXTURE_REL" \
      && [[ ! -f "$WORK/cargo-invoked" ]]; then
    pass "an explicit \$INTRA_CRATE_LINT_BIN is used exclusively (cargo never invoked)"
  else
    fail "an explicit \$INTRA_CRATE_LINT_BIN is used exclusively ($(cat "$D/$FIXTURE_REL" | head -1))"
  fi
else
  fail "an explicit \$INTRA_CRATE_LINT_BIN is used exclusively ($(cat "$WORK/stderr"))"
fi

# ─── 4. a non-executable $INTRA_CRATE_LINT_BIN fails closed ──────────────────
# The gate refuses the same shape (resolve_lint_bin): a request that cannot be
# honoured must not silently become a different binary.
# NEGATION: pre-#2499 the hand-rolled procedure would fall back to whatever
# binary existed, measuring with a tool the caller did not name.
D="$(new_fixture bad-bin)"
cp "$D/$FIXTURE_REL" "$WORK/bad-bin-before"
if INTRA_CRATE_LINT_BIN="$WORK/no-such-extractor" run_writer "$D"; then
  fail "a non-executable \$INTRA_CRATE_LINT_BIN fails closed"
elif cmp -s "$D/$FIXTURE_REL" "$WORK/bad-bin-before" && grep -q "INTRA_CRATE_LINT_BIN" "$WORK/stderr"; then
  pass "a non-executable \$INTRA_CRATE_LINT_BIN fails closed, fixture intact, naming the variable"
else
  fail "a non-executable \$INTRA_CRATE_LINT_BIN fails closed naming the variable ($(cat "$WORK/stderr"))"
fi

# ─── 5. a failed build fails closed and leaves the fixture intact ────────────
# NEGATION: pre-#2499 the hand-rolled redirect (`> fixture`) truncated the file
# BEFORE the command ran, so a broken build could leave an empty golden set
# behind — a lie the differential test would then inherit.
D="$(new_fixture build-fails)"
cp "$D/$FIXTURE_REL" "$WORK/build-fails-before"
if FAKE_CARGO_FAIL=1 run_writer "$D"; then
  fail "a failed build fails closed"
elif cmp -s "$D/$FIXTURE_REL" "$WORK/build-fails-before" && grep -q "failed to build" "$WORK/stderr"; then
  pass "a failed build fails closed and the fixture is intact"
else
  fail "a failed build fails closed and the fixture is intact ($(cat "$WORK/stderr"))"
fi

# ─── 6. extractor parse errors fail closed, fixture intact, errors named ─────
# The gate's own contract: a parse error is never a silently skipped file. The
# writer inherits it — a tree that does not scan completely must not produce a
# golden set at all.
# NEGATION: pre-#2499 a hand-rolled redirect captured whatever came out before
# the failure, pinning a partial measurement as the golden truth.
D="$(new_fixture parse-errors)"
cp "$D/$FIXTURE_REL" "$WORK/parse-errors-before"
if FAKE_EXTRACTOR_FAIL=broken.rs run_writer "$D"; then
  fail "extractor parse errors fail closed"
elif cmp -s "$D/$FIXTURE_REL" "$WORK/parse-errors-before" && grep -q "broken.rs" "$WORK/stderr"; then
  pass "extractor parse errors fail closed, fixture intact, the offending file named"
else
  fail "extractor parse errors fail closed naming the file ($(cat "$WORK/stderr"))"
fi

# ─── 7. an empty extraction over a real tree is refused ──────────────────────
# A zero-byte golden set would unpin every site while reporting success — the
# extractor contract guarantees at least the tree's own sites, so empty output
# is a malfunction, not a measurement.
# NEGATION: pre-#2499 nothing stood between a broken extraction and a truncated
# fixture; the differential test would then fail with "the number of sites
# changed" and no explanation of when the pinning happened.
D="$(new_fixture empty-output)"
cp "$D/$FIXTURE_REL" "$WORK/empty-output-before"
if FAKE_EXTRACTOR_EMPTY=1 run_writer "$D"; then
  fail "an empty extraction is refused"
elif cmp -s "$D/$FIXTURE_REL" "$WORK/empty-output-before" && grep -q "zero sites" "$WORK/stderr"; then
  pass "an empty extraction is refused and the fixture is intact"
else
  fail "an empty extraction is refused ($(cat "$WORK/stderr"))"
fi

# ─── 8. the result is shown as a diff over the fixture ───────────────────────
# The writer's consumer reviews the diff before committing it; a silent
# rewrite would bury the golden change inside an unreviewed commit.
# NEGATION: pre-#2499 the hand-rolled redirect showed nothing — the diff was
# whatever `git status` surfaced after the fact.
D="$(new_fixture diff-display)"
if run_writer "$D"; then
  if grep -q "real_tree_sites.txt" "$WORK/stdout"; then
    pass "the resulting diff names the fixture"
  else
    fail "the resulting diff names the fixture ($(cat "$WORK/stdout"))"
  fi
else
  fail "the resulting diff names the fixture ($(cat "$WORK/stderr"))"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]
