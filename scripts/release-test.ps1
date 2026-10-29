# The release gate: everything the Test lab checks before a release, in one report folder.
#   powershell -File scripts\release-test.ps1 -Firmware C:\Users\fuchs\splitkeeb\firmware-<sha>\firmware
# 1. UF2 preflight of both halves (scripts/flash, --dry-run: reads files only, never flashes).
# 2. The ZMK simulator's CI run for this commit (zmk-sim.yml, read with gh; #627's activity-relay).
# 3. How each half shows up on USB (foxglove-lab usb).
# 4. The release suite on the keyboard (foxglove-lab run --suite release).
# 5. The resilience suite (tests/hil/resilience.toml, #618): it needs Foxglove quit, so a running
#    Foxglove is stopped by its exact name and started again by its path after, only with
#    FOXGLOVE_APP_OK=1 (On-call); without it the stage is BLOCKED.
# 6. With -App <foxglove.exe> and FOXGLOVE_APP_OK=1: the app-level lab cases of the stability pass
#    (scripts/lab/overlay_tray.py: #612/#620, #601/#622, #618 agent controls; api_tools.py: #617/
#    #623). They launch that build on a temp FOXGLOVE_CONFIG_DIR. Without them the stage is BLOCKED.
# The 30-minute soak (scripts/lab/soak.py) is separate: it is the sitting's exit check (#618).
# It never flashes. Flash first with scripts/flash/foxglove_flash.py (the user types FLASH),
# then run this against the same firmware folder.
# Agents: add -NonInteractive (manual cases are reported BLOCKED) and -AllowApp if Foxglove runs.
# Exit 0 PASS, 1 FAIL, 2 BLOCKED (nothing failed, something couldn't run).
# The lab's key presses and tray clicks are input to Windows; foxglove-lab and labkit record them
# (%LOCALAPPDATA%\Foxglove\lab-injections.log), so the "user is working" gate before a soak
# (python scripts/lab/labkit.py idle) leaves them out (#618).
param(
    [Parameter(Mandatory)][string]$Firmware,
    [string]$Suite = 'release',
    [string]$App,
    [switch]$AllowApp,
    [switch]$NonInteractive
)
$ErrorActionPreference = 'Stop'
# The lab scripts print UTF-8 (arrows in log lines): decode it as such through Tee-Object (#618).
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$env:PYTHONIOENCODING = 'utf-8'

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $sha = (git rev-parse --short HEAD).Trim()
    $full = (git rev-parse HEAD).Trim()
    $out = Join-Path 'lab-reports' ("{0:yyyyMMddTHHmmss}Z-{1}-release" -f (Get-Date).ToUniversalTime(), $sha)
    New-Item -ItemType Directory -Force $out | Out-Null
    $failed = @()
    $blocked = @()
    $appOk = $env:FOXGLOVE_APP_OK -eq '1'

    Write-Host '== UF2 preflight' -ForegroundColor Cyan
    python scripts/flash/foxglove_flash.py --firmware $Firmware --dry-run --json |
        Out-File -Encoding utf8 (Join-Path $out 'preflight.json')
    if ($LASTEXITCODE) { $failed += 'preflight' }
    Get-Content (Join-Path $out 'preflight.json')

    Write-Host '== ZMK simulator CI (zmk-sim.yml) for this commit' -ForegroundColor Cyan
    if (Get-Command gh -ErrorAction SilentlyContinue) {
        # gh's stderr (no network, not logged in) mustn't stop the gate: that's a BLOCKED stage.
        $json = & { $ErrorActionPreference = 'Continue'; gh run list --workflow zmk-sim.yml --commit $full --limit 5 --json conclusion,databaseId,status 2>$null }
        $runs = if ($json) { ($json -join "`n") | ConvertFrom-Json } else { @() }
        $done = @($runs | Where-Object { $_.status -eq 'completed' })
        if (-not $done) { $blocked += 'zmk-sim CI (no finished run for this commit)' }
        elseif ($done[0].conclusion -ne 'success') { $failed += "zmk-sim CI (run $($done[0].databaseId): $($done[0].conclusion))" }
        else { Write-Host "zmk-sim CI run $($done[0].databaseId): success" }
    } else { $blocked += 'zmk-sim CI (gh not installed)' }

    & "$PSScriptRoot\build-slot.ps1" cargo build -q --release -p foxglove-lab
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

    Write-Host '== Suite resilience (Foxglove quit)' -ForegroundColor Cyan
    # Exactly the app's process name: other sessions' test binaries must never be stopped.
    $running = @(Get-Process -Name foxglove -ErrorAction SilentlyContinue | ForEach-Object { $_.Path })
    if ($running -and -not $appOk) {
        $blocked += 'suite resilience (Foxglove runs; FOXGLOVE_APP_OK=1 lets this stop and restart it)'
    } else {
        if ($running) {
            Get-Process -Name foxglove -ErrorAction SilentlyContinue | Stop-Process -Confirm:$false
            Start-Sleep -Seconds 2 # its HID handles close
        }
        try {
            $rout = Join-Path $out 'resilience'
            $run = @('run', '--suite', 'tests/hil/resilience.toml', '--out', $rout)
            if ($NonInteractive) { $run += '--non-interactive' }
            & $lab @run
            # A blocked case (a GA build without test hooks, the manual replug) isn't a failure:
            # the summary names it, so On-call sees what didn't run.
            $report = Get-Content (Join-Path $rout 'report.json') -Raw | ConvertFrom-Json
            $bad = @($report.cases | Where-Object { $_.status -eq 'FAIL' } | ForEach-Object { $_.name })
            $skip = @($report.cases | Where-Object { $_.status -eq 'BLOCKED' -and -not $_.manual } | ForEach-Object { $_.name })
            if ($bad) { $failed += "suite resilience ($($bad -join ', '))" }
            if ($skip) { $blocked += "suite resilience ($($skip -join ', '))" }
        } finally {
            foreach ($p in ($running | Select-Object -Unique)) { Start-Process -FilePath $p }
        }
    }

    Write-Host '== App lab cases (overlay, tray, re-apply, API)' -ForegroundColor Cyan
    if (-not $App) { $blocked += 'app lab cases (no -App <foxglove.exe>)' }
    elseif (-not $appOk) { $blocked += 'app lab cases (they launch the app: FOXGLOVE_APP_OK=1, On-call)' }
    else {
        $cases = @(
            @{ Name = 'overlay_tray'; Args = @('scripts/lab/overlay_tray.py', '--app', $App, '--lab', $lab) },
            @{ Name = 'api_tools'; Args = @('scripts/lab/api_tools.py', '--app', $App) }
        )
        foreach ($c in $cases) {
            $name = $c.Name
            python @($c.Args) | Tee-Object -FilePath (Join-Path $out "$name.txt")
            if ($LASTEXITCODE -eq 2) { $blocked += $name } elseif ($LASTEXITCODE) { $failed += $name }
        }
    }

    $summary = if ($failed) { "RELEASE GATE: FAIL ($($failed -join ', '))" }
               elseif ($blocked) { "RELEASE GATE: BLOCKED ($($blocked -join '; '))" }
               else { 'RELEASE GATE: PASS' }
    @($summary, "firmware: $Firmware", "repo: $sha", "report: $out\report.html") |
        Out-File -Encoding utf8 (Join-Path $out 'summary.txt')
    Write-Host $summary -ForegroundColor $(if ($failed) { 'Red' } elseif ($blocked) { 'Yellow' } else { 'Green' })
    Write-Host "Report: $out\report.html"
    if ($failed) { exit 1 }
    if ($blocked) { exit 2 }
} finally {
    Pop-Location
}
