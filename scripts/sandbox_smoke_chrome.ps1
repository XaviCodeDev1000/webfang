# Launch Chrome through arapuca's native Windows sandbox (AppContainer/Job
# Objects) from the launcher/service context required by S8/T8.
#
# `arapuca run` always sets allow_exec=true; ARAPUCA_ALLOW_EXEC is set here as
# well so the proof states the condition explicitly. The network-denial flag
# is not passed, so use_netns stays false and the native sandbox owns the process.
#
# Overrides (used by the contract harness and non-standard installs):
#   WEBFANG_ARAPUCA_BINARY          path to the built arapuca.exe
#   WEBFANG_CHROME_BINARY           path to chrome.exe
#   WEBFANG_SANDBOX_SMOKE_PROFILE   writable profile directory
#   WEBFANG_SANDBOX_SMOKE_READ_PATH read-only mount containing chrome.exe
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

$chrome = $env:WEBFANG_CHROME_BINARY
if (-not $chrome) {
  $candidates = @(
    (Join-Path ${env:ProgramFiles} 'Google/Chrome/Application/chrome.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Google/Chrome/Application/chrome.exe'),
    (Join-Path $env:LOCALAPPDATA 'Google/Chrome/Application/chrome.exe')
  )
  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) { $chrome = $candidate; break }
  }
}
if (-not $chrome -or -not (Test-Path -LiteralPath $chrome -PathType Leaf)) {
  throw 'no chrome.exe found; set WEBFANG_CHROME_BINARY'
}

$readPath = if ($env:WEBFANG_SANDBOX_SMOKE_READ_PATH) { $env:WEBFANG_SANDBOX_SMOKE_READ_PATH } else { Split-Path -Parent $chrome }
if (-not (Test-Path -LiteralPath $readPath -PathType Container)) {
  throw "browser read path is not a directory: $readPath"
}

$createdProfile = $false
$profile = $env:WEBFANG_SANDBOX_SMOKE_PROFILE
if (-not $profile) {
  $profile = Join-Path $env:TEMP ("webfang-sandbox-smoke-" + [guid]::NewGuid().ToString('N'))
  $createdProfile = $true
}
New-Item -ItemType Directory -Force -Path $profile | Out-Null

$env:ARAPUCA_ALLOW_EXEC = '1'
$marker = 'WEBFANG_SANDBOX_SMOKE_OK'
$taskId = 'webfang-sandbox-smoke-windows'
$smokeArgs = @(
  'run', '--task-id', $taskId, '--timeout', '90',
  '-v', "${readPath}:ro", '-v', "${profile}:rw",
  '--', $chrome,
  '--headless', '--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
  "--user-data-dir=$profile", '--dump-dom', "data:text/html,<h1>$marker</h1>"
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
}

if ($exitCode -ne 0) { throw "arapuca run exited $exitCode for $chrome`n$output" }
if ($output -notlike "*$marker*") { throw "browser marker '$marker' missing from sandboxed output`n$output" }

Write-Host "sandbox smoke: OK (Windows AppContainer, task=$taskId)"
