# MIT. Hideout demo: adopt Vulpes (Arete's module) and Foxglove into an ISOLATED hub folder, then
# optionally start that hub. Runbook: docs/hideout-demo.md.
#
#   powershell -File scripts\ops\demo-up.ps1 -AreteModuleDir <folder> -HideoutExe <hideout.exe> -FoxgloveExe <foxglove.exe>
#       DRY RUN (default): prints the plan and the checks; writes nothing, runs nothing.
#   ... -Adopt    registers the two modules in the demo folder (`hideout.exe --adopt`: no window, starts nothing).
#   ... -Launch   does -Adopt, then starts the demo hub. This is the only switch that starts anything.
#   ... -Rollback [-Launch]  stops the demo hub, then (with -Launch) starts -FoxgloveExe with NO arguments and no demo
#       environment: Foxglove on its own, as today. Touches no data (the demo folder stays unless -Clean).
#   ... -Down     stops only the demo hub (by its pid file); with -Clean also deletes the demo folder.
#
# Isolation: HIDEOUT_CONFIG_DIR (the hub's files) and FOXGLOVE_CONFIG_DIR (Foxglove's data) are set to
# folders under -DemoHome for the child processes only; the user's real Hideout/Foxglove folders are
# refused as -DemoHome and never read or written.
param(
    [string]$AreteModuleDir,            # folder holding Arete's module.toml (run[0] = this PC's arete.exe)
    [string]$AreteExe,                  # optional: arete.exe; the demo copy of module.toml gets it as run[0]
    [string]$HideoutExe,
    [string]$FoxgloveExe,               # a foxglove.exe of THIS build (its version goes into the manifest)
    [string]$FoxgloveVersion,           # default: app/Cargo.toml's version
    [string]$DemoHome,                  # default: %TEMP%\hideout-demo
    [switch]$Adopt,
    [switch]$Launch,
    [switch]$Down,
    [switch]$Rollback,                  # one step back to standalone Foxglove: stop the demo hub; with -Launch start foxglove.exe plain
    [switch]$Clean
)
$ErrorActionPreference = 'Stop'
$script:Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

