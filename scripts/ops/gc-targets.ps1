# MIT. Garbage collection for build files (Hub 11/11): runs from the 'Foxglove Ops GC' task every
# 2 h, no LLM, no prompts. Deletes cargo targets in worktrees\_targets, nothing else, unless
# -RemoveWorktrees is given (a person asks for that).
#   powershell -File scripts\ops\gc-targets.ps1 [-DryRun] [-ListWorktrees] [-RemoveWorktrees]
# A target (<wt> or gate-<wt>) goes when ONE of:
#   1. its worktree is gone (no worktree or folder of that name),
#   2. its branch is merged or closed on GitHub (or a detached HEAD already on origin/main),
#   3. nothing has touched it for -StaleHours (24).
# Always kept: a target idle under -MinIdleMin (60), a target named by a running cargo/rustc
# (command line holds its path or its worktree's), and the names in scripts\protected-worktrees.txt.
# Then a size cap: over -CapGB (150) -> evict least recently used targets until under -TargetGB (120).
# Then a floor: C: free under -FloorFreeGB (100) -> the same eviction with a -FloorCapGB (60) cap.
# Logs every removal with its size to review\gc-targets.log.
# Worktree FOLDERS are never deleted unless -RemoveWorktrees: a registered git worktree whose branch
# is fully pushed or merged, with no uncommitted changes, idle -StaleHours, no open PR; removed with
# plain 'git worktree remove' (never --force). -ListWorktrees (and every normal run) writes the
# candidates, plus stale firmware-* / bw-* folders, to review\gc-worktree-candidates.md.
param(
    [string]$Repo = (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)),
    [string]$Root,                       # the folder that holds worktrees\ (default: next to the main checkout)
    [string]$TargetsDir,
    [string]$LogFile,
    [string]$ReportFile,
    [double]$CapGB = 150, [double]$TargetGB = 120, [double]$FloorFreeGB = 100, [double]$FloorCapGB = 60,
    [int]$StaleHours = 24, [int]$MinIdleMin = 60,
    [switch]$DryRun, [switch]$ListWorktrees, [switch]$RemoveWorktrees,
    # Test hooks: PR states as JSON {"branch":"MERGED"}, free GB, running command lines.
    [string]$PrStatesFile, [double]$FreeGB = -1, [string]$RunningText,
    [int]$TimeLimitSec = 1800
)
. (Join-Path $PSScriptRoot 'ops-common.ps1')

$main = [IO.Path]::GetFullPath(((git -C $Repo rev-parse --path-format=absolute --git-common-dir) -replace '[\\/]\.git$', ''))
if (-not $Root) { $Root = Split-Path $main -Parent }
if (-not $TargetsDir) { $TargetsDir = Join-Path $Root 'worktrees\_targets' }
if (-not $LogFile) { $LogFile = Join-Path $Root 'review\gc-targets.log' }
if (-not $ReportFile) { $ReportFile = Join-Path $Root 'review\gc-worktree-candidates.md' }

# One list of what no cleanup script may touch; clean-worktrees.ps1 reads it too.
function Get-Protected {
    $f = Join-Path $PSScriptRoot '..\protected-worktrees.txt'
    if (Test-Path $f) { @(Get-Content $f | ForEach-Object { $_.Trim() } | Where-Object { $_ -and $_ -notlike '#*' }) } else { @() }
}

function Write-GcLog([string]$m) {
    $d = Split-Path $LogFile -Parent; if (-not (Test-Path $d)) { New-Item -ItemType Directory -Force $d | Out-Null }
    Add-Content -Encoding UTF8 -Path $LogFile -Value ('{0} {1}' -f (Get-Date -Format s), $m)
}

