# The release gate: everything the Test lab checks before a release, in one report folder.
#   powershell -File scripts\release-test.ps1 -Firmware C:\Users\fuchs\splitkeeb\firmware-<sha>\firmware
# 1. UF2 preflight of both halves (scripts/flash, --dry-run: reads files only, never flashes).
# 2. The ZMK simulator's CI run for this commit (zmk-sim.yml, read with gh; #627's activity-relay).
# 3. How each half shows up on USB (foxglove-lab usb).
# 4. The resilience suite (tests/hil/resilience.toml, #618): it needs Foxglove quit, so a running
#    Foxglove is stopped by its exact name and started again by its path after, only with
#    FOXGLOVE_APP_OK=1 (On-call); without it the stage is BLOCKED. It runs before the app stages,
#    once the app's processes have exited (a bounded wait on their exit, no timer), with
#    right-follows-left-activity first: its 1 s wake limit failed after the app stages and the
#    suite's own pushes and floods, and passed alone (986 ms) (#618 follow-up).
# 5. The release suite on the keyboard (foxglove-lab run --suite release), with the app started
#    again: its idle CPU case runs first and measures once the app has run 2 minutes (settle_s).
# 6. With -App <foxglove.exe> and FOXGLOVE_APP_OK=1: the app-level lab cases of the stability pass
#    (scripts/lab/overlay_tray.py: #612/#620, #601/#622, #618 agent controls; api_tools.py: #617/
#    #623). They launch that build on a temp FOXGLOVE_CONFIG_DIR. Without them the stage is BLOCKED.
# foxglove-lab saves the underglow (RGB_STATE) when each suite starts and puts it back when it ends,
# also after a failure or Ctrl-C (crates/foxglove-lab/src/lights.rs; two log lines: "lights saved",
# "lights restored"). On-call no longer restores the user's look by hand after a gate run (#618
# follow-up; resilience's right-relink case left 19/100/20 behind).
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

$root = Split-Path -Parent $PSScriptRoot
# Every tool path the gate hands on is absolute (relative ones are taken from the repo root): a
# relative --lab made overlay_tray.py crash with FileNotFoundError mid-run, so its re-apply and
# relink cases never ran (#618 follow-up).
function Resolve-RepoPath([string]$Path, [string]$What, [switch]$Folder) {
    $full = if ([IO.Path]::IsPathRooted($Path)) { $Path } else { Join-Path $root $Path }
    $full = [IO.Path]::GetFullPath($full)
    $type = if ($Folder) { 'Container' } else { 'Leaf' }
    if (-not (Test-Path -LiteralPath $full -PathType $type)) {
        throw "$What ${Path}: not found ($full; relative paths are from the repo root $root)"
    }
    $full
}
$Firmware = Resolve-RepoPath $Firmware '-Firmware' -Folder
if ($App) { $App = Resolve-RepoPath $App '-App' }

Push-Location $root
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
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    $lab = Resolve-RepoPath (Join-Path $target 'release\foxglove-lab.exe') 'foxglove-lab'

    Write-Host '== USB halves' -ForegroundColor Cyan
    & $lab usb | Tee-Object -FilePath (Join-Path $out 'usb.txt')
    if ($LASTEXITCODE) { $failed += 'usb' }

    Write-Host '== Suite resilience (Foxglove quit)' -ForegroundColor Cyan
    # Exactly the app's process name: other sessions' test binaries must never be stopped.
    $running = @(Get-Process -Name foxglove -ErrorAction SilentlyContinue | ForEach-Object { $_.Path })
    if ($running -and -not $appOk) {
        $blocked += 'suite resilience (Foxglove runs; FOXGLOVE_APP_OK=1 lets this stop and restart it)'
    } else {
        $restarted = $false
        try {
            if ($running) {
                $procs = @(Get-Process -Name foxglove -ErrorAction SilentlyContinue)
                $procs | Stop-Process -Confirm:$false
                $restarted = $true
                # Quiet first: wait on the processes' exit (their HID handles close with them),
                # bounded; the suite doesn't start while one is still there.
                $procs | Wait-Process -Timeout 20 -ErrorAction SilentlyContinue
            }
            if (Get-Process -Name foxglove -ErrorAction SilentlyContinue) {
                $blocked += 'suite resilience (Foxglove did not exit within 20 s)'
            } else {
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
            }
        } finally {
            foreach ($p in ($running | Select-Object -Unique)) { Start-Process -FilePath $p }
        }
        if ($restarted) {
            # The release suite sees the app as running only once it is up (its API, its panel),
            # and its idle CPU case must not measure the startup: one timed wait until the app
            # has run 2 minutes (the settle the solo idle run had), counted from its start.
            $young = Get-Process -Name foxglove -ErrorAction SilentlyContinue |
                Sort-Object StartTime -Descending | Select-Object -First 1
            if ($young) {
                $left = 120 - ((Get-Date) - $young.StartTime).TotalSeconds
                if ($left -gt 0) {
                    Write-Host ("Foxglove started again; settling {0:N0} s before the release suite" -f $left)
                    Start-Sleep -Milliseconds ([int]($left * 1000))
                }
            }
        }
    }

    Write-Host "== Suite $Suite" -ForegroundColor Cyan
    $run = @('run', '--suite', $Suite, '--out', $out)
    if ($AllowApp) { $run += '--allow-app' }
    if ($NonInteractive) { $run += '--non-interactive' }
    & $lab @run
    # Exit 2: nothing failed, a case couldn't run (a manual one under -NonInteractive): BLOCKED.
    if ($LASTEXITCODE -eq 2) { $blocked += "suite $Suite" } elseif ($LASTEXITCODE) { $failed += "suite $Suite" }

    Write-Host '== App lab cases (overlay, tray, re-apply, API)' -ForegroundColor Cyan
    if (-not $App) { $blocked += 'app lab cases (no -App <foxglove.exe>)' }
    elseif (-not $appOk) { $blocked += 'app lab cases (they launch the app: FOXGLOVE_APP_OK=1, On-call)' }
    else {
        $cases = @(
            @{ Name = 'overlay_tray'; Args = @('scripts/lab/overlay_tray.py', '--app', $App, '--lab', $lab) },
            @{ Name = 'api_tools'; Args = @('scripts/lab/api_tools.py', '--app', $App) },
            @{ Name = 'lights_page'; Args = @('scripts/lab/lights_page.py', '--app', $App, '--lab', $lab) }
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