function Get-DemoHome([string]$Requested) {
    $h = if ($Requested) { $Requested } else { Join-Path ([IO.Path]::GetTempPath()) 'hideout-demo' }
    $h = [IO.Path]::GetFullPath($h).TrimEnd('\')
    foreach ($base in $env:LOCALAPPDATA, $env:APPDATA) {
        if (-not $base) { continue }
        foreach ($name in 'Hideout', 'Foxglove') {
            $r = [IO.Path]::GetFullPath($base).TrimEnd('\') + '\' + $name
            if ($h -ieq $r -or $h.StartsWith($r + '\', [StringComparison]::OrdinalIgnoreCase)) {
                throw "refusing -DemoHome '$h': it is the user's real $name folder"
            }
        }
    }
    $h
}

# `run[0]` of a manifest's text (the first string of the `run = [...]` line), or $null.
function Get-RunExe([string]$Text) {
    $m = [regex]::Match($Text, '(?m)^\s*run\s*=\s*\[\s*(?:"((?:[^"\\]|\\.)*)"|''([^'']*)'')')
    if (-not $m.Success) { return $null }
    if ($m.Groups[1].Success) { return ($m.Groups[1].Value -replace '\\\\', '\') }
    $m.Groups[2].Value
}

function Get-ManifestVersion([string]$Text) {
    $m = [regex]::Match($Text, '(?m)^\s*version\s*=\s*"([^"]+)"')
    if ($m.Success) { $m.Groups[1].Value } else { $null }
}

# Foxglove's manifest for this PC: the repo's template with @VERSION@ and @EXE@ filled (as hub_link::manifest does).
function New-FoxgloveManifest([string]$Template, [string]$Version, [string]$Exe) {
    $Template.Replace('@VERSION@', $Version).Replace('@EXE@', $Exe.Replace('\', '\\'))
}

# Arete's manifest with run[0] replaced by a real exe; the rest is kept as given.
function Set-AreteRun([string]$Text, [string]$Exe) {
    $q = $Exe.Replace('\', '/')
    [regex]::Replace($Text, '(?m)^(\s*run\s*=\s*\[\s*)"[^"]*"', { param($m) $m.Groups[1].Value + '"' + $q + '"' })
}

function Test-Registered([string]$HubHome, [string]$Id) {
    $inv = Join-Path $HubHome 'inventory.json'
    if (-not (Test-Path $inv)) { return $false }
    $j = Get-Content -Raw $inv | ConvertFrom-Json
    if ($null -eq $j.modules) { return $false }
    [bool]($j.modules.PSObject.Properties.Name -contains $Id)
}

function Invoke-DemoUp {
    try { $demo = Get-DemoHome $DemoHome } catch { Write-Host "PROBLEM  : $($_.Exception.Message)"; return 1 }
    $hubHome = Join-Path $demo 'hub'
    $fgData = Join-Path $demo 'foxglove-data'
    $adoptDir = Join-Path $demo 'adopt'
    $pidFile = Join-Path $demo 'hub.pid'
    $apply = $Adopt -or $Launch
    $problems = New-Object System.Collections.Generic.List[string]
    $mode = if ($Rollback) { 'ROLLBACK' } elseif ($Down) { 'DOWN' } elseif ($Launch) { 'LAUNCH' } elseif ($Adopt) { 'ADOPT only' } else { 'DRY RUN' }
    Write-Host "demo home: $demo   (mode: $mode)"

    if ($Down -or $Rollback) {
        if (Test-Path $pidFile) {
            $id = [int](Get-Content $pidFile -Raw)
            $p = Get-Process -Id $id -ErrorAction SilentlyContinue
            if ($p -and $p.ProcessName -eq 'hideout') { Write-Host "stopping demo hub pid $id"; Stop-Process -Id $id }
            else { Write-Host "demo hub pid $id is not running (or not a hideout process): nothing stopped" }
            Remove-Item $pidFile -ErrorAction SilentlyContinue
        } else { Write-Host 'no hub.pid: nothing stopped' }
        if ($Clean) { Write-Host "deleting $demo"; Remove-Item -Recurse -Force $demo -ErrorAction SilentlyContinue }
        if ($Rollback) {
            if ($FoxgloveExe -and (Test-Path $FoxgloveExe)) {
                if ($Launch) { $p = Start-Process -FilePath $FoxgloveExe -PassThru; Write-Host "started standalone Foxglove pid $($p.Id) (no --module, no demo environment)" }
                else { Write-Host "dry run for the restart: with -Launch this starts: $FoxgloveExe (no arguments). The module-mode Foxglove ends by itself after the hub goes (30 s grace); do not start the standalone one while it still runs." }
            } else { Write-Host 'standalone Foxglove: pass -FoxgloveExe to start it, or start it as usual' }
        }
        return 0
    }

    # --- inputs ---
    $areteText = $null
    if (-not $AreteModuleDir) { $problems.Add("-AreteModuleDir is required (the folder with Arete's module.toml)") }
    elseif (-not (Test-Path (Join-Path $AreteModuleDir 'module.toml'))) { $problems.Add("no module.toml in $AreteModuleDir") }
    else {
        $areteText = Get-Content -Raw -Encoding UTF8 (Join-Path $AreteModuleDir 'module.toml')
        if ($AreteExe) {
            if (-not (Test-Path $AreteExe)) { $problems.Add("-AreteExe not found: $AreteExe") } else { $areteText = Set-AreteRun $areteText $AreteExe }
        }
        $run = Get-RunExe $areteText
        if (-not $run) { $problems.Add('Arete module.toml has no run = [...]') }
        elseif ($run -match 'PLACEHOLDER') { $problems.Add("Arete run[0] is still the placeholder ($run): pass -AreteExe <arete.exe>") }
        elseif (-not (Test-Path $run)) { $problems.Add("Arete run[0] does not exist: $run") }
        else { Write-Host "vulpes   : $(Get-ManifestVersion $areteText)  run[0] = $run" }
    }
    $tmplPath = Join-Path $script:Root 'app\src\foxglove\hub_link\module.toml'
    $fgVer = $FoxgloveVersion
    if (-not $fgVer) {
        $m = [regex]::Match((Get-Content -Raw (Join-Path $script:Root 'app\Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"')
        if ($m.Success) { $fgVer = $m.Groups[1].Value }
    }
    if (-not $FoxgloveExe) { $problems.Add('-FoxgloveExe is required (a foxglove.exe of this build)') }
    elseif (-not (Test-Path $FoxgloveExe)) { $problems.Add("-FoxgloveExe not found: $FoxgloveExe") }
    elseif (-not (Test-Path $tmplPath)) { $problems.Add("Foxglove manifest template missing: $tmplPath") }
    else { Write-Host "foxglove : $fgVer  run[0] = $FoxgloveExe  (--module)" }
    if (-not $HideoutExe) { $problems.Add('-HideoutExe is required') }
    elseif (-not (Test-Path $HideoutExe)) { $problems.Add("-HideoutExe not found: $HideoutExe") }

    # --- plan ---
    $already = @{}
    foreach ($id in 'vulpes', 'foxglove') { $already[$id] = Test-Registered $hubHome $id }
    Write-Host "isolation: HIDEOUT_CONFIG_DIR=$hubHome  FOXGLOVE_CONFIG_DIR=$fgData (child processes only)"
    foreach ($id in 'vulpes', 'foxglove') {
        $what = if ($already[$id]) { 'already registered: skip' } else { 'register with hideout.exe --adopt' }
        Write-Host ('plan     : {0,-9} {1}' -f $id, $what)
    }
    $hub = if ($Launch) { 'YES (-Launch)' } else { 'no (needs -Launch)' }
    Write-Host "plan     : hub start $hub; nothing else is started by this script"
    foreach ($p in $problems) { Write-Host "PROBLEM  : $p" }
    if (-not $apply) { Write-Host 'dry run: nothing written, nothing started. Add -Adopt or -Launch.'; return [int]($problems.Count -gt 0) }
    if ($problems.Count) { return 1 }

    # --- apply ---
    New-Item -ItemType Directory -Force $hubHome, $fgData, $adoptDir | Out-Null
    $env:HIDEOUT_CONFIG_DIR = $hubHome; $env:FOXGLOVE_CONFIG_DIR = $fgData
    try {
        $utf8 = New-Object Text.UTF8Encoding $false
        if (-not $already['vulpes']) {
            $d = Join-Path $adoptDir 'vulpes'; New-Item -ItemType Directory -Force $d | Out-Null
            [IO.File]::WriteAllText((Join-Path $d 'module.toml'), $areteText, $utf8)
            $r = & $HideoutExe --adopt $d 2>&1; Write-Host "adopt vulpes: $r"; if ($LASTEXITCODE) { return 1 }
        }
        if (-not $already['foxglove']) {
            $d = Join-Path $adoptDir 'foxglove'; New-Item -ItemType Directory -Force $d | Out-Null
            $fgText = New-FoxgloveManifest (Get-Content -Raw -Encoding UTF8 $tmplPath) $fgVer $FoxgloveExe
            [IO.File]::WriteAllText((Join-Path $d 'module.toml'), $fgText, $utf8)
            $r = & $HideoutExe --adopt $d 2>&1; Write-Host "adopt foxglove: $r"; if ($LASTEXITCODE) { return 1 }
        }
        foreach ($id in 'vulpes', 'foxglove') { Write-Host ('check    : inventory has {0}: {1}' -f $id, (Test-Registered $hubHome $id)) }
        if ($Launch) {
            $p = Start-Process -FilePath $HideoutExe -PassThru
            Set-Content -Path $pidFile -Value $p.Id
            Write-Host "started demo hub pid $($p.Id) (pid file $pidFile). Undo: demo-up.ps1 -Down -Clean -DemoHome '$demo'"
            Write-Host "check    : log $hubHome\logs\hideout.log; the hub window is the demo (docs/hideout-demo.md)"
        }
    } finally { Remove-Item Env:HIDEOUT_CONFIG_DIR, Env:FOXGLOVE_CONFIG_DIR -ErrorAction SilentlyContinue }
    0
}

if ($MyInvocation.InvocationName -ne '.') { exit (Invoke-DemoUp) }
