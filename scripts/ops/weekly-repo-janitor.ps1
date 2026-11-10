# MIT. Weekly repo janitor as a script (#949): a scorecard of stale branches, worktrees, the cargo
# target pool sizes and git-ignored script folders (tools/ and the like). PROPOSALS ONLY: it
# deletes, merges and opens nothing. Output: ops\janitor-<yyyy-mm-dd>.md.
#   powershell -File scripts\ops\weekly-repo-janitor.ps1 [-Repo <checkout>] [-StaleDays 14] [-TimeLimitSec 600]
param(
    [string]$Repo = (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)),
    [int]$StaleDays = 14,
    [string]$TargetsDir,
    [string]$OutFile,
    [int]$TimeLimitSec = 600,
    [int]$SizeBudgetSec = 240
)
. (Join-Path $PSScriptRoot 'ops-common.ps1')

function Invoke-Git([string[]]$GitArgs) {
    $r = Invoke-Bounded -File 'git' -Arguments (@('-C', $Repo) + $GitArgs) -TimeoutSec 60
    if ($r.Code -ne 0) { return @() }
    @($r.Out -split "`r?`n" | Where-Object { $_ })
}

# Local branches with no commit for $StaleDays days; "merged" when contained in $Base.
function Get-StaleBranches([string]$Base, [int]$Days) {
    $merged = @(Invoke-Git @('branch', '--merged', $Base, '--format=%(refname:short)'))
    foreach ($l in (Invoke-Git @('for-each-ref', 'refs/heads', '--format=%(refname:short)|%(committerdate:iso-strict)'))) {
        $name, $date = $l -split '\|', 2
        $age = ((Get-Date) - [datetime]$date).TotalDays
        if ($age -ge $Days -and $name -ne ($Base -replace '^origin/', '')) {
            [pscustomobject]@{ Branch = $name; AgeDays = [int]$age; Merged = ($merged -contains $name) }
        }
    }
}

# Worktrees from `git worktree list --porcelain`, flagging missing directories and prunable ones.
function Get-WorktreeRows {
    $rows = @(); $cur = $null
    foreach ($l in ((Invoke-Git @('worktree', 'list', '--porcelain')) + 'worktree <end>')) {
        if ($l -match '^worktree (.+)$') {
            if ($cur) { $cur.Missing = -not (Test-Path $cur.Path); $rows += [pscustomobject]$cur }
            $cur = [ordered]@{ Path = $Matches[1]; Branch = ''; Prunable = $false }
        }
        elseif ($cur -and $l -match '^branch refs/heads/(.+)$') { $cur.Branch = $Matches[1] }
        elseif ($cur -and $l -match '^prunable') { $cur.Prunable = $true }
    }
    $rows
}

