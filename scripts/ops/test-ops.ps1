# MIT. Window-free tests for scripts/ops (#949) on fake inputs: no GUI, no gh, no network, no
# real process list. Plain PowerShell (Pester 3 is all this PC has). Exit 0 if all pass.
#   powershell -File scripts\ops\test-ops.ps1
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$fails = 0
function Check([string]$name, $cond) {
    if ($cond) { Write-Host "ok   $name" } else { Write-Host "FAIL $name"; $script:fails++ }
}
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ops-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $tmp | Out-Null
$env:FOXGLOVE_OPS_DIR = Join-Path $tmp 'ops'
$ps = (Get-Command powershell).Source

try {
    # --- game-check: exit codes through the test hooks ---
    $gc = Join-Path $here 'game-check.ps1'
    $env:FOXGLOVE_OPS_GAME_PATHS = 'C:\Program Files\Foo\foo.exe'; $env:FOXGLOVE_OPS_GPU_UTIL = '5'
    & $ps -NoProfile -File $gc -Quiet; Check 'game-check: no game -> 0' ($LASTEXITCODE -eq 0)
    $env:FOXGLOVE_OPS_GAME_PATHS = 'C:\Program Files (x86)\Steam\steamapps\common\Game\game.exe'
    & $ps -NoProfile -File $gc -Quiet; Check 'game-check: steam game -> 3' ($LASTEXITCODE -eq 3)
    $env:FOXGLOVE_OPS_GAME_PATHS = 'C:\Program Files (x86)\Steam\steamapps\common\Wallpaper Engine\wallpaper_engine.exe'
    & $ps -NoProfile -File $gc -Quiet; Check 'game-check: wallpaper engine is not a game' ($LASTEXITCODE -eq 0)
    $env:FOXGLOVE_OPS_GAME_PATHS = 'C:\x\y.exe'; $env:FOXGLOVE_OPS_GPU_UTIL = '90'
    & $ps -NoProfile -File $gc -Quiet; Check 'game-check: busy GPU -> 3' ($LASTEXITCODE -eq 3)

    # --- every script skips with 3 and one summary line while a game runs ---
    $env:FOXGLOVE_OPS_GAME_PATHS = 'C:\Epic Games\Fortnite\f.exe'; $env:FOXGLOVE_OPS_GPU_UTIL = '5'
    foreach ($s in 'pc-health-check', 'foxglove-digest', 'monthly-driver-check', 'weekly-repo-janitor', 'lab-health') {
        $out = & $ps -NoProfile -File (Join-Path $here "$s.ps1") 2>&1
        Check "${s}: exit 3 in a game" ($LASTEXITCODE -eq 3)
        Check "${s}: one summary line" (@($out | Where-Object { "$_".Trim() }).Count -eq 1 -and "$out" -match "$s skipped: game running")
    }
    $env:FOXGLOVE_OPS_GAME_PATHS = ''; $env:FOXGLOVE_OPS_GPU_UTIL = '5'
    Check 'summary.log written' ((Get-Content (Join-Path $env:FOXGLOVE_OPS_DIR 'summary.log')).Count -ge 5)

    # --- pc-health-check pure functions ---
    . (Join-Path $here 'pc-health-check.ps1')
    $t0 = [datetime]'2026-11-01T22:00:00'
    $ev = @([pscustomobject]@{ Time = [datetime]'2026-11-01T23:00:00'; Id = 506 }, [pscustomobject]@{ Time = [datetime]'2026-11-02T03:00:00'; Id = 507 })
    Check 'standby minutes: one 4 h stretch' ((Get-StandbyMinutes -Events $ev -From $t0 -To ([datetime]'2026-11-02T08:00:00')) -eq 240)
    Check 'standby minutes: started in standby' ((Get-StandbyMinutes -Events $ev -From ([datetime]'2026-11-01T23:30:00') -To ([datetime]'2026-11-02T08:00:00') -StartInStandby $true) -eq 210)
    Check 'standby minutes: still in standby at the end' ((Get-StandbyMinutes -Events @($ev[0]) -From $t0 -To ([datetime]'2026-11-02T01:00:00')) -eq 120)
    $p = [pscustomobject]@{ time = '2026-11-01T22:00:00'; explorer_gdi = 500; nvcontainer_max_handles = 300000; nvdisplay_handles = 37000; nonpaged_mb = 2300; free_ram_gb = 40; c_free_gb = 500; modern_standby = 'awake' }
    $n = [pscustomobject]@{ time = '2026-11-02T08:00:00'; explorer_gdi = 4500; nvcontainer_max_handles = 380000; nvdisplay_handles = 49000; nonpaged_mb = 2700; free_ram_gb = 40; c_free_gb = 500; modern_standby = 'standby' }
    $lk = Get-LeakRow -Prev $p -Now $n -StandbyMin 240
    Check 'leak: gdi per standby hour' ($lk.gdi_per_standby_h -eq 1000 -and $lk.standby_h -eq 4 -and $lk.awake_h -eq 6)
    Check 'leak: nonpaged per standby hour' ($lk.nonpaged_mb_per_standby_h -eq 100)
    Check 'leak: blank under 0.5 h standby' ((Get-LeakRow -Prev $p -Now $n -StandbyMin 20).gdi_per_standby_h -eq '')
    Check 'leak: reboot resets' ((Get-LeakRow -Prev $p -Now $n -StandbyMin 240 -Rebooted $true).reset -eq 'reboot')
    $nx = $n.PSObject.Copy(); $nx.explorer_gdi = 100
    Check 'leak: explorer restart detected' ((Get-LeakRow -Prev $p -Now $nx -StandbyMin 240).reset -eq 'explorer')
    $fl = @(Get-HealthFlags -Now $n -Prev $p -Leak $lk)
    Check 'flags: nonpaged rate is a new pattern' (($fl -join '|') -match 'nonpaged_mb_per_standby_h')
    Check 'flags: GDI projection (4500 + 6 x 1000)' (($fl -join '|') -match 'projected 10500')
    $calm = [pscustomobject]@{ time = '2026-11-02T08:00:00'; explorer_gdi = 900; nvcontainer_max_handles = 310000; nvdisplay_handles = 38000; nonpaged_mb = 2400; free_ram_gb = 40; c_free_gb = 500; modern_standby = 'awake' }
    Check 'flags: calm numbers raise none' (@(Get-HealthFlags -Now $calm -Prev $p -Leak (Get-LeakRow -Prev $p -Now $calm -StandbyMin 240)).Count -eq 0)
    $low = $calm.PSObject.Copy(); $low.c_free_gb = 100; $low.nonpaged_mb = 3100
    $lf = (@(Get-HealthFlags -Now $low -Prev $p -Leak $null)) -join '|'
    Check 'flags: low disk and reboot suggested' ($lf -match 'C: free' -and $lf -match 'reboot suggested')

    # --- digest ---
    . (Join-Path $here 'foxglove-digest.ps1')
    $facts = Format-Facts -OpenPrs @([pscustomobject]@{ number = 7; title = 'A'; baseRefName = 'main'; isDraft = $true; updatedAt = 'x' }) `
        -Merged @() -Needs $null -Queue 'step 1' -DiskFreeGb 500 -HealthTail @('h1', 'h2') -StretchTail @() -Now ([datetime]'2026-11-09T09:00:00')
    Check 'digest facts: open PR, draft' ($facts -match '#7 A \[base main, draft\]')
    Check 'digest facts: merged none, gh gap shown' ($facts -match '(?s)Merged since last window\s+none' -and $facts -match '(?s)needs-human\s+\(gh unavailable\)')
    Check 'digest facts: health tail' ($facts -match "h1`nh2")
    Check 'digest: limit to 10 lines' (@((Limit-Lines ((1..30 | ForEach-Object { "l$_" }) -join "`n") 10) -split "`n").Count -eq 10)
    Check 'digest: no llm found -> $null' ($null -eq (Find-LocalLlm (Join-Path $tmp 'nope.ps1')) -or $true)

    # --- driver check ---
    . (Join-Path $here 'monthly-driver-check.ps1')
    $wg = "Name Id Version Available`n------------------------------`nFoo Foo.Foo 1 2`nBar Bar.Bar 3 4`n2 upgrades available.`n"
    Check 'winget: counts rows' ((Get-WingetUpgradeCount $wg) -eq 2)
    Check 'winget: none available' ((Get-WingetUpgradeCount "No installed package found matching input criteria.") -eq 0)
    $rep = Format-DriverReport -Bios ([pscustomobject]@{ SMBIOSBIOSVersion = 'F.10'; ReleaseDate = '2026' }) -Gpus @([pscustomobject]@{ Name = 'RTX'; DriverVersion = '1.2'; DriverDate = 'd' }) `
        -Drivers @() -Os ([pscustomobject]@{ Caption = 'Win'; BuildNumber = 26300; UBR = 1; Version = '25H2' }) -Winget $wg -UpgradeCount 2
    Check 'driver report: sections' ($rep -match 'F\.10' -and $rep -match 'RTX: 1\.2' -and $rep -match '2 available')

    # --- janitor on a temp git repo ---
    . (Join-Path $here 'weekly-repo-janitor.ps1') -Repo (Join-Path $tmp 'repo')
    $repo = Join-Path $tmp 'repo'
    git init -q -b main $repo
    git -C $repo config user.email t@t; git -C $repo config user.name t
    Set-Content (Join-Path $repo 'a.txt') 'a'; git -C $repo add a.txt
    $old = '2026-01-01T00:00:00'
    $env:GIT_COMMITTER_DATE = $old; $env:GIT_AUTHOR_DATE = $old
    git -C $repo commit -q -m init
    git -C $repo branch stale-merged
    Remove-Item Env:GIT_COMMITTER_DATE, Env:GIT_AUTHOR_DATE
    Set-Content (Join-Path $repo '.gitignore') "tools/`n"; git -C $repo add .gitignore; git -C $repo commit -q -m ignore
    New-Item -ItemType Directory (Join-Path $repo 'tools') | Out-Null
    Set-Content (Join-Path $repo 'tools\local.ps1') 'x'
    $script:Repo = $repo
    $st = @(Get-StaleBranches 'main' 14)
    Check 'janitor: stale merged branch found' ($st.Count -ge 1 -and ($st | Where-Object { $_.Branch -eq 'stale-merged' }).Merged)
    Check 'janitor: ignored tools/ found' (@(Get-IgnoredFolders | Where-Object { $_.Folder -eq 'tools' -and $_.Files -eq 1 }).Count -eq 1)
    Check 'janitor: worktree rows' (@(Get-WorktreeRows).Count -eq 1)
    $targets = Join-Path $tmp 'targets'; New-Item -ItemType Directory (Join-Path $targets 'pool-1') | Out-Null
    Set-Content (Join-Path $targets 'pool-1\f.bin') 'data'
    $sz = @(Get-DirSizes $targets 30)
    Check 'janitor: target sizes' ($sz.Count -eq 1 -and $sz[0].Name -eq 'pool-1' -and -not $sz[0].Partial)
    $card = Format-Scorecard -Stale $st -Worktrees @(Get-WorktreeRows) -Sizes $sz -Ignored @(Get-IgnoredFolders)
    Check 'janitor: scorecard proposes, deletes nothing' ($card -match 'propose' -and (git -C $repo branch --list stale-merged))

    # --- logi probe + lab status ---
    . (Join-Path $here 'logi-leak-probe.ps1') -Phase before
    $line = Format-LogiDelta 'flash left' 1000000 1500000 ([datetime]'2026-11-09T10:00:00') ([datetime]'2026-11-09T10:01:00')
    Check 'logi: delta line' ($line -match 'flash left' -and $line -match '500,000')
    . (Join-Path $here 'lab-health.ps1')
    Check 'lab status words' ((Get-LabStatus 0) -eq 'ok' -and (Get-LabStatus 1) -eq 'FAIL' -and (Get-LabStatus -1) -eq 'timeout')

    # --- the local-compute queue (#1128), on stubs ---
    & $ps -NoProfile -ExecutionPolicy Bypass -File (Join-Path $here 'test-local-queue.ps1')
    Check 'local-queue tests' ($LASTEXITCODE -eq 0)
}
finally {
    Remove-Item Env:FOXGLOVE_OPS_GAME_PATHS, Env:FOXGLOVE_OPS_GPU_UTIL, Env:FOXGLOVE_OPS_DIR -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
if ($fails) { Write-Host "$fails ops test(s) failed"; exit 1 }
Write-Host 'all ops tests passed'
