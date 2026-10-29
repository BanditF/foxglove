# relay-sim on Windows with MSVC (Build Tools 2022). Builds the harness against a widgets
# directory (default: this checkout's) and runs the scenarios. CI uses run.sh with gcc.
#   powershell -File tests/relay-sim/build.ps1 [-Widgets <dir>] [-Out <dir>] [-Scenarios a,b]
param(
    [string]$Widgets = "",
    [string]$Out = "",
    [string]$Scenarios = "", # comma separated; default: all of config.ps1
    [int]$Log = 2
)
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$root = Resolve-Path (Join-Path $here "..\..")
if (-not $Widgets) { $Widgets = Join-Path $root "boards\shields\nice_view_custom\widgets" }
if (-not $Out) { $Out = Join-Path $root "target\relay-sim" }
New-Item -ItemType Directory -Force $Out | Out-Null
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & "$vswhere" -latest -products "*" -property installationPath
$vcvars = Join-Path $vs "VC\Auxiliary\Build\vcvars64.bat"
$env:PATH = "$(Split-Path $vswhere);$env:PATH" # vcvars64.bat calls vswhere.exe by name
. (Join-Path $here "config.ps1")
$defs = ($RelaySimDefines | ForEach-Object { "/D$_" }) -join " "
$common = "/nologo /std:c11 /Zc:preprocessor /W3 /wd4200 /wd4244 /wd4267 /wd4146 /wd4996 /Od /Zi"
$inc = "/I`"$here\include`" /I`"$Widgets`" /I`"$here`""
$exe = Join-Path $Out "relay-sim.exe"
$cmd = @"
call "$vcvars" >nul && cd /d "$Out" && cl $common /c "$here\sim_coro.c" /Fo"$Out\sim_coro.obj" && cl $common /Zp1 $defs $inc /c "$here\sim_kernel.c" "$here\sim_bt.c" "$here\sim_stubs.c" "$here\main.c" "$Widgets\art_relay_central.c" && cl $common /Fe"$exe" "$Out\sim_coro.obj" "$Out\sim_kernel.obj" "$Out\sim_bt.obj" "$Out\sim_stubs.obj" "$Out\main.obj" "$Out\art_relay_central.obj"
"@
cmd /c $cmd
if ($LASTEXITCODE -ne 0) { throw "relay-sim: build failed" }
$list = if ($Scenarios) { $Scenarios -split "," } else { $RelaySimScenarios }
$fail = 0
foreach ($s in $list) {
    & $exe $s $Log
    if ($LASTEXITCODE -ne 0) { $fail++ }
}
if (-not $Scenarios) {
    foreach ($s in $RelaySimKnownGaps) {
        & $exe $s 1 | Select-String "^RESULT "
        if ($LASTEXITCODE -eq 0) { Write-Host "relay-sim: known gap $s passed: update config.ps1, run.sh and README.md"; $fail++ }
        else { Write-Host "relay-sim: known gap $s fails as documented" }
    }
}
if ($fail) { Write-Host "relay-sim: $fail scenario(s) failed"; exit 1 }
Write-Host "relay-sim: all scenarios passed"
