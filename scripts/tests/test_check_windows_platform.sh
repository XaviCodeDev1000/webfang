#!/usr/bin/env bash
#
# test_check_windows_platform.sh — semantics harness for
# scripts/check_windows_platform.sh (#1833).
#
# What it pins — the case matrix of #1833:
#   1  rustup target missing          -> exit != 0 AND the message names the
#                                        exact `rustup target add
#                                        x86_64-pc-windows-msvc` command
#                                        (fail-closed, #1829 precedent;
#                                        driven through a PATH-stubbed rustup,
#                                        so it runs everywhere, CI included)
#   2  target installed, real module  -> exit 0 (also proves the path-mirrored
#                                        shim resolves the module's one
#                                        cross-crate import,
#                                        crate::application::crawler::ports::CTRL_C_EVENT_NAME)
#   3  target installed, compile error injected into a cfg(windows) arm
#                                     -> exit != 0 (the #1810 defect class:
#                                        a typo no Linux gate can see)
#   4  target installed, bare `unsafe` injected outside the module's scoped
#      #[allow(unsafe_code)] sites    -> exit != 0 (the mirrored workspace
#                                        lint table is load-bearing)
#
# Hermeticity: cases run against tmp copies via `--module`; the real tree is
# never modified. The scratch crate lives in mktemp -d and is cleaned by the
# script itself. The compile cache (CARGO_TARGET_DIR under
# ~/.cache/cargo-target/windows-typecheck-scratch) is intentionally shared
# across runs and callers: it holds content-addressed cargo artifacts only.
#
# Environment honesty: cases 2-4 need the MSVC std target
# (`rustup target add x86_64-pc-windows-msvc` — std only, no MSVC linker:
# cargo check never links). Where the target is absent they SKIP with a loud
# marker instead of pretending to pass; CI exercises case 1 until the
# repo-guards runner adds the target, which would promote 2-4 automatically.
#
# `--offline` runs ONLY case 1 (hermetic by construction: no cargo, no
# target needed). The ci_fast_gate lane passes it so the gate's sandboxed
# stub-cargo fixtures stay hermetic; the full matrix is what a developer
# runs by hand after touching the platform modules.
set -euo pipefail

OFFLINE=false
for arg in "$@"; do
  case "$arg" in
    --offline) OFFLINE=true ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK_SCRIPT="$SCRIPT_DIR/../check_windows_platform.sh"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REAL_MODULE="$REPO_ROOT/crates/webfang_core/src/infrastructure/platform/windows_console.rs"
TARGET="x86_64-pc-windows-msvc"

pass=0; fail=0; skip=0
note() { echo "test_check_windows_platform: $*"; }
expect_ok() { # description, command...
  local desc="$1"; shift
  if "$@" >/dev/null 2>&1; then note "PASS: $desc"; pass=$((pass + 1));
  else note "FAIL: $desc"; fail=$((fail + 1)); fi
}
expect_fails_with() { # description, needle, command...
  local desc="$1" needle="$2"; shift 2
  local out status
  if out="$("$@" 2>&1)"; status=$?; [[ $status -ne 0 ]]; then
    if [[ "$out" == *"$needle"* ]]; then
      note "PASS: $desc"; pass=$((pass + 1))
    else
      note "FAIL: $desc — exited $status but the message lacks the needle '$needle'"; fail=$((fail + 1))
    fi
  else
    note "FAIL: $desc — expected non-zero exit, got 0"; fail=$((fail + 1))
  fi
}

[[ -f "$CHECK_SCRIPT" ]] || { echo "harness cannot find $CHECK_SCRIPT" >&2; exit 1; }

