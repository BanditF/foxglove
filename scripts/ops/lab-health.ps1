# MIT. The post-batch foxglove-lab health check (#705, #949) as a script: runs
# `foxglove-lab health` (the CLI; it reads the app's health.read over its API and launches no
# window) under a hard time limit, after the shared game check. It builds nothing and starts no
# app. Exit 0 pass, 1 fail/warn, 3 game running. The report goes to ops\lab-health-<time>.txt.
#   powershell -File scripts\ops\lab-health.ps1 [-Lab <foxglove-lab.exe>] [-TimeLimitSec 120]
param([string]$Lab, [int]$TimeLimitSec = 120)
. (Join-Path $PSScriptRoot 'ops-common.ps1')

function Find-Lab([string]$Explicit) {
    $repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
    $c = @($Explicit, $env:FOXGLOVE_LAB, (Join-Path $repo 'target\release\foxglove-lab.exe')) | Where-Object { $_ }
    foreach ($x in $c) { if (Test-Path $x) { return (Resolve-Path $x).Path } }
    $null
}

# Pure: one status word from the lab's exit code.
function Get-LabStatus([int]$Code) {
    switch ($Code) { 0 { 'ok' } -1 { 'timeout' } default { 'FAIL' } }
}

if ($MyInvocation.InvocationName -ne '.') {
    Start-OpsJob -Name 'lab-health' -TimeLimitSec ($TimeLimitSec + 30)
    $exe = Find-Lab $Lab
    if (-not $exe) { Stop-OpsJob -Status 'warn' -Text 'foxglove-lab.exe not found (build it with scripts\build-slot.ps1 first)' -Code 1 }
    $r = Invoke-Bounded -File $exe -Arguments @('health') -TimeoutSec $TimeLimitSec
    $rep = Join-Path (Get-OpsDir) ("lab-health-{0}.txt" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
    Set-Content -Encoding UTF8 $rep ([string]$r.Out + "`n" + [string]$r.Err)
    $st = Get-LabStatus $r.Code
    $warns = @(([string]$r.Out -split "`r?`n") | Where-Object { $_ -match 'WARN|FAIL' }).Count
    Stop-OpsJob -Status $st -Text "foxglove-lab health exit $($r.Code), $warns WARN/FAIL rows; report $rep" -Code $(if ($r.Code -eq 0) { 0 } else { 1 })
}