# Directory size in bytes without walking it in PowerShell (a 9 GB target has ~100k files).
function Get-DirBytes([string]$Path) {
    $o = & robocopy $Path "$env:TEMP\gc-none" /L /S /NJH /NFL /NDL /NC /NP /XJ /R:0 /W:0 /BYTES 2>$null
    foreach ($l in $o) { if ($l -match '^\s*Bytes\s*:\s*(\d+)') { return [int64]$Matches[1] } }
    0
}

# Last time anything touched the target: the newest of the folder and its profile folders.
function Get-Touched([string]$Path) {
    $t = (Get-Item -LiteralPath $Path).LastWriteTime
    foreach ($s in 'debug', 'release', 'target\debug', 'target\release') {
        $p = Join-Path $Path $s
        if (Test-Path $p) { $x = (Get-Item -LiteralPath $p).LastWriteTime; if ($x -gt $t) { $t = $x } }
    }
    $t
}

function Remove-Target([string]$Path) {
    cmd /c rd /s /q "`"$Path`"" 2>$null | Out-Null
    -not (Test-Path -LiteralPath $Path)
}

Start-OpsJob -Name 'gc-targets' -TimeLimitSec $TimeLimitSec -SkipGameCheck

# --- worktrees (registered) and GitHub states ---
$wts = @(); $cur = $null
foreach ($line in (git -C $Repo worktree list --porcelain)) {
    if ($line -like 'worktree *') { $cur = @{ Path = [IO.Path]::GetFullPath($line.Substring(9)); Branch = $null; Head = $null }; $wts += $cur }
    elseif ($line -like 'branch refs/heads/*') { $cur.Branch = $line.Substring(18) }
    elseif ($line -like 'HEAD *') { $cur.Head = $line.Substring(5) }
}
$prs = @{}
if ($PrStatesFile) {
    (Get-Content -Raw $PrStatesFile | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $prs[$_.Name] = $_.Value }
} else {
    $r = Invoke-Bounded -File 'gh' -Arguments @('pr', 'list', '--repo', 'BanditF/foxglove', '--state', 'all', '--limit', '1000', '--json', 'headRefName,state', '-q', '.[] | [.headRefName, .state] | @tsv') -TimeoutSec 90
    if ($r.Code -eq 0 -and $r.Out) {
        foreach ($l in ($r.Out -split "`r?`n" | Where-Object { $_ })) {
            $b, $s = $l -split "`t", 2
            if ($s -eq 'OPEN' -or -not $prs.ContainsKey($b)) { $prs[$b] = $s }
        }
    } else { Write-GcLog 'warning: gh gave no PR list; branch rules skipped, age and size rules still run' }
}
function Test-OnMain([string]$Path) {
    git -C $Repo merge-base --is-ancestor (git -C $Path rev-parse HEAD) origin/main 2>$null
    $LASTEXITCODE -eq 0
}
$running = if ($PSBoundParameters.ContainsKey('RunningText')) { $RunningText }
           else { ((Get-CimInstance Win32_Process -Filter "Name='cargo.exe' OR Name='rustc.exe'" | Where-Object CommandLine | ForEach-Object CommandLine) -join "`n") }
$protected = Get-Protected
$byLeaf = @{}; foreach ($w in $wts) { $byLeaf[(Split-Path $w.Path -Leaf)] = $w }

