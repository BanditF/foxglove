# MIT. The one shared game check for the ops scripts (#949), moved out of the git-ignored
# tools/local-llm.ps1. Exit 0: no game. Exit 3: a game runs (the reason is printed unless -Quiet).
# The list is data: scripts/ops/games.json. Read-only, no launches.
#   powershell -File scripts\ops\game-check.ps1 [-Quiet] [-ListFile games.json]
# Test hooks (window-free tests): FOXGLOVE_OPS_GAME_PATHS = semicolon-separated fake process
# paths, FOXGLOVE_OPS_GPU_UTIL = fake GPU utilisation percent. When either is set the real
# process list and nvidia-smi are not consulted.
param([switch]$Quiet, [string]$ListFile = (Join-Path $PSScriptRoot 'games.json'))

function Get-GameReason {
    param($List, [string[]]$Paths, $GpuUtil)
    $pattern = ($List.libraryPatterns -join '|')
    foreach ($p in $Paths) {
        if ($p -and ($p -match $pattern) -and ($p -notmatch $List.notGamesPattern)) { return "game process: $p" }
    }
    if ($null -ne $GpuUtil -and [int]$GpuUtil -ge [int]$List.gpuBusyPercent) { return "GPU busy at $GpuUtil%" }
    return $null
}

function Get-RealPaths {
    foreach ($p in (Get-Process -ErrorAction SilentlyContinue)) { try { $p.Path } catch {} }
}

function Get-RealGpuUtil {
    $smi = Get-Command nvidia-smi -ErrorAction SilentlyContinue
    if (-not $smi) { return $null }
    $u = & $smi.Source --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>$null | Select-Object -First 1
    if ($u) { return [int]$u.Trim() }
    return $null
}

if ($MyInvocation.InvocationName -ne '.') {
    $list = Get-Content -Raw -Encoding UTF8 $ListFile | ConvertFrom-Json
    if ($env:FOXGLOVE_OPS_GAME_PATHS -or $env:FOXGLOVE_OPS_GPU_UTIL) {
        $paths = @($env:FOXGLOVE_OPS_GAME_PATHS -split ';' | Where-Object { $_ })
        $gpu = if ($env:FOXGLOVE_OPS_GPU_UTIL) { [int]$env:FOXGLOVE_OPS_GPU_UTIL } else { $null }
    } else {
        $paths = @(Get-RealPaths)
        $gpu = Get-RealGpuUtil
    }
    $why = Get-GameReason -List $list -Paths $paths -GpuUtil $gpu
    if ($why) { if (-not $Quiet) { Write-Host "game-check: $why" }; exit 3 }
    exit 0
}
