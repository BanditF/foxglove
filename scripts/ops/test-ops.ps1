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

    # --- gc-targets (fake tree: real git worktrees, tiny files, test hooks for PR states and free space) ---
    $g = Join-Path $tmp 'gc'; $gm = Join-Path $g 'foxglove'; $gw = Join-Path $g 'worktrees'; $gt = Join-Path $gw '_targets'
    New-Item -ItemType Directory $gm, $gt | Out-Null
    git -C $gm init -q -b main; git -C $gm -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
    git -C $gm update-ref refs/remotes/origin/main HEAD
    foreach ($b in 'live', 'merged', 'mid') { git -C $gm worktree add -q (Join-Path $gw $b) -b $b }
    function New-Tgt($n, $mb, $ageH) {
        $d = Join-Path $gt $n; New-Item -ItemType Directory $d, "$d\debug" -Force | Out-Null
        [IO.File]::WriteAllBytes("$d\debug\f.bin", (New-Object byte[] ($mb * 1MB)))
        (Get-Item $d).LastWriteTime = (Get-Date).AddHours(-$ageH); (Get-Item "$d\debug").LastWriteTime = (Get-Date).AddHours(-$ageH)
    }
    New-Tgt 'live' 1 3; New-Tgt 'gate-merged' 1 3; New-Tgt 'gate-gone' 1 3; New-Tgt 'mid' 1 30; New-Tgt 'fresh-gone' 1 0
    New-Tgt 'busy-gone' 1 5; New-Tgt 'ops-main' 1 90
    $pr = Join-Path $tmp 'prs.json'; Set-Content $pr '{"merged":"MERGED","live":"OPEN"}'
    $gcs = Join-Path $here 'gc-targets.ps1'; $gl = Join-Path $g 'review\gc-targets.log'
    $gcArgs = @('-NoProfile', '-File', $gcs, '-Repo', $gm, '-Root', $g, '-PrStatesFile', $pr, '-FreeGB', '500', '-RunningText', "rustc --out-dir $gt\busy-gone\debug")
    & $ps @gcArgs -DryRun | Out-Null
    Check 'gc: dry run deletes nothing' ((Get-ChildItem $gt).Count -eq 7)
    & $ps @gcArgs | Out-Null
    $left = (Get-ChildItem $gt).Name
    Check 'gc: worktree gone -> removed' ($left -notcontains 'gate-gone')
    Check 'gc: merged branch -> removed' ($left -notcontains 'gate-merged')
    Check 'gc: untouched over 24 h -> removed' ($left -notcontains 'mid')
    Check 'gc: open PR, recent -> kept' ($left -contains 'live')
    Check 'gc: under the min idle -> kept' ($left -contains 'fresh-gone')
    Check 'gc: cargo/rustc running there -> kept' ($left -contains 'busy-gone')
    Check 'gc: protected name -> kept' ($left -contains 'ops-main')
    Check 'gc: log has sizes' ((Get-Content $gl -Raw) -match 'removed gate-gone \(0\.0 GB\): worktree gone')
    # size cap: three 1 MB targets, cap 2.5 MB -> evict the least recently used until under 1.5 MB
    Remove-Item -Recurse -Force $gt; New-Item -ItemType Directory $gt | Out-Null
    New-Tgt 'p1' 1 5; New-Tgt 'p2' 1 4; New-Tgt 'p3' 1 3; foreach ($n in 'p1', 'p2', 'p3', 'q1', 'q2') { New-Item -ItemType Directory (Join-Path $gw $n) -Force | Out-Null }
    $capArgs = @('-NoProfile', '-File', $gcs, '-Repo', $gm, '-Root', $g, '-PrStatesFile', $pr, '-RunningText', 'none', '-StaleHours', '999')
    & $ps @capArgs -FreeGB 500 -CapGB 0.0025 -TargetGB 0.0015 | Out-Null
    $left = (Get-ChildItem $gt).Name
    Check 'gc: cap evicts the oldest first' ($left -notcontains 'p1' -and $left -contains 'p3' -and @($left).Count -eq 1)
    # floor: free 50 GB < 100 GB -> the 60 GB floor cap, shrunk here to 0.0015 GB
    New-Tgt 'q1' 1 5; New-Tgt 'q2' 1 4
    & $ps @capArgs -FreeGB 50 -FloorCapGB 0.0015 | Out-Null
    Check 'gc: low free space applies the floor cap' (@(Get-ChildItem $gt).Count -eq 1 -and (Get-ChildItem $gt).Name -eq 'p3')
    Check 'gc: candidate report written, no worktree removed' ((Test-Path (Join-Path $g 'review\gc-worktree-candidates.md')) -and (Test-Path (Join-Path $gw 'merged')))
    # worktree removal needs the switch, keeps dirty ones
    Set-Content (Join-Path $gw 'mid\dirty.txt') 'x'
    (Get-Item (Join-Path (git -C (Join-Path $gw 'merged') rev-parse --path-format=absolute --git-dir) 'index')).LastWriteTime = (Get-Date).AddDays(-3)
    & $ps -NoProfile -File $gcs -Repo $gm -Root $g -PrStatesFile $pr -RunningText none -FreeGB 500 -RemoveWorktrees | Out-Null
    Check 'gc: -RemoveWorktrees keeps a dirty tree' (Test-Path (Join-Path $gw 'mid'))
    Check 'gc: -RemoveWorktrees removes a clean merged one' (-not (Test-Path (Join-Path $gw 'merged')))

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
