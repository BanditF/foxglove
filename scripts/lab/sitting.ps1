# The 0.2 sitting, hands-free (#618; docs/test-lab/sitting-0.2-h-i-r.md): every former user step of
# batches H, I, R and P as a lab case, in the doc's order, one report folder. On-call runs it with
# the PC unlocked and the user away (the H and I cases take the focus and make real input; P keeps
# the keyboard untouched about 13 minutes). Nothing here flashes; the user's Foxglove is stopped by
# its exact name for the stages that need it quit, and started again by its path.
#
#   $env:FOXGLOVE_APP_OK = '1'
#   powershell -ExecutionPolicy Bypass -File scripts\lab\sitting.ps1 [-App <foxglove.exe>] [-Skip H,I,R,P]
#
# -App defaults to app\target\release\foxglove.exe built from this tree (its API is this tree's).
# -Monitor is a piece of the HUD monitor's Wallpaper Engine key (SAM). Takes about 25 minutes:
# H 3, I 3, R 4, P 14. Exit 0 PASS, 1 FAIL, 2 BLOCKED (nothing failed, something couldn't run).
# release-test.ps1 -Sitting runs it after the release gate.
param(
    [string]$App,
    [string]$Lab,
    [string]$Out,
    [string]$Monitor = 'SAM',
    [string[]]$Skip = @()
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$env:PYTHONIOENCODING = 'utf-8'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Push-Location $root
$results = [ordered]@{}
$running = @()
try {
    if (-not $App) { $App = Join-Path $root 'app\target\release\foxglove.exe' }
    if (-not (Test-Path -LiteralPath $App -PathType Leaf)) { throw "-App ${App}: not found (build the app from this tree)" }
    $App = (Resolve-Path -LiteralPath $App).Path
    if (-not $Lab) {
        & "$root\scripts\build-slot.ps1" cargo build -q --release -p foxglove-lab
        if ($LASTEXITCODE) { throw 'foxglove-lab did not build' }
        $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
        $Lab = Join-Path $target 'release\foxglove-lab.exe'
    }
    $Lab = (Resolve-Path -LiteralPath $Lab).Path
    if (-not $Out) {
        $sha = (git rev-parse --short HEAD).Trim()
        $Out = Join-Path $root ("lab-reports\{0:yyyyMMddTHHmmss}Z-{1}-sitting" -f (Get-Date).ToUniversalTime(), $sha)
    }
    New-Item -ItemType Directory -Force $Out | Out-Null
    if ($env:FOXGLOVE_APP_OK -ne '1') {
        Write-Host 'BLOCKED: the sitting launches test apps and takes the focus; On-call runs it with FOXGLOVE_APP_OK=1' -ForegroundColor Yellow
        exit 2
    }

    # One step: a command line, its output teed to <id>.txt, its exit code read as PASS/FAIL/BLOCKED.
    function Step([string]$Id, [string]$What, [scriptblock]$Run) {
        Write-Host "== $Id $What" -ForegroundColor Cyan
        # A tool's stderr line is output here, never a terminating error (PowerShell 5.1 turns a
        # redirected native stderr line into one under 'Stop').
        $ErrorActionPreference = 'Continue'
        $t0 = Get-Date
        $global:LASTEXITCODE = 0
        try { & $Run 2>&1 | Tee-Object -FilePath (Join-Path $Out "$Id.txt") | Out-Host; $code = $LASTEXITCODE }
        catch { Write-Host $_; $code = 1 }
        $status = switch ($code) { 0 { 'PASS' } 2 { 'BLOCKED' } default { 'FAIL' } }
        $results[$Id] = "$status  $What ({0:N0} s)" -f ((Get-Date) - $t0).TotalSeconds
        Write-Host "$Id $status" -ForegroundColor $(if ($status -eq 'PASS') { 'Green' } elseif ($status -eq 'BLOCKED') { 'Yellow' } else { 'Red' })
    }
    # A foxglove-lab suite case on its own: 0 PASS, 2 BLOCKED (a gate such as the right's power), 1 FAIL.
    function Case([string]$Suite, [string]$Name, [switch]$AllowApp) {
        $argv = @('run', '--suite', $Suite, '--only', $Name, '--non-interactive', '--out', (Join-Path $Out "case-$Name"))
        if ($AllowApp) { $argv = @('--allow-app') + $argv }
        & $Lab @argv
    }
    function Quit-Foxglove {
        # Exactly the app's process name: other sessions' test binaries must never be stopped.
        $procs = @(Get-Process -Name foxglove -ErrorAction SilentlyContinue)
        $script:running = @($procs | ForEach-Object { $_.Path } | Select-Object -Unique)
        $procs | Stop-Process -Confirm:$false
        $procs | Wait-Process -Timeout 20 -ErrorAction SilentlyContinue
    }
    function Start-Foxglove {
        foreach ($p in $script:running) { Start-Process -FilePath $p }
        $script:running = @()
    }

    if ($Skip -notcontains 'H') {
        # HA0: the state the H cases start from.
        Step 'HA0' 'wallpaper state and the keyboard' { python scripts/lab/wallpaper_check.py state; & $Lab --allow-app status }
        Step 'HA3' 'old H4: the Presets card lists the wallpapers (scenes.page)' { python scripts/lab/wallpaper_check.py presets --app $App }
        Step 'HA6' 'old H5: the keyboard keeps the gold it follows' { python scripts/lab/wallpaper_check.py gold }
        Step 'HA4' 'lights follow the wallpaper (test hook)' { python scripts/lab/wallpaper_follow.py --app $App --lab $Lab }
        # HA1: the devlog listener (bounded), Diagnostics on, the GAME layer on.
        $devlog = 'C:\Users\fuchs\bandit-wallpapers\tools\devlog.py'
        $listener = $null
        if (Test-Path -LiteralPath $devlog) {
            $listener = Start-Process python -ArgumentList @('-I', $devlog, '--minutes', '6', '--idle', '3') -PassThru -WindowStyle Hidden
        }
        $since = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        Step 'HA1' 'Diagnostics on, GAME layer on' {
            python scripts/lab/wallpaper_check.py debug on --monitor $Monitor
            if (-not $LASTEXITCODE) { Case 'tests/hil/sitting.toml' 'game-layer-on' -AllowApp }
        }
        Step 'HA5' 'old H1-H3: a full-screen game, the Win key, the HUD flicker' {
            python scripts/lab/game_flicker.py --monitor $Monitor --out (Join-Path $Out 'game-flicker')
        }
        Step 'HA2' 'the game, from the logs: game mode kept, no dropped -control, reloads' {
            python scripts/lab/wallpaper_check.py game --since $since
        }
        Step 'HA2b' 'Diagnostics off, GAME layer off' {
            python scripts/lab/wallpaper_check.py debug off --monitor $Monitor
            Case 'tests/hil/sitting.toml' 'game-layer-off' -AllowApp
        }
        if ($listener -and -not $listener.HasExited) { $listener | Stop-Process -Confirm:$false }
    }

    if ($Skip -notcontains 'I') {
        Step 'IA1-IA3' 'old I1-I4: overlay click-through, focus, real clicks' {
            python scripts/lab/overlay_tray.py --app $App --lab $Lab --focus --real-clicks
        }
    }

    if ($Skip -notcontains 'R') {
        Quit-Foxglove
        try {
            Step 'RA1' 'a SELECT while dark is drawn on the wake' { Case 'tests/hil/sitting.toml' 'right-redraw-after-select-while-idle' }
            Step 'RA2' 'take and hand back the LEDs: the right rewritten each time' { python scripts/lab/right_redraws.py --lab $Lab }
            Step 'RA3' 'idle and a LEFT tap rewrite the right' { Case 'tests/hil/resilience.toml' 'right-redraw-after-wake' }
            Step 'RA4' 'old R1-R2: on USB power, clean after the wake' { Case 'tests/hil/sitting.toml' 'right-clean-wake-usb' }
            Step 'RA5' 'old R3-R5: on its battery, clean after the wake (runs when the right is unplugged)' { Case 'tests/hil/sitting.toml' 'right-clean-wake-battery' }
        } finally { Start-Foxglove }
    }

    if ($Skip -notcontains 'P') {
        Step 'PA1-PA4' 'old P1-P6: display off and on (test hook), quit, dark on its own' {
            python scripts/lab/pc_away.py --app $App --lab $Lab --history
        }
    }
} finally {
    Start-Foxglove
    Pop-Location
}
$lines = @($results.GetEnumerator() | ForEach-Object { "$($_.Key)  $($_.Value)" })
$failed = @($results.Values | Where-Object { $_ -like 'FAIL*' })
# RA5 waits for the right on its battery: BLOCKED there alone is the expected gate, not a hole.
$blocked = @($results.GetEnumerator() | Where-Object { $_.Value -like 'BLOCKED*' -and $_.Key -ne 'RA5' })
$summary = if ($failed) { 'SITTING: FAIL' } elseif ($blocked) { 'SITTING: BLOCKED' } else { 'SITTING: PASS' }
@($summary) + $lines + @("app: $App", "report: $Out") | Out-File -Encoding utf8 (Join-Path $Out 'summary.txt')
$lines | ForEach-Object { Write-Host $_ }
Write-Host $summary -ForegroundColor $(if ($failed) { 'Red' } elseif ($blocked) { 'Yellow' } else { 'Green' })
Write-Host "Report: $Out"
if ($failed) { exit 1 }
if ($blocked) { exit 2 }
exit 0
