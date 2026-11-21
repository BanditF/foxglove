# MIT. Hourly stall check, run by Task Scheduler (register-stall-check.ps1): runs stall-check.ps1 and
# wakes Claude ONLY when `action` is not none. A quiet hour costs no Claude tokens at all.
#   none            nothing.
#   alarm           one message to the Hub (the alarm lines).
#   freeze_lifted   one message to the Hub (game over, resume Planner and On-call).
#   pause           disable this Windows task, then one message to the Hub.
# The wake is `claude -p` with the exact message text already written here, so Claude only relays it.
# If claude.exe cannot be found or fails, nothing is lost: stall-status.json and stall-alarm.txt
# (written by stall-check.ps1) hold the same facts for the Hub to read.
# Tests (window-free): -ActionFile feeds a fake stall-check result, -DryRun prints instead of acting,
# -ClaudeExe can point at a stub.
param(
    [string]$Script = (Join-Path $PSScriptRoot 'stall-check.ps1'),
    [string]$TaskName = 'Foxglove Stall Check',
    [string]$HubId = 'local_a0a18f0e-fccb-42df-872a-8c2cdeea0855',
    [string]$ClaudeExe,
    [string]$ActionFile,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'

function Find-Claude([string]$Given) {
    $c = @($Given, $env:FOXGLOVE_CLAUDE_EXE) + @((Get-Command claude -ErrorAction SilentlyContinue).Source) +
         @("$env:USERPROFILE\.local\bin\claude.exe", "$env:APPDATA\npm\claude.cmd")
    foreach ($p in $c) { if ($p -and (Test-Path $p)) { return $p } }
    return $null
}

# Pure: the stall-check result in, what to do out. $null when nothing is to be done.
function Get-WakePlan($d) {
    switch ($d.action) {
        'alarm' {
            $lines = @($d.alarm.lines); if (-not $lines.Count) { $lines = @($d.lines) }
            return @{ disable = $false; message = ("Stall check:`n" + ($lines -join "`n")) }
        }
        'freeze_lifted' { return @{ disable = $false; message = 'Stall check: game over, freeze lifted. Resume Planner and On-call.' } }
        'pause' { return @{ disable = $true; message = 'Stall check: all quiet (done or waiting on the user), paused myself. Re-enable the Foxglove Stall Check task when work restarts.' } }
        default { return $null }
    }
}

if ($MyInvocation.InvocationName -eq '.') { return }   # dot-sourced by a test: functions only

$raw = if ($ActionFile) { Get-Content $ActionFile -Raw } else {
    (& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $Script | Out-String)
}
$start = $raw.IndexOf('{')
if ($start -lt 0) { Write-Host 'stall-check printed no JSON; nothing to do'; exit 0 }
$d = $raw.Substring($start) | ConvertFrom-Json
$plan = Get-WakePlan $d
if (-not $plan) { Write-Host "action $($d.action): no wake"; exit 0 }

if ($plan.disable) {
    if ($DryRun) { Write-Host "DRYRUN would disable task '$TaskName'" }
    else { Disable-ScheduledTask -TaskName $TaskName | Out-Null }
}
$claude = Find-Claude $ClaudeExe
$prompt = "Send exactly ONE message with the SendMessage tool, to $HubId, with exactly this text (lines are separated by ' // ', keep them), then stop. Do nothing else. TEXT: $($plan.message -replace '\s*\r?\n', ' // ' -replace '"', "'")"
if ($DryRun) { Write-Host "DRYRUN action $($d.action); claude=$claude"; Write-Host $prompt; exit 0 }
if (-not $claude) { Write-Host 'claude not found; alarm stays in stall-alarm.txt / stall-status.json'; exit 0 }
& $claude -p $prompt --model haiku --allowedTools SendMessage --max-turns 4 | Out-Null
exit 0
