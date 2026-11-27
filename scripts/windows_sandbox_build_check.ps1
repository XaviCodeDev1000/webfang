# Windows sandbox build checks (S8/T8 follow-up).
#
# This step intentionally does NOT claim AppContainer runtime enforcement.
# Verified on the hosted runner: a Win32 console process cannot start under the
# AppContainer profile created by arapuca (CreateProcess fails with Access is
# denied; even a cmd.exe copied into the granted profile fails). Chromium also
# aborts in Crashpad because AppContainer denies its global named pipe. GUI
# Chrome under Seatbelt remains verified on macOS. The Windows job therefore
# proves only that the native Windows backend compiles and that the arapuca
# binary runs on the host; the runtime enforcement claim is explicitly absent.
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$arapuca = $env:WEBFANG_ARAPUCA_BINARY
if (-not $arapuca) {
  foreach ($candidate in @(
    (Join-Path $root 'target/debug/arapuca.exe'),
    (Join-Path $root 'vendor/arapuca/target/debug/arapuca.exe')
  )) {
    if (Test-Path -LiteralPath $candidate -PathType Leaf) { $arapuca = $candidate; break }
  }
}
if (-not $arapuca) { $arapuca = Join-Path $root 'target/debug/arapuca.exe' }
if (-not (Test-Path -LiteralPath $arapuca -PathType Leaf)) {
  throw "arapuca binary not found: $arapuca (build it with: cargo build --manifest-path vendor/arapuca/Cargo.toml --bin arapuca --target-dir target)"
}

$version = (& $arapuca --version 2>&1 | Out-String).Trim()
if (-not $version) { throw 'arapuca --version produced no output' }

Write-Host "arapuca build OK: $version"
Write-Host 'Windows AppContainer runtime enforcement NOT verified: build and host-level checks only.'
