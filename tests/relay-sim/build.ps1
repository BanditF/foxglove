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
# The self-report (#705) where the widgets have it (bisect.ps1 builds older ones too).
$healthSrc = ""
$healthObj = ""
if (Test-Path (Join-Path $Widgets "health.c")) {
    $healthSrc = "`"$Widgets\health.c`""
    $healthObj = "`"$Out\health.obj`""
}
# The D7 05 position press (#851): test_hooks.c as dev builds have it, where the widgets have
# the art relay's press job.
$hooksSrc = ""
$hooksObj = ""
if (Select-String -Quiet -Path (Join-Path $Widgets "art_relay_central.c") -Pattern "art_relay_press") {
    $hooksSrc = "`"$Widgets\test_hooks.c`""
    $hooksObj = "`"$Out\test_hooks.obj`""
    $defs += " /DSIM_TEST_HOOKS=1"
}
$cmd = @"
call "$vcvars" >nul && cd /d "$Out" && cl $common /c "$here\sim_coro.c" /Fo"$Out\sim_coro.obj" && cl $common /Zp1 $defs $inc /c "$here\sim_kernel.c" "$here\sim_bt.c" "$here\sim_stubs.c" "$here\main.c" "$Widgets\art_relay_central.c" $healthSrc $hooksSrc && cl $common /Fe"$exe" "$Out\sim_coro.obj" "$Out\sim_kernel.obj" "$Out\sim_bt.obj" "$Out\sim_stubs.obj" "$Out\main.obj" "$Out\art_relay_central.obj" $healthObj $hooksObj
"@
cmd /c $cmd
if ($LASTEXITCODE -ne 0) { throw "relay-sim: build failed" }
# hooks-real-keys (#851, hooks_sim.c): test_hooks.c alone, built as the left and as the right.
$hooksExe = @{}
$hooksFile = Join-Path $Widgets "test_hooks.c"
if ((Test-Path $hooksFile) -and (Select-String -Quiet -Path $hooksFile -Pattern "HOLD_MAX_MS")) {
    $rightDefs = ($RelaySimDefines | Where-Object { $_ -ne "CONFIG_ZMK_SPLIT_ROLE_CENTRAL=1" } | ForEach-Object { "/D$_" }) -join " "
    $rightDefs += " /DLAMP_STRIP_LEN=21" # the right's own chain (lamp_parts.h)
    foreach ($h in @(@("left", $defs), @("right", $rightDefs))) {
        $dir = Join-Path $Out ("hooks-" + $h[0])
        New-Item -ItemType Directory -Force $dir | Out-Null
        $hexe = Join-Path $dir "hooks.exe"
        $hcmd = "call `"$vcvars`" >nul && cd /d `"$dir`" && cl $common /Zp1 $($h[1]) $inc /Fe`"$hexe`" `"$here\sim_kernel.c`" `"$here\hooks_sim.c`" `"$hooksFile`" `"$Out\sim_coro.obj`""
        cmd /c $hcmd
        if ($LASTEXITCODE -ne 0) { throw "relay-sim: hooks-$($h[0]) build failed" }
        $hooksExe[$h[0]] = $hexe
    }
}
$list = if ($Scenarios) { $Scenarios -split "," } else { $RelaySimScenarios }
$fail = 0
foreach ($s in $list) {
    if ($s -eq "hooks-real-keys") {
        foreach ($h in @("left", "right")) {
            if (-not $hooksExe[$h]) { Write-Host "relay-sim: hooks-$h not built"; $fail++; continue }
            & $hooksExe[$h]
            if ($LASTEXITCODE -ne 0) { $fail++ }
        }
        continue
    }
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
