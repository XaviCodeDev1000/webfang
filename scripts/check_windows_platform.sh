#!/usr/bin/env bash
# Type-check the cfg(windows) platform modules on a non-Windows host (#1833).
#
# The Linux gate never compiles the #[cfg(windows)] arms of
# crates/webfang_core/src/infrastructure/platform/windows_console.rs, and the
# Windows CI lane is a ~25-minute round trip that only reports after the fact.
# Two real defects already slipped through exactly this gap on PR #1810: a
# missing `use tracing::warn;` and bare `unsafe` blocks the workspace's
# `unsafe_code = "deny"` lint would have rejected.
#
# The fix is the procedure #1810 validated by hand, turned into a script that
# fails closed: it generates a throwaway crate under $TMPDIR carrying a copy
# of the real module (cp, nothing checked into the tree), resolves the
# module's cross-crate imports against path-mirrored shims, mirrors the
# workspace lint table, and runs `cargo check --target x86_64-pc-windows-msvc`.
# The check never links, so no MSVC toolchain is needed — but the target's
# std must be installed; a missing target is a loud failure naming the exact
# `rustup target add` command, never a silent skip.
#
# Usage:
#   bash scripts/check_windows_platform.sh                 # check the real module
#   bash scripts/check_windows_platform.sh --module PATH   # check a copy (harness)
#   bash scripts/check_windows_platform.sh --keep-scratch  # debug: keep + print the scratch crate
set -euo pipefail

TARGET="x86_64-pc-windows-msvc"
MODULE=""
KEEP_SCRATCH=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --module)
      [[ $# -ge 2 ]] || { echo "::error::--module needs a path" >&2; exit 2; }
      MODULE="$2"; shift 2 ;;
    --keep-scratch) KEEP_SCRATCH=true; shift ;;
    *) echo "::error::unknown argument: $1" >&2; exit 2 ;;
  esac
done

REPO_ROOT="$(git rev-parse --show-toplevel)"
MODULE="${MODULE:-$REPO_ROOT/crates/webfang_core/src/infrastructure/platform/windows_console.rs}"

fail() { echo "::error::$1" >&2; exit 1; }

[[ -f "$MODULE" ]] || fail "module not found: $MODULE (run from inside the repo, or pass --module)"

# Fail closed on a missing target, naming the exact fix — the #1829
# target-isolation precedent: a silent fallback nobody notices is worse than
# a loud failure.
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  echo "::error::rustup target '$TARGET' is not installed — the cfg(windows) arms cannot be type-checked." >&2
  echo "       fix: rustup target add $TARGET" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Workspace-linked config, extracted at runtime so nothing here rots:
# edition, the [workspace.lints] table (mirrored verbatim as [lints]) and the
# workspace dependency entries for the crates the module uses.
# ---------------------------------------------------------------------------
EDITION="$(awk '/^\[workspace\.package\]/{f=1;next} f && /^edition = /{gsub(/"/,"",$3); print $3; exit}' "$REPO_ROOT/Cargo.toml")"
[[ -n "$EDITION" ]] || fail "could not extract edition from [workspace.package] in the root Cargo.toml"

LINTS_BLOCK="$(sed -n '/^\[workspace\.lints\.rust\]/,/^\[profile\./p' "$REPO_ROOT/Cargo.toml" | sed -e '/^\[profile\./d' -e 's/^\[workspace\.lints\./[lints./')"
[[ -n "$LINTS_BLOCK" ]] || fail "could not extract the [workspace.lints] table from the root Cargo.toml"

dep_entry() {
  local name="$1" entry
  entry="$(awk -v pat="^$name = " '$0 ~ pat {print; exit}' "$REPO_ROOT/Cargo.toml")"
  [[ -n "$entry" ]] || fail "could not extract the '$name' workspace dependency entry from the root Cargo.toml"
  printf '%s\n' "$entry"
}
TOKIO_ENTRY="$(dep_entry tokio)"
FUTURES_ENTRY="$(dep_entry futures)"
TRACING_ENTRY="$(dep_entry tracing)"
WIN_SYS_ENTRY="$(dep_entry windows-sys)"

