# Windows AppContainer enforcement probe (S8/T8 follow-up).
#
# This script intentionally does NOT launch Chrome. Chromium initializes Crashpad
# before command-line handling, and Crashpad creates a global named pipe that an
# AppContainer token cannot create (CreateNamedPipe: Access is denied, 0x5).
# Chromium checks the POSIX-only Crashpad testing switch on POSIX, so it cannot
# fix a Win32 Chrome under AppContainer. The honest Windows proof is to
# assert the sandbox itself from inside the sandboxed process:
#   1. the token carries an AppContainer package SID (S-1-15-2-*),
#   2. a granted path is readable,
#   3. an ungranted path is NOT readable.
# Chrome under Seatbelt stays verified on macOS; Windows claims enforcement only.
#
# `arapuca run` defaults to allow_exec=true; ARAPUCA_ALLOW_EXEC is set here as
# well so the proof states the condition explicitly. No network-namespace flag
# is passed, so the native Windows backend keeps its own configuration.
#
# Override (used by the contract harness and non-standard installs):
#   WEBFANG_ARAPUCA_BINARY            path to the built arapuca.exe
#   WEBFANG_SANDBOX_SMOKE_PROFILE     writable profile directory
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
# vendor/arapuca is excluded from the workspace: without --target-dir its
# binary lands in vendor/arapuca/target. Accept both layouts; CI pins the root.
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

$createdProfile = $false
$profile = $env:WEBFANG_SANDBOX_SMOKE_PROFILE
if (-not $profile) {
  $profile = Join-Path $env:TEMP ("webfang-sandbox-probe-" + [guid]::NewGuid().ToString('N'))
  $createdProfile = $true
}
New-Item -ItemType Directory -Force -Path $profile | Out-Null

$allowedFile = Join-Path $profile 'allowed.txt'
Set-Content -LiteralPath $allowedFile -Value 'WEBFANG_ALLOW_READ_OK'

# The denied sentinel lives outside every -v mount. It is created by the host,
# never granted to the AppContainer, and removed in the finally block below.
$deniedDir = Join-Path ([IO.Path]::GetTempPath()) ("webfang-sandbox-denied-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $deniedDir | Out-Null
$deniedFile = Join-Path $deniedDir 'denied.txt'
Set-Content -LiteralPath $deniedFile -Value 'WEBFANG_MUST_NOT_BE_READABLE'

$system32 = Join-Path $env:SystemRoot 'System32'
$probeBatch = Join-Path $profile 'probe.cmd'
$batch = @(
  '@echo off'
  'echo WEBFANG_SANDBOX_OK'
  '"%WEBFANG_PROBE_WHOAMI%" /groups | "%WEBFANG_PROBE_FINDSTR%" /C:"S-1-15-2-" >nul'
  'if errorlevel 1 exit /b 11'
  'echo WEBFANG_APP_CONTAINER_OK'
  'type "%WEBFANG_PROBE_ALLOWED%" >nul'
  'if errorlevel 1 exit /b 12'
  'echo WEBFANG_ALLOW_READ_OK'
  'type "%WEBFANG_PROBE_DENIED%" >nul 2>nul'
  'if not errorlevel 1 exit /b 13'
  'echo WEBFANG_DENY_READ_OK'
) -join "`r`n"
Set-Content -LiteralPath $probeBatch -Value $batch -Encoding Ascii

$env:ARAPUCA_ALLOW_EXEC = '1'
$taskId = 'webfang-sandbox-probe-windows'
$smokeArgs = @(
  'run', '--task-id', $taskId, '--timeout', '90',
  '--env', "WEBFANG_PROBE_ALLOWED=$allowedFile",
  '--env', "WEBFANG_PROBE_DENIED=$deniedFile",
  '--env', "WEBFANG_PROBE_WHOAMI=$(Join-Path $system32 'whoami.exe')",
  '--env', "WEBFANG_PROBE_FINDSTR=$(Join-Path $system32 'findstr.exe')",
  '-v', "${profile}:rw",
  '--', (Join-Path $system32 'cmd.exe'), '/d', '/c', $probeBatch
)

# Native stderr redirected into the pipeline becomes ErrorRecord objects; with
# Stop they would abort before we can report the sandbox's own diagnostic.
$previous = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
  $output = (& $arapuca @smokeArgs 2>&1 | Out-String)
  $exitCode = $LASTEXITCODE
} finally {
  $ErrorActionPreference = $previous
  if ($createdProfile) { Remove-Item -LiteralPath $profile -Recurse -Force -ErrorAction SilentlyContinue }
  Remove-Item -LiteralPath $deniedDir -Recurse -Force -ErrorAction SilentlyContinue
}

if ($exitCode -ne 0) { throw "arapuca run exited $exitCode for the AppContainer probe`n$output" }
foreach ($marker in @(
  'WEBFANG_SANDBOX_OK',
  'WEBFANG_APP_CONTAINER_OK',
  'WEBFANG_ALLOW_READ_OK',
  'WEBFANG_DENY_READ_OK'
)) {
  if ($output -notlike "*$marker*") { throw "probe marker '$marker' missing from output`n$output" }
}

Write-Host "sandbox probe: OK (AppContainer identity + allowlist enforced, task=$taskId)"
