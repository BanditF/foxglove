# Removes Foxglove worktrees that are done with: their PR is merged, nothing is modified or
# untracked, nothing touched them for -IdleHours, and no running process names their folder.
# Each worktree holds its own target\ and app\target\ (2+ GB), so these add up fast.
#   powershell -File scripts\clean-worktrees.ps1            # list what would go
#   powershell -File scripts\clean-worktrees.ps1 -Apply     # remove it
# Never touches the main checkout, a dirty worktree, or one whose PR is open or missing.
param([switch]$Apply, [int]$IdleHours = 2)
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$main = (git -C $repo rev-parse --path-format=absolute --git-common-dir) -replace '[\\/]\.git$', ''
$merged = @{}
gh pr list --repo BanditF/foxglove --state merged --limit 1000 --json headRefName -q '.[].headRefName' |
    ForEach-Object { $merged[$_] = $true }
if ($merged.Count -eq 0) { throw 'gh returned no merged PRs (offline or not signed in?)' }

# Command lines of running processes, so a worktree in use by a build or a shell is kept.
$running = (Get-CimInstance Win32_Process | Where-Object CommandLine | ForEach-Object CommandLine) -join "`n"

$worktrees = @()
$current = $null
foreach ($line in git -C $repo worktree list --porcelain) {
    if ($line -like 'worktree *') { $current = @{ Path = $line.Substring(9); Branch = $null }; $worktrees += $current }
    elseif ($line -like 'branch refs/heads/*') { $current.Branch = $line.Substring(18) }
}

$freed = 0
foreach ($w in $worktrees) {
    $path = [IO.Path]::GetFullPath($w.Path)
    if ($path -eq [IO.Path]::GetFullPath($main)) { continue }
    $why = $null
    if (-not $w.Branch -or -not $merged.ContainsKey($w.Branch)) { $why = 'no merged PR' }
    # --no-optional-locks: a plain status refreshes the index, which would reset the idle clock.
    elseif (git --no-optional-locks -C $path status --porcelain) { $why = 'has changes' }
    elseif ($running -match [regex]::Escape($path)) { $why = 'in use' }
    else {
        $index = Join-Path (git -C $path rev-parse --path-format=absolute --git-dir) 'index'
        $age = ((Get-Date) - (Get-Item $index).LastWriteTime).TotalHours
        if ($age -lt $IdleHours) { $why = ('touched {0:N1} h ago' -f $age) }
    }
    if ($why) { Write-Host "keep   $($w.Branch) ($why)"; continue }

    $size = 0
    foreach ($t in 'target', 'app\target') {
        $dir = Join-Path $path $t
        if (Test-Path $dir) {
            $size += (Get-ChildItem $dir -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
        }
    }
    $freed += $size
    Write-Host ('remove {0} ({1:N1} GB) {2}' -f $w.Branch, ($size / 1GB), $path)
    if ($Apply) {
        foreach ($t in 'target', 'app\target') {
            $dir = Join-Path $path $t
            if (Test-Path $dir) { Remove-Item $dir -Recurse -Force }
        }
        git -C $repo worktree remove $path
        if ($LASTEXITCODE) { Write-Warning "git kept $path" }
    }
}
if ($Apply) { git -C $repo worktree prune }
Write-Host ('{0} {1:N1} GB.' -f ($(if ($Apply) { 'Freed' } else { 'Would free' })), ($freed / 1GB))
