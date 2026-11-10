# MIT. Shared helpers for the scripts in scripts/ops (#949). Dot-source it.
# Every ops script: hard time limit (a watchdog child, no window), below-normal priority, exits
# early with 3 while a game runs, never launches a GUI, writes ONE summary line.

function Get-OpsDir {
    $d = if ($env:FOXGLOVE_OPS_DIR) { $env:FOXGLOVE_OPS_DIR } else { Join-Path $env:LOCALAPPDATA 'Foxglove\ops' }
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Force $d | Out-Null }
    $d
}

$script:OpsWatchdog = $null
$script:OpsName = 'ops'

function Start-OpsJob {
    param([Parameter(Mandatory)][string]$Name, [int]$TimeLimitSec = 600, [switch]$SkipGameCheck)
    $script:OpsName = $Name
    try { (Get-Process -Id $PID).PriorityClass = 'BelowNormal' } catch {}
    if ($TimeLimitSec -gt 0) {
        # A separate hidden process ends this one if it overruns, even inside a native call.
        $cmd = "Wait-Process -Id $PID -Timeout $TimeLimitSec -ErrorAction SilentlyContinue; " +
               "if (Get-Process -Id $PID -ErrorAction SilentlyContinue) { Stop-Process -Id $PID -Force }"
        $script:OpsWatchdog = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList `
            '-NoProfile', '-NonInteractive', '-Command', $cmd
    }
    if (-not $SkipGameCheck) {
        & (Join-Path $PSScriptRoot 'game-check.ps1') -Quiet
        if ($LASTEXITCODE -eq 3) { Stop-OpsJob -Status 'skipped' -Text 'game running' -Code 3 }
    }
}

# The one summary line, to stdout and to ops\summary.log. Ends the process with $Code.
function Stop-OpsJob {
    param([string]$Status = 'ok', [string]$Text = '', [int]$Code = 0)
    $line = '{0} {1} {2}: {3}' -f (Get-Date -Format s), $script:OpsName, $Status, $Text
    Write-Host $line
    try { Add-Content -Encoding UTF8 -Path (Join-Path (Get-OpsDir) 'summary.log') -Value $line } catch {}
    if ($script:OpsWatchdog) { try { Stop-Process -Id $script:OpsWatchdog.Id -Force -ErrorAction SilentlyContinue } catch {} }
    exit $Code
}

# Runs an external command with a hard wait; returns @{Code; Out; Err}. Code -1 on timeout (killed).
function Invoke-Bounded {
    param([Parameter(Mandatory)][string]$File, [string[]]$Arguments = @(), [int]$TimeoutSec = 60, [string]$WorkDir)
    $o = [IO.Path]::GetTempFileName(); $e = [IO.Path]::GetTempFileName()
    try {
        $sp = @{ FilePath = $File; NoNewWindow = $true; PassThru = $true; RedirectStandardOutput = $o; RedirectStandardError = $e }
        if ($Arguments.Count) { $sp.ArgumentList = ($Arguments | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join ' ' }
        if ($WorkDir) { $sp.WorkingDirectory = $WorkDir }
        $p = Start-Process @sp
        $null = $p.Handle
        if (-not $p.WaitForExit($TimeoutSec * 1000)) { try { $p.Kill() } catch {}; return @{ Code = -1; Out = ''; Err = 'timeout' } }
        $p.WaitForExit()
        @{ Code = $p.ExitCode; Out = (Get-Content -Raw -Encoding UTF8 $o); Err = (Get-Content -Raw -Encoding UTF8 $e) }
    } finally { Remove-Item $o, $e -ErrorAction SilentlyContinue }
}