# Size in GB of each direct subfolder of $Dir, within a time budget (partial results are marked).
function Get-DirSizes([string]$Dir, [int]$BudgetSec) {
    if (-not $Dir -or -not (Test-Path $Dir)) { return @() }
    $deadline = (Get-Date).AddSeconds($BudgetSec)
    foreach ($d in (Get-ChildItem -LiteralPath $Dir -Directory -ErrorAction SilentlyContinue)) {
        if ((Get-Date) -gt $deadline) { [pscustomobject]@{ Name = $d.Name; GB = $null; Partial = $true }; continue }
        $sum = (Get-ChildItem -LiteralPath $d.FullName -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
        [pscustomobject]@{ Name = $d.Name; GB = [math]::Round($sum / 1GB, 2); Partial = $false }
    }
}

# Top-level git-ignored folders (untracked by design, so not backed up by the repo): "!! tools/".
function Get-IgnoredFolders {
    foreach ($l in (Invoke-Git @('status', '--ignored', '--porcelain=v1'))) {
        if ($l -match '^!! (.+)/$') {
            $p = Join-Path $Repo $Matches[1]
            if ($Matches[1] -match '^(target|node_modules|\.venv|__pycache__)$|/target$') { continue }
            $files = @(Get-ChildItem -LiteralPath $p -Recurse -File -Force -ErrorAction SilentlyContinue)
            [pscustomobject]@{ Folder = $Matches[1]; Files = $files.Count; MB = [math]::Round(($files | Measure-Object Length -Sum).Sum / 1MB, 1) }
        }
    }
}

function Format-Scorecard {
    param($Stale, $Worktrees, $Sizes, $Ignored, [datetime]$Now = (Get-Date))
    $o = New-Object System.Collections.Generic.List[string]
    $o.Add("# Repo janitor $($Now.ToString('yyyy-MM-dd')) (proposals only)")
    $o.Add('')
    $o.Add("## Stale branches ($(@($Stale).Count))")
    foreach ($b in $Stale) { $o.Add("- $($b.Branch): $($b.AgeDays) d$(if ($b.Merged) { ', merged: propose `git branch -d`' } else { ', NOT merged: ask the owner' })") }
    $o.Add('')
    $o.Add("## Worktrees ($(@($Worktrees).Count))")
    foreach ($w in $Worktrees) { $o.Add("- $($w.Path) [$($w.Branch)]$(if ($w.Missing -or $w.Prunable) { ': directory missing, propose `git worktree prune`' })") }
    $o.Add('')
    $o.Add('## Target pool sizes')
    foreach ($s in $Sizes) { $o.Add("- $($s.Name): $(if ($s.Partial) { 'not measured (time budget)' } else { "$($s.GB) GB" })") }
    $tot = (@($Sizes) | Where-Object { -not $_.Partial } | Measure-Object GB -Sum).Sum
    $o.Add("- total measured: $([math]::Round([double]$tot, 2)) GB")
    $o.Add('')
    $o.Add('## Git-ignored folders (exist only on this PC, not in the repo)')
    foreach ($i in $Ignored) { $o.Add("- $($i.Folder)/: $($i.Files) files, $($i.MB) MB: propose a copy under scripts/ if it is reusable, else leave") }
    $o -join "`n"
}

if ($MyInvocation.InvocationName -ne '.') {
    Start-OpsJob -Name 'weekly-repo-janitor' -TimeLimitSec $TimeLimitSec
    if (-not (Test-Path (Join-Path $Repo '.git'))) { Stop-OpsJob -Status 'warn' -Text "$Repo is not a git checkout" -Code 1 }
    $base = if (Invoke-Git @('rev-parse', '--verify', '-q', 'origin/main')) { 'origin/main' } else { 'main' }
    if (-not $TargetsDir) {
        # The pool sits beside the checkout: <root>\foxglove or <root>\ops-main -> <root>\worktrees\_targets,
        # or a worktree <root>\worktrees\<wt> -> <root>\worktrees\_targets.
        $parent = Split-Path -Parent $Repo
        $TargetsDir = @((Join-Path $parent 'worktrees\_targets'), (Join-Path $parent '_targets')) |
            Where-Object { Test-Path $_ } | Select-Object -First 1
        if (-not $TargetsDir) { Stop-OpsJob -Status 'warn' -Text "no worktrees\_targets beside $Repo; pass -TargetsDir" -Code 1 }
    }
    $stale = @(Get-StaleBranches $base $StaleDays)
    $wts = @(Get-WorktreeRows)
    $sizes = @(Get-DirSizes $TargetsDir $SizeBudgetSec)
    $ign = @(Get-IgnoredFolders)
    $out = if ($OutFile) { $OutFile } else { Join-Path (Get-OpsDir) ("janitor-{0}.md" -f (Get-Date -Format 'yyyy-MM-dd')) }
    Set-Content -Encoding UTF8 $out (Format-Scorecard -Stale $stale -Worktrees $wts -Sizes $sizes -Ignored $ign)
    $gb = [math]::Round([double](@($sizes) | Where-Object { -not $_.Partial } | Measure-Object GB -Sum).Sum, 1)
    Stop-OpsJob -Status 'ok' -Text "$(@($stale).Count) stale branches, $(@($wts).Count) worktrees, targets $gb GB, $(@($ign).Count) ignored folders; scorecard $out"
}