# The one cross-crate import the module carries today. The shim mirrors the
# real path (crate::application::crawler::ports) so the copied module stays
# byte-identical — no sed over the source. If the real constant moves or
# renames, this extraction fails loudly: extend the shim, do not silence it.
CONSTANT_SRC="$REPO_ROOT/crates/webfang_core/src/application/crawler/ports.rs"
CONSTANT_DEF="$(awk '/^pub const CTRL_C_EVENT_NAME/{f=1} f{print} f && /;[[:space:]]*$/{exit}' "$CONSTANT_SRC")"
[[ -n "$CONSTANT_DEF" ]] || fail "CTRL_C_EVENT_NAME not found in $CONSTANT_SRC — the module's import list changed; update the shim in this script"

# ---------------------------------------------------------------------------
# Scratch crate: src mirrors the real module paths, so every
# `crate::application::crawler::ports::*` import resolves verbatim against
# the shim. A NEW crate:: import the module gains fails to resolve here and
# fails this script loudly — that is the signal to extend the shim.
# ---------------------------------------------------------------------------
SCRATCH="$(mktemp -d)"
cleanup() { if [[ "$KEEP_SCRATCH" == "true" ]]; then echo "check_windows_platform: scratch kept at $SCRATCH"; else rm -rf "$SCRATCH"; fi; }
trap cleanup EXIT

mkdir -p "$SCRATCH/src/application/crawler" "$SCRATCH/src/infrastructure/platform"
cp "$MODULE" "$SCRATCH/src/infrastructure/platform/windows_console.rs"

cat > "$SCRATCH/Cargo.toml" <<TOML
# Throwaway crate generated by scripts/check_windows_platform.sh — type-check
# only, never linked or run. Safe to delete at any time.
[package]
name = "windows_platform_check"
version = "0.0.0"
edition = "$EDITION"
publish = false

[dependencies]
$TOKIO_ENTRY
$FUTURES_ENTRY
$TRACING_ENTRY
$WIN_SYS_ENTRY

# Mirrored verbatim from the root [workspace.lints] table at generation time:
# a lint the workspace denies is denied here too, so a bare unsafe block (the
# #1810 defect class) fails exactly as it would on Windows.
$LINTS_BLOCK
TOML

cat > "$SCRATCH/src/lib.rs" <<'RS'
//! Throwaway type-check crate — see scripts/check_windows_platform.sh.

pub mod application {
    pub mod crawler {
        pub mod ports;
    }
}

pub mod infrastructure {
    pub mod platform {
        pub mod windows_console;
    }
}
RS

cat > "$SCRATCH/src/application/crawler/ports.rs" <<RS
//! Path-mirrored shim: the checked module imports this constant from the
//! real crate; the definition below is extracted verbatim from
//! crates/webfang_core/src/application/crawler/ports.rs at generation time.
$CONSTANT_DEF
RS

echo "check_windows_platform: type-checking $(basename "$MODULE") for $TARGET"
# The cross-target check must run a consistent (cargo, rustc, std) triple
# from ONE toolchain:
#   - mbx's action cache mishandles the MSVC std resolution for a non-host
#     target (E0463 "can't find crate for core/std") — observed 2026-10-29;
#   - and even the real toolchain cargo invokes `rustc` from PATH, which on
#     this workstation can be a shim resolving to a DIFFERENT toolchain than
#     the one `rustup which cargo` picked (1.91.0 lacks the MSVC std;
#     1.91.1 — the rust-toolchain.toml pin — has it).
# Pinning RUSTC to the same bin dir as cargo removes both failure modes.
# `rustup which` honors rust-toolchain.toml and RUSTUP_TOOLCHAIN.
TOOLCHAIN_BIN="$(dirname "$(rustup which cargo)")"
if (cd "$SCRATCH" && CARGO_TARGET_DIR="$HOME/.cache/cargo-target/windows-typecheck-scratch" \
    RUSTC="$TOOLCHAIN_BIN/rustc" \
    "$TOOLCHAIN_BIN/cargo" check --target "$TARGET" --all-targets); then
  echo "check_windows_platform: OK — cfg(windows) arms type-check for $TARGET"
else
  status=$?
  echo "::error::cfg(windows) type-check FAILED for $TARGET — the compiler errors above reference the module copy at $SCRATCH/src/infrastructure/platform/windows_console.rs" >&2
  exit "$status"
fi
