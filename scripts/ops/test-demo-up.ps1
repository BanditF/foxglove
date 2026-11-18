# MIT. Window-free tests for scripts/ops/demo-up.ps1: a fake hideout.exe (a .cmd that only logs), fake
# arete/foxglove exes (empty files), temp folders. Nothing real is started. Exit 0 if all pass.
#   powershell -File scripts\ops\test-demo-up.ps1
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$fails = 0
function Check([string]$name, $cond) { if ($cond) { Write-Host "ok   $name" } else { Write-Host "FAIL $name"; $script:fails++ } }
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("demo-up-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $tmp | Out-Null
$ps = (Get-Command powershell).Source
$script_ = Join-Path $here 'demo-up.ps1'
try {
    $arete = Join-Path $tmp 'arete.exe'; Set-Content $arete 'x'
    $fox = Join-Path $tmp 'foxglove.exe'; Set-Content $fox 'x'
    $log = Join-Path $tmp 'mock.log'
    $mock = Join-Path $tmp 'hideout.cmd'
    Set-Content $mock "@echo %1 %2 cfg=%HIDEOUT_CONFIG_DIR% fg=%FOXGLOVE_CONFIG_DIR%>>`"$log`"" -Encoding ASCII
    $mdir = Join-Path $tmp 'arete-mod'; New-Item -ItemType Directory $mdir | Out-Null
    $aretePath = $arete.Replace('\', '/')
    Set-Content (Join-Path $mdir 'module.toml') "id = `"vulpes`"`nversion = `"0.1.0`"`nkind = `"adopted`"`nrun = [`"$aretePath`", `"hub`"]`n"
    $ph = Join-Path $tmp 'arete-ph'; New-Item -ItemType Directory $ph | Out-Null
    Set-Content (Join-Path $ph 'module.toml') "id = `"vulpes`"`nversion = `"0.1.0`"`nkind = `"adopted`"`nrun = [`"C:/Users/PLACEHOLDER/Arete/arete.exe`", `"hub`"]`n"
    $home1 = Join-Path $tmp 'demo1'
    $common = @('-NoProfile', '-File', $script_, '-AreteModuleDir', $mdir, '-HideoutExe', $mock, '-FoxgloveExe', $fox, '-DemoHome', $home1)

    # dry run: default, writes nothing, runs nothing
    $o = & $ps @common 2>&1 | Out-String
    Check 'dry run: exit 0' ($LASTEXITCODE -eq 0)
    Check 'dry run: says dry run and plans both modules' ($o -match 'DRY RUN' -and $o -match 'plan\s+: vulpes' -and $o -match 'plan\s+: foxglove')
    Check 'dry run: no demo folder, no hideout call' (-not (Test-Path $home1) -and -not (Test-Path $log))
    Check 'dry run: hub not started' ($o -match 'no \(needs -Launch\)')

    # problems: placeholder run[0], missing exe
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $ph -HideoutExe $mock -FoxgloveExe $fox -DemoHome $home1 2>&1 | Out-String
    Check 'placeholder run[0] is a PROBLEM, dry run exits 1' ($LASTEXITCODE -eq 1 -and $o -match 'PROBLEM.*placeholder')
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $ph -AreteExe $arete -HideoutExe $mock -FoxgloveExe $fox -DemoHome $home1 2>&1 | Out-String
    Check '-AreteExe fixes the placeholder' ($LASTEXITCODE -eq 0 -and $o -notmatch 'PROBLEM')
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $mdir -HideoutExe $mock -FoxgloveExe (Join-Path $tmp 'nope.exe') -DemoHome $home1 2>&1 | Out-String
    Check 'missing foxglove.exe is a PROBLEM' ($LASTEXITCODE -eq 1 -and $o -match 'PROBLEM.*not found')

    # the user's real folders are refused
    $real = Join-Path $env:LOCALAPPDATA 'Hideout'
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $mdir -HideoutExe $mock -FoxgloveExe $fox -DemoHome $real -Adopt 2>&1 | Out-String
    Check 'real %LOCALAPPDATA%\Hideout refused as demo home' ($LASTEXITCODE -ne 0 -and $o -match 'refusing' -and -not (Test-Path $log))
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $mdir -HideoutExe $mock -FoxgloveExe $fox -DemoHome (Join-Path $env:APPDATA 'Foxglove\x') -Adopt 2>&1 | Out-String
    Check 'a folder under %APPDATA%\Foxglove refused too' ($LASTEXITCODE -ne 0 -and $o -match 'refusing')

    # -Adopt: two --adopt calls, isolated env, manifests written, no hub start
    $o = & $ps @common -Adopt 2>&1 | Out-String
    Check '-Adopt: exit 0' ($LASTEXITCODE -eq 0)
    $l = @(Get-Content $log)
    Check '-Adopt: exactly two --adopt calls' ($l.Count -eq 2 -and ($l | Where-Object { $_ -like '--adopt *' }).Count -eq 2)
    Check '-Adopt: child env is the demo folder' (($l -join '|') -like "*cfg=$home1\hub fg=$home1\foxglove-data*")
    Check '-Adopt: env not leaked to the caller' (-not $env:HIDEOUT_CONFIG_DIR)
    $ft = Get-Content -Raw (Join-Path $home1 'adopt\foxglove\module.toml')
    $bs = [string][char]92; $escaped = $fox.Replace($bs, $bs + $bs)
    Check 'foxglove manifest: exe filled, version from app/Cargo.toml, no placeholders' ($ft.Contains($escaped) -and $ft -notmatch '@EXE@|@VERSION@' -and $ft -match 'version = "\d+\.\d+\.\d+')
    Check 'vulpes manifest copied as given' ((Get-Content -Raw (Join-Path $home1 'adopt\vulpes\module.toml')) -match 'id = "vulpes"')
    Check '-Adopt: no pid file (hub not started)' (-not (Test-Path (Join-Path $home1 'hub.pid')))

    # already registered: skipped
    Set-Content (Join-Path $home1 'hub\inventory.json') '{"modules":{"vulpes":{},"foxglove":{}}}'
    Remove-Item $log
    $o = & $ps @common -Adopt 2>&1 | Out-String
    Check 'registered modules are skipped' ($LASTEXITCODE -eq 0 -and -not (Test-Path $log) -and $o -match 'already registered')

    # -Launch with the mock: starts exactly the hub (the mock), writes the pid file
    $home2 = Join-Path $tmp 'demo2'
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $mdir -HideoutExe $mock -FoxgloveExe $fox -DemoHome $home2 -Launch 2>&1 | Out-String
    Check '-Launch: starts the hub and records its pid' ($LASTEXITCODE -eq 0 -and $o -match 'started demo hub pid' -and (Test-Path (Join-Path $home2 'hub.pid')))

    # -Down -Clean: pid is not a hideout process, so nothing is stopped; folder deleted
    $o = & $ps -NoProfile -File $script_ -DemoHome $home2 -Down -Clean 2>&1 | Out-String
    Check '-Down: stops only a hideout process; -Clean deletes the demo folder' ($LASTEXITCODE -eq 0 -and $o -match 'nothing stopped' -and -not (Test-Path $home2))

    # -Rollback: dry by default for the restart; -Launch starts the exe with no arguments (the mock logs them)
    $home3 = Join-Path $tmp 'demo3'; New-Item -ItemType Directory $home3 | Out-Null
    $o = & $ps -NoProfile -File $script_ -DemoHome $home3 -Rollback -FoxgloveExe $mock 2>&1 | Out-String
    Check '-Rollback without -Launch starts nothing and keeps the folder' ($LASTEXITCODE -eq 0 -and $o -match 'ROLLBACK' -and $o -match 'dry run for the restart' -and (Test-Path $home3))
    Remove-Item $log -ErrorAction SilentlyContinue
    $o = & $ps -NoProfile -File $script_ -DemoHome $home3 -Rollback -Launch -FoxgloveExe $mock 2>&1 | Out-String
    Start-Sleep -Milliseconds 800
    Check '-Rollback -Launch starts standalone Foxglove without --module or demo env' ($o -match 'started standalone Foxglove' -and (Test-Path $log) -and ((Get-Content $log -Raw) -notmatch 'module' ) -and ((Get-Content $log -Raw) -match 'cfg=\s'))
}
finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
if ($fails) { Write-Host "$fails demo-up test(s) failed"; exit 1 }
Write-Host 'all demo-up tests passed'
