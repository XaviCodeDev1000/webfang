#!/usr/bin/env bash
# Launch Chrome through arapuca's native sandbox on macOS (Seatbelt).
#
# This is the real platform proof required by S8/T8: it is meant to run on a
# macOS hosted runner, where `sandbox-exec` exists. On Linux it is only used by
# the contract harness with injected fake adapters.
#
# Overrides (used by the harness and by runners with non-standard installs):
#   WEBFANG_ARAPUCA_BINARY          path to the built arapuca binary
#   WEBFANG_CHROME_BINARY           path to the real Chrome/Chromium binary
#   WEBFANG_SANDBOX_SMOKE_PROFILE   writable profile directory
#   WEBFANG_SANDBOX_SMOKE_READ_PATH read-only mount containing the browser
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MARKER="Google Chrome"
TASK_ID="webfang-sandbox-smoke-$$"

die() { echo "::error::$1" >&2; exit 1; }

# vendor/arapuca is excluded from the workspace: without --target-dir its
# binary lands in vendor/arapuca/target. Accept both layouts so a local build
# and the CI build (which pins --target-dir target) both work.
ARAPUCA="${WEBFANG_ARAPUCA_BINARY:-}"
if [[ -z "$ARAPUCA" ]]; then
  for candidate in "$ROOT/target/debug/arapuca" "$ROOT/vendor/arapuca/target/debug/arapuca"; do
    if [[ -x "$candidate" ]]; then ARAPUCA="$candidate"; break; fi
  done
fi
ARAPUCA="${ARAPUCA:-$ROOT/target/debug/arapuca}"
[[ -x "$ARAPUCA" ]] || die "arapuca binary not executable: $ARAPUCA (build it with: cargo build --manifest-path vendor/arapuca/Cargo.toml --bin arapuca --target-dir target)"

CHROME="${WEBFANG_CHROME_BINARY:-}"
if [[ -z "$CHROME" ]]; then
  for candidate in \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "/Applications/Chromium.app/Contents/MacOS/Chromium"
  do
    if [[ -x "$candidate" ]]; then CHROME="$candidate"; break; fi
  done
fi
if [[ -z "$CHROME" ]]; then
  for name in google-chrome chromium chrome; do
    if command -v "$name" >/dev/null 2>&1; then CHROME="$(command -v "$name")"; break; fi
  done
fi
[[ -n "$CHROME" && -x "$CHROME" ]] || die "no executable Chrome/Chromium found; set WEBFANG_CHROME_BINARY"

# The app bundle (not just MacOS/) must be visible: Chrome loads frameworks
# from sibling directories inside the bundle. A harness fixture uses its own
# directory via WEBFANG_SANDBOX_SMOKE_READ_PATH.
if [[ -n "${WEBFANG_SANDBOX_SMOKE_READ_PATH:-}" ]]; then
  READ_PATH="$WEBFANG_SANDBOX_SMOKE_READ_PATH"
elif [[ "$CHROME" == *.app/Contents/MacOS/* ]]; then
  READ_PATH="${CHROME%%/Contents/MacOS/*}"
else
  READ_PATH="$(dirname "$CHROME")"
fi
[[ -d "$READ_PATH" ]] || die "browser read path is not a directory: $READ_PATH"

CREATED_PROFILE=false
PROFILE="${WEBFANG_SANDBOX_SMOKE_PROFILE:-}"
if [[ -z "$PROFILE" ]]; then
  PROFILE="$(mktemp -d)/profile"
  CREATED_PROFILE=true
fi
cleanup() { if [[ "$CREATED_PROFILE" == "true" ]]; then rm -rf "$(dirname "$PROFILE")"; fi; }
trap cleanup EXIT
mkdir -p "$PROFILE"

# `arapuca run` fixes allow_exec=true and, without --deny-network, leaves the
# network namespace disabled. Seatbelt is the macOS native backend.
# `--version` is a real Chrome launch through the native sandbox. A full
# headless --dump-dom is not portable here: Chrome's named-pipe singleton is
# denied under the Windows AppContainer profile.
if ! output="$("$ARAPUCA" run --task-id "$TASK_ID" --timeout 90 \
  -v "$READ_PATH:ro" -v "$PROFILE:rw" \
  -- "$CHROME" "--user-data-dir=$PROFILE" --version 2>&1)"
then
  die "arapuca run failed for $CHROME: $output"
fi

case "$output" in
  *"$MARKER"*) echo "sandbox smoke: OK (macOS Seatbelt, task=$TASK_ID, marker=$MARKER)" ;;
  *) die "browser marker '$MARKER' missing from sandboxed output: $output" ;;
esac
