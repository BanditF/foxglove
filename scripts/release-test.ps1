# The release gate: everything the Test lab checks before a release, in one report folder.
#   powershell -File scripts\release-test.ps1 -Firmware C:\Users\fuchs\splitkeeb\firmware-<sha>\firmware
# 1. UF2 preflight of both halves (scripts/flash, --dry-run: reads files only, never flashes).
# 2. How each half shows up on USB (foxglove-lab usb).
# 3. The release suite on the keyboard (foxglove-lab run --suite release).
# It never flashes. Flash first with scripts/flash/foxglove_flash.py (the user types FLASH),
# then run this against the same firmware folder.
# Agents: add -NonInteractive (manual cases are reported BLOCKED) and -AllowApp if Foxglove runs.
param(
    [Parameter(Mandatory)][string]$Firmware,
    [string]$Suite = 'release',
    [switch]$AllowApp,
    [switch]$NonInteractive
)
$ErrorActionPreference = 'Stop'

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $sha = (git rev-parse --short HEAD).Trim()
    $out = Join-Path 'lab-reports' ("{0:yyyyMMddTHHmmss}Z-{1}-release" -f (Get-Date).ToUniversalTime(), $sha)
    New-Item -ItemType Directory -Force $out | Out-Null
    $failed = @()

    Write-Host '== UF2 preflight' -ForegroundColor Cyan
    python scripts/flash/foxglove_flash.py --firmware $Firmware --dry-run --json |
        Out-File -Encoding utf8 (Join-Path $out 'preflight.json')
    if ($LASTEXITCODE) { $failed += 'preflight' }
    Get-Content (Join-Path $out 'preflight.json')

    cargo build -q --release -p foxglove-lab
    if ($LASTEXITCODE) { throw 'foxglove-lab did not build' }
    $lab = 'target/release/foxglove-lab.exe'
    if ($env:CARGO_TARGET_DIR) { $lab = Join-Path $env:CARGO_TARGET_DIR 'release/foxglove-lab.exe' }

    Write-Host '== USB halves' -ForegroundColor Cyan
    & $lab usb | Tee-Object -FilePath (Join-Path $out 'usb.txt')
    if ($LASTEXITCODE) { $failed += 'usb' }

    Write-Host "== Suite $Suite" -ForegroundColor Cyan
    $run = @('run', '--suite', $Suite, '--out', $out)
    if ($AllowApp) { $run += '--allow-app' }
    if ($NonInteractive) { $run += '--non-interactive' }
    & $lab @run
    if ($LASTEXITCODE) { $failed += "suite $Suite" }

    $summary = if ($failed) { "RELEASE GATE: FAIL ($($failed -join ', '))" } else { 'RELEASE GATE: PASS' }
    @($summary, "firmware: $Firmware", "repo: $sha", "report: $out\report.html") |
        Out-File -Encoding utf8 (Join-Path $out 'summary.txt')
    Write-Host $summary -ForegroundColor $(if ($failed) { 'Red' } else { 'Green' })
    Write-Host "Report: $out\report.html"
    if ($failed) { exit 1 }
} finally {
    Pop-Location
}