# --- 1. per-target rules ---
$freed = 0L; $removed = 0
$left = @()
if (Test-Path $TargetsDir) {
    foreach ($d in (Get-ChildItem -LiteralPath $TargetsDir -Directory -Force)) {
        $name = $d.Name; $wtName = $name -replace '^gate-', ''
        $why = $null
        $idleMin = ((Get-Date) - (Get-Touched $d.FullName)).TotalMinutes
        $w = $byLeaf[$wtName]; if (-not $w) { $w = $byLeaf[$name] }
        $folder = Test-Path (Join-Path $Root "worktrees\$wtName")
        if ($name -in $protected -or $wtName -in $protected) { $left += $d; continue }
        $inUse = $running -and (($running -like "*$($d.FullName)*") -or ($w -and $running -like "*$($w.Path)*") -or ($folder -and $running -like "*worktrees\$wtName*"))
        if ($inUse -or $idleMin -lt $MinIdleMin) { $left += $d; continue }
        if (-not $w -and -not $folder) { $why = 'worktree gone' }
        elseif ($w -and $w.Branch -and $prs[$w.Branch] -in 'MERGED', 'CLOSED') { $why = "branch $($prs[$w.Branch].ToLower())" }
        elseif ($w -and -not $w.Branch -and (Test-Path $w.Path) -and (Test-OnMain $w.Path)) { $why = 'detached HEAD on main' }
        elseif ($idleMin -ge $StaleHours * 60) { $why = ('untouched {0:N0} h' -f ($idleMin / 60)) }
        if (-not $why) { $left += $d; continue }
        $sz = Get-DirBytes $d.FullName
        if ($DryRun) { Write-GcLog ("would remove {0} ({1:N1} GB): {2}" -f $name, ($sz / 1GB), $why); $left += $d; continue }
        if (Remove-Target $d.FullName) { $freed += $sz; $removed++; Write-GcLog ("removed {0} ({1:N1} GB): {2}" -f $name, ($sz / 1GB), $why) }
        else { Write-GcLog "could not remove $name (a file is open?)"; $left += $d }
    }
}

# --- 2 + 3. size cap, then the free-space floor ---
function Invoke-Evict([double]$CapGBv, [double]$ToGBv, [string]$Reason) {
    $rows = @($left | Where-Object { Test-Path -LiteralPath $_.FullName } | ForEach-Object {
        [pscustomobject]@{ D = $_; Bytes = (Get-DirBytes $_.FullName); Touched = (Get-Touched $_.FullName) } })
    $total = ($rows | Measure-Object Bytes -Sum).Sum
    if ($total -le $CapGBv * 1GB) { return }
    foreach ($r in ($rows | Sort-Object Touched)) {
        if ($total -le $ToGBv * 1GB) { break }
        $n = $r.D.Name; $wn = $n -replace '^gate-', ''
        if ($n -in $protected -or $wn -in $protected) { continue }
        $w = $byLeaf[$wn]
        if ($running -and (($running -like "*$($r.D.FullName)*") -or ($w -and $running -like "*$($w.Path)*"))) { continue }
        if ($DryRun) { Write-GcLog ("would evict {0} ({1:N1} GB): {2}" -f $n, ($r.Bytes / 1GB), $Reason); $total -= $r.Bytes; continue }
        if (Remove-Target $r.D.FullName) {
            $total -= $r.Bytes; $script:freed += $r.Bytes; $script:removed++
            Write-GcLog ("evicted {0} ({1:N1} GB): {2}" -f $n, ($r.Bytes / 1GB), $Reason)
        }
    }
    $script:left = @($script:left | Where-Object { Test-Path -LiteralPath $_.FullName })
}
if (Test-Path $TargetsDir) {
    Invoke-Evict $CapGB $TargetGB "over the $CapGB GB cap (LRU)"
    $free = if ($FreeGB -ge 0) { $FreeGB } else { (Get-PSDrive -Name ((Split-Path $TargetsDir -Qualifier).TrimEnd(':'))).Free / 1GB }
    if ($free -lt $FloorFreeGB) {
        Write-GcLog ('C: free {0:N0} GB is under the {1} GB floor: cap {2} GB' -f $free, $FloorFreeGB, $FloorCapGB)
        Invoke-Evict $FloorCapGB $FloorCapGB "free space under $FloorFreeGB GB (LRU, $FloorCapGB GB cap)"
    }
}

