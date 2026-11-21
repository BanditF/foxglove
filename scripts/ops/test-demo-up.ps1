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
    $bs = [string][char]92
    $fixed1 = Join-Path $home1 'apps\foxglove\foxglove.exe'; $escaped = $fixed1.Replace($bs, $bs + $bs)
    Check 'foxglove manifest: FIXED app-folder exe filled (not the build path), version from app/Cargo.toml, no placeholders' ($ft.Contains($escaped) -and -not $ft.Contains($fox.Replace($bs, $bs + $bs)) -and $ft -notmatch '@EXE@|@VERSION@' -and $ft -match 'version = "\d+\.\d+\.\d+')
    Check '-Adopt: the build was copied into the fixed app folder' ((Test-Path $fixed1) -and ((Get-Content -Raw $fixed1) -eq (Get-Content -Raw $fox)))
    Check 'vulpes manifest copied as given' ((Get-Content -Raw (Join-Path $home1 'adopt\vulpes\module.toml')) -match 'id = "vulpes"')
    Check '-Adopt: no pid file (hub not started)' (-not (Test-Path (Join-Path $home1 'hub.pid')))

    # already registered: skipped
    function Register-Fake([string]$HubHome, [string]$FoxRun) {
        $m = Join-Path $HubHome 'modules\foxglove\0.3.0'; New-Item -ItemType Directory -Force $m | Out-Null
        Set-Content (Join-Path $m 'module.toml') ("id = `"foxglove`"`nversion = `"0.3.0`"`nrun = [`"" + $FoxRun.Replace($bs, $bs + $bs) + "`", `"--module`"]`n")
        Set-Content (Join-Path $HubHome 'inventory.json') '{"modules":{"vulpes":{"version":"0.1.0"},"foxglove":{"version":"0.3.0"}}}'
    }
    Register-Fake (Join-Path $home1 'hub') $fixed1
    Remove-Item $log
    $o = & $ps @common -Adopt 2>&1 | Out-String
    Check 'registered at the fixed path: skipped, no warning' ($LASTEXITCODE -eq 0 -and -not (Test-Path $log) -and $o -match 'already registered' -and $o -notmatch 'WARNING')

    # a rebuild: new bytes land in the SAME fixed path (no re-adopt needed, so the exe path never changes)
    $fox2 = Join-Path $tmp 'foxglove2.exe'; Set-Content $fox2 'newer build'
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $mdir -HideoutExe $mock -FoxgloveExe $fox2 -DemoHome $home1 -Adopt 2>&1 | Out-String
    Check 'rebuild: copied over the fixed exe, path unchanged, no re-adopt' ($LASTEXITCODE -eq 0 -and $o -match 'copied this build' -and ((Get-Content -Raw $fixed1) -match 'newer build') -and -not (Test-Path $log))
    # dry run of a rebuild writes nothing
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $mdir -HideoutExe $mock -FoxgloveExe $fox -DemoHome $home1 2>&1 | Out-String
    Check 'dry run of another build leaves the fixed exe alone' ($LASTEXITCODE -eq 0 -and ((Get-Content -Raw $fixed1) -match 'newer build'))

    # the trap: registered at an OLD path (a cargo target): reported and re-adopted at the fixed path (hub not running)
    $old = Join-Path $tmp 'target\debug\foxglove.exe'; New-Item -ItemType Directory -Force (Split-Path $old) | Out-Null; Set-Content $old 'old'
    Register-Fake (Join-Path $home1 'hub') $old
    $o = & $ps @common -Adopt 2>&1 | Out-String
    $l = @(Get-Content $log)
    Check 'moved path: WARNING names the old path and the fixed folder' ($o -match 'WARNING.*registered at .*target.*not the fixed folder')
    Check 'moved path: re-adopts foxglove only (one --adopt call), manifest points at the fixed exe' ($LASTEXITCODE -eq 0 -and $l.Count -eq 1 -and (Get-Content -Raw (Join-Path $home1 'adopt\foxglove\module.toml')).Contains($escaped))
    $o = & $ps @common 2>&1 | Out-String
    Check 'moved path, dry run: plans the re-adopt, writes nothing more' ($o -match 'plan\s+: foxglove\s+register' -and $o -match 're-adopt')
    # unreadable record counts as moved too (never silently trusted)
    Set-Content (Join-Path $home1 'hub\inventory.json') '{"modules":{"vulpes":{},"foxglove":{}}}'
    $o = & $ps @common 2>&1 | Out-String
    Check 'unreadable registered path is reported, not trusted' ($o -match 'WARNING.*\(unreadable\)')

    # the hub is running (a process named hideout): no re-adopt, exact manual step, exit 1
    $ping = (Get-Command ping.exe).Source; $fakeHub = Join-Path $tmp 'hideout.exe'; Copy-Item $ping $fakeHub
    $hp = Start-Process -FilePath $fakeHub -ArgumentList '-n', '30', '127.0.0.1' -PassThru -WindowStyle Hidden
    try {
        Set-Content (Join-Path $home1 'hub.pid') $hp.Id
        Register-Fake (Join-Path $home1 'hub') $old
        Remove-Item $log -ErrorAction SilentlyContinue
        $o = & $ps @common -Adopt 2>&1 | Out-String
        Check 'moved path + running hub: PROBLEM with the manual step, nothing adopted' ($LASTEXITCODE -eq 1 -and $o -match 'PROBLEM.*hub is running.*Manual step.*-Down' -and -not (Test-Path $log))
    } finally { Stop-Process -Id $hp.Id -ErrorAction SilentlyContinue; Remove-Item (Join-Path $home1 'hub.pid') -ErrorAction SilentlyContinue }

    # -Launch with the mock: starts exactly the hub (the mock), writes the pid file
    $home2 = Join-Path $tmp 'demo2'
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $mdir -HideoutExe $mock -FoxgloveExe $fox -DemoHome $home2 -Launch 2>&1 | Out-String
    Check '-Launch: starts the hub and records its pid' ($LASTEXITCODE -eq 0 -and $o -match 'started demo hub pid' -and (Test-Path (Join-Path $home2 'hub.pid')))

    # -SensorsExe: the PC sensors app's files are only placed next to the hideout.exe in use; nothing is adopted
    $sens = Join-Path $tmp 'sensors.exe'; Set-Content $sens 'sensors build'
    $home4 = Join-Path $tmp 'demo4'
    $sDir = Join-Path (Split-Path -Parent $mock) 'apps\sensors'
    $withS = @('-NoProfile', '-File', $script_, '-AreteModuleDir', $mdir, '-HideoutExe', $mock, '-FoxgloveExe', $fox, '-DemoHome', $home4, '-SensorsExe', $sens)
    $o = & $ps @withS 2>&1 | Out-String
    Check '-SensorsExe dry run: planned, nothing written' ($LASTEXITCODE -eq 0 -and $o -match 'plan\s+: sensors\s+place' -and -not (Test-Path $home4) -and -not (Test-Path $sDir))
    $o = & $ps -NoProfile -File $script_ -AreteModuleDir $mdir -HideoutExe $mock -FoxgloveExe $fox -DemoHome $home4 -SensorsExe (Join-Path $tmp 'no-sensors.exe') 2>&1 | Out-String
    Check 'missing sensors.exe is a PROBLEM' ($LASTEXITCODE -eq 1 -and $o -match 'PROBLEM.*-SensorsExe not found')
    Remove-Item $log -ErrorAction SilentlyContinue
    $o = & $ps @withS -Adopt 2>&1 | Out-String
    $l = @(Get-Content $log)
    Check '-SensorsExe -Adopt: only the two other apps are adopted, sensors is not' ($LASTEXITCODE -eq 0 -and $l.Count -eq 2 -and -not ($l | Where-Object { $_ -like '*sensors*' }))
    $sFile = Join-Path $sDir 'sensors.exe'; $sMan = Join-Path $sDir 'module.toml'
    Check '-SensorsExe: the build is placed in apps\sensors next to the hideout.exe' ((Test-Path $sFile) -and ((Get-Content -Raw $sFile) -match 'sensors build'))
    $repoMan = Join-Path (Split-Path (Split-Path $here)) 'crates\sensors\module.toml'
    Check 'sensors manifest: the repo file, byte for byte (kind = service, relative run)' ((Test-Path $sMan) -and ((Get-FileHash $sMan).Hash -eq (Get-FileHash $repoMan).Hash) -and ((Get-Content -Raw $sMan) -match '(?m)^kind = "service"'))
    Check 'sensors: nothing in the demo folder (no adopt record, no apps\sensors there)' (-not (Test-Path (Join-Path $home4 'adopt\sensors')) -and -not (Test-Path (Join-Path $home4 'apps\sensors')))
    $o = & $ps @withS -Adopt 2>&1 | Out-String
    Check 'sensors placed again: identical files are left alone' ($LASTEXITCODE -eq 0 -and $o -match 'already holds this build')
    $o = & $ps @common 2>&1 | Out-String
    Check 'no -SensorsExe: sensors is not planned' ($o -notmatch 'sensors')

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
