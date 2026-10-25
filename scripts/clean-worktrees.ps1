# Frees disk taken by Foxglove worktrees. Each one carries its own target\ and app\target\
# (1-2 GB each), so they add up fast. Runs on events, never on a timer: at the end of
# check.ps1, after a PR merges, and after a worktree is removed.
#   powershell -File scripts\clean-worktrees.ps1            # list what would go
#   powershell -File scripts\clean-worktrees.ps1 -Apply     # do it
# For a worktree that is clean, idle for -IdleHours and not in use:
#   - PR merged: remove the worktree (git worktree remove, never --force). A worktree with no PR
#     (a detached checkout, or a branch never pushed) counts as merged once its HEAD is on
#     origin/main or an origin/release/* branch: nothing in it is lost. Those wait -LandedHours
#     idle (a fresh worktree has no commits yet either, and its session may still be coming back).
#   - PR open:   delete only its target\ and app\target\; a review fix costs one rebuild.
# The main checkout is never removed; its target\ is cleaned once it passes -MainLimitGB.
# Never touches a dirty worktree, one with unpushed commits, the caller's own worktree, or -Skip.
param(
    [switch]$Apply,
    [int]$IdleHours = 2,
    [int]$LandedHours = 24,
    [int]$MainLimitGB = 8,
    # The Planner's merge worktrees, and On-call's sitting worktrees (0.1 re-check; drop them
    # from this list when On-call is done with them).
    [string[]]$Skip = @('fox-mm', 'fox-rm', 'fmain', 'lamp-lab', 'rel01')
)
$ErrorActionPreference = 'Stop'

$repo = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$main = [IO.Path]::GetFullPath(((git -C $repo rev-parse --path-format=absolute --git-common-dir) -replace '[\\/]\.git$', ''))
$prs = @{}
gh pr list --repo BanditF/foxglove --state all --limit 1000 --json headRefName,state -q '.[] | [.headRefName, .state] | @tsv' |
    ForEach-Object { $b, $s = $_ -split "`t", 2; if ($s -eq 'OPEN' -or -not $prs.ContainsKey($b)) { $prs[$b] = $s } }
if ($prs.Count -eq 0) { throw 'gh returned no PRs (offline or not signed in?)' }

# Command lines of running processes: a worktree named by a build, test or shell is in use.
$running = (Get-CimInstance Win32_Process | Where-Object CommandLine | ForEach-Object CommandLine) -join "`n"
$targets = 'target', 'app\target'

function Size($path) {
    $sum = 0
    foreach ($t in $targets) {
        $dir = Join-Path $path $t
        if (Test-Path $dir) { $sum += (Get-ChildItem $dir -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum }
    }
    $sum
}
function DropTargets($path) {
    foreach ($t in $targets) {
        $dir = Join-Path $path $t
        if (Test-Path $dir) { Remove-Item $dir -Recurse -Force }
    }
}
# Hours since the worktree was last committed to, checked out or built.
function IdleHours($path) {
    $stamps = @((Get-Item (Join-Path (git -C $path rev-parse --path-format=absolute --git-dir) 'index')).LastWriteTime)
    foreach ($t in $targets) {
        $debug = Join-Path $path "$t\debug"
        if (Test-Path $debug) { $stamps += (Get-Item $debug).LastWriteTime }
    }
    ((Get-Date) - ($stamps | Measure-Object -Maximum).Maximum).TotalHours
}

# Branches whose history is safe to drop a worktree for.
$landed = @(git -C $repo for-each-ref --format='%(refname)' refs/remotes/origin/main 'refs/remotes/origin/release/*')
function Landed($path) {
    $head = git -C $path rev-parse HEAD
    foreach ($ref in $landed) {
        git -C $repo merge-base --is-ancestor $head $ref
        if (-not $LASTEXITCODE) { return $true }
    }
    $false
}

$worktrees = @()
$current = $null
foreach ($line in git -C $repo worktree list --porcelain) {
    if ($line -like 'worktree *') { $current = @{ Path = [IO.Path]::GetFullPath($line.Substring(9)); Branch = $null }; $worktrees += $current }
    elseif ($line -like 'branch refs/heads/*') { $current.Branch = $line.Substring(18) }
}

$freed = 0
foreach ($w in $worktrees) {
    $path = $w.Path
    if ($path -eq $main -or $path -eq $repo) { continue }
    $state = if ($w.Branch) { $prs[$w.Branch] } else { $null }
    $landedOnly = $state -notin 'MERGED', 'OPEN' -and (Landed $path)
    if ($landedOnly) { $state = 'MERGED' }
    $why = $null
    if ((Split-Path $path -Leaf) -in $Skip) { $why = 'skipped' }
    elseif ($state -notin 'MERGED', 'OPEN') { $why = 'no open or merged PR' }
    # --no-optional-locks: a plain status refreshes the index, which would reset the idle clock.
    elseif (git --no-optional-locks -C $path status --porcelain) { $why = 'has changes' }
    elseif ($running -match [regex]::Escape($path)) { $why = 'in use' }
    else {
        $idle = IdleHours $path
        $limit = if ($landedOnly) { $LandedHours } else { $IdleHours }
        if ($idle -lt $limit) { $why = ('active {0:N1} h ago' -f $idle) }
    }
    $name = if ($w.Branch) { $w.Branch } else { "(detached) $path" }
    if ($why) { Write-Host "keep    $name ($why)"; continue }

    $size = Size $path
    if ($state -eq 'OPEN' -and $size -eq 0) { continue }
    $freed += $size
    $what = if ($state -eq 'MERGED') { 'remove ' } else { 'targets' }
    Write-Host ('{0} {1} ({2:N1} GB) {3}' -f $what, $name, ($size / 1GB), $path)
    if ($Apply) {
        DropTargets $path
        if ($state -eq 'MERGED') {
            git -C $repo worktree remove $path
            if ($LASTEXITCODE) { Write-Warning "git kept $path" }
        }
    }
}

# The main checkout builds for every merge and keeps stale copies of each crate. Only its debug
# output goes: the user runs app\target\release\foxglove.exe from there (RESCUE.md).
if ($running -notmatch [regex]::Escape($main)) {
    $debug = $targets | ForEach-Object { Join-Path $main "$_\debug" } | Where-Object { Test-Path $_ }
    $size = ($debug | ForEach-Object { Get-ChildItem $_ -Recurse -File -Force -ErrorAction SilentlyContinue } | Measure-Object Length -Sum).Sum
    if ($size -gt $MainLimitGB * 1GB) {
        $freed += $size
        Write-Host ('clean   main checkout debug builds ({0:N1} GB > {1} GB)' -f ($size / 1GB), $MainLimitGB)
        if ($Apply) { $debug | ForEach-Object { Remove-Item $_ -Recurse -Force } }
    }
}

if ($Apply) { git -C $repo worktree prune }
Write-Host ('{0} {1:N1} GB.' -f ($(if ($Apply) { 'Freed' } else { 'Would free' })), ($freed / 1GB))