# --- case 1: missing target fails loudly, naming the exact command ---------
# NEGATION: a missing rustup target must never degrade into a silent skip —
# the guard has to fail loudly naming the exact `rustup target add` fix (the
# #1829 silent-fallback precedent this repo codified).
# A stub rustup that reports only the host target, so the guard must refuse.
# The module is a synthetic fixture, not the real one: this case must behave
# identically in the gate's sandboxed fixtures (where crates/ is not copied)
# and on a developer tree, and it never compiles anything.
STUB_BIN="$(mktemp -d)"
cat > "$STUB_BIN/rustup" <<'STUB'
#!/usr/bin/env sh
case "$1" in target) echo "x86_64-unknown-linux-gnu (installed)";; *) exit 0;; esac
STUB
chmod +x "$STUB_BIN/rustup"
FIXTURE_MODULE="$(mktemp)"
printf '// harness fixture: enough for the target check to be reached\n' > "$FIXTURE_MODULE"
expect_fails_with \
  "missing target -> loud failure naming 'rustup target add $TARGET'" \
  "rustup target add $TARGET" \
  env PATH="$STUB_BIN:/usr/bin:/bin" bash "$CHECK_SCRIPT" --module "$FIXTURE_MODULE"
rm -rf "$STUB_BIN" "$FIXTURE_MODULE"

# --- cases 2-4: need the real MSVC std target ------------------------------
[[ -f "$REAL_MODULE" ]] || { echo "harness cannot find $REAL_MODULE" >&2; exit 1; }
if [[ "$OFFLINE" == "true" ]]; then
  note "--offline: compile cases (2-4) not run here; they need cargo and the MSVC std target"
  note "summary: pass=$pass fail=$fail skip=0 (offline scope)"
  [[ $fail -eq 0 ]]
  exit 0
fi
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  note "SKIP: cases 2-4 need '$TARGET' installed (std only, no MSVC linker): rustup target add $TARGET"
  note "summary: pass=$pass fail=$fail skip=$skip (compile cases not exercised in this environment)"
  [[ $fail -eq 0 ]]
  exit 0
fi

# --- case 2: the real module type-checks -----------------------------------
expect_ok "real module -> exit 0 (shim resolves the cross-crate import)" \
  bash "$CHECK_SCRIPT"

# --- case 3: compile error inside a cfg(windows) arm is caught -------------
# NEGATION: pre-#1833 a compile error inside a cfg(windows) arm was invisible
# to every Linux gate and only surfaced through the ~25-minute Windows CI
# round trip — #1810's missing `use tracing::warn;` shipped exactly this way.
FIXTURE="$(mktemp -d)"
cp "$REAL_MODULE" "$FIXTURE/module_with_type_error.rs"
cat >> "$FIXTURE/module_with_type_error.rs" <<'RS'

// Harness-injected defect (#1833 case 3): a type error no Linux gate can
// see, because the arm only compiles on Windows.
#[cfg(windows)]
fn harness_injected_type_error() {
    let _defect: u32 = "not a u32";
}
RS
expect_fails_with \
  "cfg(windows) arm compile error -> non-zero exit" \
  "windows_console.rs" \
  bash "$CHECK_SCRIPT" --module "$FIXTURE/module_with_type_error.rs"

# --- case 4: the mirrored lint table denies bare unsafe --------------------
# NEGATION: mirroring the workspace lint table is load-bearing — without it
# the scratch crate compiles a bare `unsafe` the workspace's
# `unsafe_code = "deny"` forbids, and #1810's second defect class ships
# silently on every Windows-only path.
cp "$REAL_MODULE" "$FIXTURE/module_with_bare_unsafe.rs"
cat >> "$FIXTURE/module_with_bare_unsafe.rs" <<'RS'

// Harness-injected defect (#1833 case 4): an unsafe block OUTSIDE the
// module's scoped #[allow(unsafe_code)] sites — the second #1810 defect
// class, caught only because the scratch crate mirrors the workspace lints.
#[cfg(windows)]
fn harness_injected_bare_unsafe() {
    unsafe { let _ = 1; }
}
RS
expect_fails_with \
  "bare unsafe outside scoped allows -> non-zero exit (mirrored lints)" \
  "usage of an \`unsafe\` block" \
  bash "$CHECK_SCRIPT" --module "$FIXTURE/module_with_bare_unsafe.rs"

rm -rf "$FIXTURE"

note "summary: pass=$pass fail=$fail skip=$skip"
[[ $fail -eq 0 ]]