# --- 4. worktree folders: candidates, and removal only when asked ---
$cand = @(); $notes = @()
foreach ($w in $wts) {
    $leaf = Split-Path $w.Path -Leaf
    if ($w.Path -eq $main -or $w.Path -eq [IO.Path]::GetFullPath($Repo) -or $leaf -in $protected) { continue }
    $state = if ($w.Branch) { $prs[$w.Branch] } else { $null }
    if ($state -eq 'OPEN') { continue }
    if (-not (Test-Path $w.Path)) { continue }
    if (git --no-optional-locks -C $w.Path status --porcelain) { continue }              # uncommitted work
    $pushed = [bool](git -C $w.Path branch -r --contains $w.Head 2>$null)
    if (-not ($pushed -or $state -eq 'MERGED' -or (Test-OnMain $w.Path))) { continue }   # unpushed commits
    $idle = ((Get-Date) - (Get-Item (Join-Path (git -C $w.Path rev-parse --path-format=absolute --git-dir) 'index')).LastWriteTime).TotalHours
    if ($idle -lt $StaleHours) { continue }
    if ($running -like "*$($w.Path)*") { continue }
    $cand += [pscustomobject]@{ Path = $w.Path; Branch = $(if ($w.Branch) { $w.Branch } else { '(detached)' }); IdleH = [math]::Round($idle); State = $(if ($state) { $state } else { 'no PR' }) }
}
$wtRoot = Join-Path $Root 'worktrees'
$regPaths = $wts | ForEach-Object { $_.Path.TrimEnd('\') }
$orphans = @()
if (Test-Path $wtRoot) {
    $orphans = @(Get-ChildItem -LiteralPath $wtRoot -Directory -Force | Where-Object { $_.Name -notlike '_*' -and $_.Name -notlike '.*' -and $_.Name -notin $protected -and ([IO.Path]::GetFullPath($_.FullName).TrimEnd('\') -notin $regPaths) })
}
$fw = @(Get-ChildItem -LiteralPath $Root -Directory -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'firmware-*' -and $_.LastWriteTime -lt (Get-Date).AddDays(-14) })
$md = @("# GC worktree candidates ($(Get-Date -Format s))", '', 'Nothing here is deleted by the scheduled run. Registered worktrees qualify when the branch is pushed or merged, the tree is clean, nothing is unpushed, no open PR, idle over a day.', '')
$md += '## Git worktrees that qualify (remove with -RemoveWorktrees)'
$md += if ($cand) { $cand | ForEach-Object { "- $($_.Path) [$($_.Branch)] $($_.State), idle $($_.IdleH) h" } } else { '- none' }
$md += '', '## Folders in worktrees\ that are not registered git worktrees (check by hand; the script never deletes these)'
$md += if ($orphans) { $orphans | ForEach-Object { "- $($_.Name) (last written $($_.LastWriteTime.ToString('yyyy-MM-dd')))" } } else { '- none' }
$md += '', '## firmware-* folders untouched for 14 days'
$md += if ($fw) { $fw | ForEach-Object { "- $($_.Name) (last written $($_.LastWriteTime.ToString('yyyy-MM-dd')))" } } else { '- none' }
$rd = Split-Path $ReportFile -Parent; if (-not (Test-Path $rd)) { New-Item -ItemType Directory -Force $rd | Out-Null }
Set-Content -Encoding UTF8 -Path $ReportFile -Value $md

$wtRemoved = 0
if ($RemoveWorktrees -and -not $DryRun) {
    foreach ($c in $cand) {
        git -C $Repo worktree remove $c.Path 2>$null
        if ($LASTEXITCODE -eq 0) { $wtRemoved++; Write-GcLog "removed worktree $($c.Path) [$($c.Branch)]" } else { Write-GcLog "git kept worktree $($c.Path)" }
    }
}
Stop-OpsJob -Status 'ok' -Text ('removed {0} targets, freed {1:N1} GB; {2} worktrees qualify, {3} unregistered folders{4}' -f $removed, ($freed / 1GB), $cand.Count, $orphans.Count, $(if ($DryRun) { ' (dry run)' } else { '' }))
