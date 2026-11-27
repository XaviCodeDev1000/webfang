#!/usr/bin/env bash
# Contract tests for the cross-platform sandbox CI proof (S8/T8).
#
# The two real launches can only happen on macOS and Windows hosted runners.
# This harness proves the LOCAL contract of those jobs without a runner:
# the POSIX smoke fails closed with no browser, succeeds through a fake
# arapuca+chrome pair, and the workflow wires every required step.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SH_SMOKE="$ROOT/scripts/sandbox_smoke_chrome.sh"
PS_SMOKE="$ROOT/scripts/sandbox_probe_appcontainer.ps1"
WORKFLOW="$ROOT/.github/workflows/cross-platform-sandbox.yml"

fail() { echo "::error::$1" >&2; exit 1; }
ok() { echo "ok: $1"; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# A fake arapuca that consumes run flags up to `--` and execs the real command.
cat >"$WORK/arapuca" <<'SH'
#!/bin/sh
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
[ "$1" = "--" ] || exit 125
shift
exec "$@"
SH
cat >"$WORK/chrome" <<'SH'
#!/bin/sh
printf '%s\n' 'Google Chrome 1.2.3'
SH
chmod +x "$WORK/arapuca" "$WORK/chrome"

# T1: the POSIX smoke launches through the injected adapter and sees the marker.
if [ ! -f "$SH_SMOKE" ]; then
  fail "scripts/sandbox_smoke_chrome.sh does not exist yet (RED)"
fi
out="$(WEBFANG_ARAPUCA_BINARY="$WORK/arapuca" \
       WEBFANG_CHROME_BINARY="$WORK/chrome" \
       WEBFANG_SANDBOX_SMOKE_PROFILE="$WORK/profile" \
       WEBFANG_SANDBOX_SMOKE_READ_PATH="$WORK" \
       bash "$SH_SMOKE" 2>&1)" || fail "smoke failed with a valid fake browser: $out"
case "$out" in
  *Google\ Chrome*) ok "posix smoke sees the browser marker" ;;
  *) fail "marker missing from smoke output: $out" ;;
esac

# T2: fail closed when the browser cannot be resolved.
if out="$(WEBFANG_ARAPUCA_BINARY="$WORK/arapuca" \
          WEBFANG_CHROME_BINARY="$WORK/missing-browser" \
          WEBFANG_SANDBOX_SMOKE_PROFILE="$WORK/profile2" \
          WEBFANG_SANDBOX_SMOKE_READ_PATH="$WORK" \
          bash "$SH_SMOKE" 2>&1)"; then
  fail "smoke must fail when the browser is missing"
fi
case "$out" in
  *"::error::"*) ok "posix smoke fails closed on a missing browser" ;;
  *) fail "missing browser produced no typed error: $out" ;;
esac

# T3: the workflow wires both runners and every required check.
if [ ! -f "$WORKFLOW" ]; then
  fail ".github/workflows/cross-platform-sandbox.yml does not exist yet (RED)"
fi
for needle in \
  'runs-on: macos-latest' \
  'runs-on: windows-latest' \
  'cargo check' \
  'scripts/check_windows_platform.sh' \
  'scripts/sandbox_smoke_chrome.sh' \
  'scripts/sandbox_probe_appcontainer.ps1' \
  'scripts/tests/test_cross_platform_sandbox_ci.sh'
do
  grep -Fq -- "$needle" "$WORKFLOW" || fail "workflow misses: $needle"
done
ok "workflow wires macOS, Windows and the contract job"

# T4: the Windows smoke states allow_exec and never disables the native sandbox
# through --deny-network (which would turn the network namespace on).
if [ ! -f "$PS_SMOKE" ]; then
  fail "scripts/sandbox_smoke_chrome.ps1 does not exist yet (RED)"
fi
grep -Fq 'ARAPUCA_ALLOW_EXEC' "$PS_SMOKE" || fail "PowerShell smoke misses allow_exec intent"
grep -Fq -- 'run' "$PS_SMOKE" || fail "PowerShell smoke does not use arapuca run"
if grep -Fq -- '--deny-network' "$PS_SMOKE"; then
  fail "PowerShell smoke enables the network namespace via --deny-network"
fi
ok "windows smoke keeps run defaults: allow_exec=true, netns disabled"

# T5: vendor/arapuca is excluded from the workspace, so an unpinned build puts
# the binary in vendor/arapuca/target. The workflow must force the root target.
grep -Fq -- '--target-dir target' "$WORKFLOW" || fail "arapuca build must pin --target-dir target"
ok "arapuca build pins the workspace target dir"

# T6: the macOS app-bundle prefix already ends in `.app`; appending it again
# produced /Applications/Google Chrome.app.app on the hosted runner.
grep -Fq -- "READ_PATH=\"\${CHROME%%/Contents/MacOS/*}\"" "$SH_SMOKE" || fail "app bundle derivation appends .app twice"
ok "macOS app bundle derivation is single-suffix"

# T7: macOS keeps the Chrome version probe. Windows MUST NOT claim a Chrome
# launch: Chromium initializes Crashpad before flags, AppContainer denies its
# named pipe, and --disable-crashpad-for-testing is POSIX-only.
grep -Fq -- '--version' "$SH_SMOKE" || fail "posix smoke must use the Chrome version probe"
if grep -Fq -- '--version' "$PS_SMOKE"; then
  fail "windows probe must not launch Chrome under AppContainer"
fi
if grep -Fq -- '--disable-crashpad-for-testing' "$PS_SMOKE"; then
  fail "windows probe must not carry the POSIX-only Crashpad switch"
fi
ok "macOS probes Chrome; Windows probes enforcement without Chrome"

# T8: the Windows probe asserts three observable effects from inside the
# AppContainer: the package SID is present, a granted path is readable, and an
# ungranted path is not readable.
for needle in \
  'WEBFANG_SANDBOX_OK' \
  'WEBFANG_APP_CONTAINER_OK' \
  'WEBFANG_ALLOW_READ_OK' \
  'WEBFANG_DENY_READ_OK' \
  'S-1-15-2-' \
  'whoami.exe' \
  'findstr.exe' \
  'cmd.exe' \
  'AppContainer SID missing' \
  'granted path unreadable' \
  'ungranted path readable'
do
  grep -Fq -- "$needle" "$PS_SMOKE" || fail "windows probe misses: $needle"
done
ok "windows probe checks AppContainer identity and the file allowlist"

# T9: the workflow must describe the Windows job as an enforcement probe, not
# as a browser launch, so the green check is not read as a Chrome claim.
grep -Fq 'AppContainer enforcement probe' "$WORKFLOW" || fail "workflow still claims a Windows Chrome launch"
ok "workflow names the Windows job as an enforcement probe"

