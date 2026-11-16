# MIT. Foxglove stall check (#1126): facts and the decision, no LLM, no network, no GUI.
# Runs unattended from Task Scheduler (scripts\ops\register-stall-check.ps1) at below-normal
# priority with a hard time limit. Read-only apart from its own files in <Root>\worktrees\_tools:
#   stall-status.json  what is stalled, since when, the last check time, the facts, and `action`
#   stall-alarm.txt    the alarm lines, only while an alarm is confirmed (deleted when it clears)
#   stall-state.json   two-strikes memory (last rules, since-times, freeze-cleared time)
#   game-freeze.flag   if present (and no game runs) it is deleted and `action` is freeze_lifted
# The one summary line also goes to ops\summary.log (ops-common.ps1).
#   powershell -File scripts\ops\stall-check.ps1 [-Root <folder that holds worktrees\>]
# Open PRs and the #90 queue need the network, so this script never fetches them: another job
# (the Hub, On-call) may drop worktrees\_tools\open-prs.json (gh pr list --json
# number,title,updatedAt,isDraft) and user-queue-count.txt; both are used only while fresher than
# -CacheMaxAgeMin. Without them prs_known is false, which is never "quiet" (no pause, no rule b).
# Rules (the Hub's six, see docs/ops-stall-check.md): a no progress >90 min while work exists
# (grace 60 min after a session start or a cleared freeze), b open non-draft PR idle >120 min with
# no cargo/rustc, c local job (hideout-fuzz, python) past 180 min, e C: free <50 GB, f possible
# usage-limit stop (once). a, b, c are reported only on two runs in a row; e and f at once.
param(
    [string]$Repo = (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)),
    [string]$Root,            # default: the folder that holds worktrees\ (next to the main checkout)
    [string]$ToolsDir,        # default: <Root>\worktrees\_tools
    [string]$ProjectDir,      # Claude transcripts; default: ~\.claude\projects\<Root encoded>
    [string]$GitDir,          # the main checkout's .git; default: from -Repo
    [string]$PrFile, [string]$QueueFile,
    [int]$CacheMaxAgeMin = 180,
    [string]$ExcludeTranscriptPrefix = '59de466b',   # the Hub's transcript never counts as work
    # Test hooks (window-free tests): a JSON array of {name,minutes}, and free GB on C:.
    [string]$JobsJson, [double]$FreeGB = -1,
    [int]$TimeLimitSec = 120
)

# The decision, pure: facts and state in, action out. $Facts: game_running, freeze_flag,
# free_gb, prs (number, draft, idle_min), prs_known, jobs (name, minutes), any_min, grace_active.
# $State: last_rules (array), f_reported (bool), since (hashtable rule -> iso time).
function Get-StallDecision {
    param($Facts, $State, [datetime]$Now)
    $st = @{ last_rules = @($State.last_rules); f_reported = [bool]$State.f_reported; since = @{}; freeze_cleared_at = $State.freeze_cleared_at }
    if ($State.since) { foreach ($k in @($State.since.Keys)) { $st.since[$k] = $State.since[$k] } }
    $out = @{ action = 'none'; rules = @(); lines = @(); stalled = @(); strikes = @{}; state = $st }
    if ($Facts.game_running) { return $out }   # the user is gaming: work is held by design, stay silent
    if ($Facts.freeze_flag) { $st.freeze_cleared_at = $Now.ToString('o'); $out.action = 'freeze_lifted'; return $out }

    $prs = @($Facts.prs); $jobs = @($Facts.jobs); $any = $Facts.any_min
    $cargo = @($jobs | Where-Object { $_.name -in 'cargo', 'rustc' }).Count -gt 0
    $hasWork = (@($prs | Where-Object { -not $_.draft }).Count -gt 0) -or $jobs.Count -gt 0
    if (-not $Facts.prs_known) { $hasWork = $true }   # unknown PR state is never "quiet"
    $found = [ordered]@{}   # rule -> line
    if (-not $Facts.grace_active -and $hasWork -and $null -ne $any -and $any -gt 90) { $found['a'] = "a: work exists but nothing has progressed for $any min" }
    foreach ($p in $prs) {
        if (-not $p.draft -and $p.idle_min -gt 120 -and -not $cargo) { $found['b'] = "b: PR #$($p.number) idle $($p.idle_min) min with no build running" }
    }
    $over = @($jobs | Where-Object { $_.name -in 'hideout-fuzz', 'python' -and $_.minutes -gt 180 })   # llmster is a long-lived server, not a job
    if ($over.Count) { $found['c'] = "c: local job past 3 h: $((($over | ForEach-Object { $_.name }) -join ', '))" }
    if ($Facts.free_gb -lt 50) { $found['e'] = "e: C: has $($Facts.free_gb) GB free" }
    $quiet = $Facts.prs_known -and -not $hasWork -and $prs.Count -eq 0 -and $null -ne $any -and $any -gt 120
    if ($quiet -and -not $st.f_reported) { $found['f'] = "f: possible usage-limit stop (no progress for $any min, nothing open)"; $st.f_reported = $true }
    elseif ($quiet) { $out.action = 'pause' }
    if ($null -ne $any -and $any -le 120) { $st.f_reported = $false }

    $newSince = @{}
    foreach ($r in @($found.Keys)) {
        $out.strikes[$r] = if (@($State.last_rules) -contains $r) { 2 } else { 1 }
        $newSince[$r] = if ($r -eq 'a') { $Now.AddMinutes(-$any).ToString('o') } elseif ($st.since[$r]) { $st.since[$r] } else { $Now.ToString('o') }
        $confirmed = ($r -in 'e', 'f') -or $out.strikes[$r] -ge 2
        $out.stalled += [ordered]@{ rule = $r; what = $found[$r]; since = $newSince[$r]; confirmed = $confirmed }
        if ($confirmed) { $out.rules += $r; $out.lines += $found[$r] }
    }
    $st.since = $newSince
    $st.last_rules = @($found.Keys)
    if ($out.rules.Count) { $out.action = 'alarm' }
    $out
}

# Minutes since a time, or $null.
function Get-MinSince($t, [datetime]$Now) { if ($t) { [int]($Now - $t).TotalMinutes } else { $null } }

if ($MyInvocation.InvocationName -eq '.') { return }   # dot-sourced by the tests: functions only

. (Join-Path $PSScriptRoot 'ops-common.ps1')
Start-OpsJob -Name 'stall-check' -TimeLimitSec $TimeLimitSec -SkipGameCheck   # below normal, watchdog; the game state is a fact here, not a reason to skip
$ErrorActionPreference = 'SilentlyContinue'

$now = Get-Date
if (-not $Root) { $Root = Split-Path $Repo -Parent; if (-not (Test-Path (Join-Path $Root 'worktrees'))) { $Root = Split-Path $Root -Parent } }
if (-not $ToolsDir) { $ToolsDir = Join-Path $Root 'worktrees\_tools' }
if (-not (Test-Path $ToolsDir)) { New-Item -ItemType Directory -Force $ToolsDir | Out-Null }
if (-not $ProjectDir) { $ProjectDir = Join-Path $env:USERPROFILE (".claude\projects\" + ($Root -replace '[:\\/]', '-')) }
if (-not $GitDir) { $GitDir = (& git -C $Repo rev-parse --path-format=absolute --git-common-dir 2>$null) }
if (-not $PrFile) { $PrFile = Join-Path $ToolsDir 'open-prs.json' }
if (-not $QueueFile) { $QueueFile = Join-Path $ToolsDir 'user-queue-count.txt' }
$statePath = Join-Path $ToolsDir 'stall-state.json'
$flagPath = Join-Path $ToolsDir 'game-freeze.flag'

# State: JSON on disk, a hashtable in memory.
$state = @{ last_rules = @(); f_reported = $false; since = @{}; freeze_cleared_at = $null }
if (Test-Path $statePath) {
    try {
        $s = Get-Content -Raw -Encoding UTF8 $statePath | ConvertFrom-Json
        $state.last_rules = @($s.last_rules); $state.f_reported = [bool]$s.f_reported; $state.freeze_cleared_at = $s.freeze_cleared_at
        if ($s.since) { foreach ($p in $s.since.PSObject.Properties) { $state.since[$p.Name] = $p.Value } }
    } catch {}
}

$facts = [ordered]@{ at = $now.ToUniversalTime().ToString('u') }
& (Join-Path $PSScriptRoot 'game-check.ps1') -Quiet
$facts.game_running = ($LASTEXITCODE -eq 3)
$facts.freeze_flag = (Test-Path $flagPath)

# Jobs: what actually runs (the Hub and stall-check never count).
if ($JobsJson) { $jobs = @($JobsJson | ConvertFrom-Json | ForEach-Object { $_ }) }
else {
    $jobs = @(Get-Process cargo, rustc, hideout-fuzz, python, llmster -ErrorAction SilentlyContinue |
        ForEach-Object { [pscustomobject]@{ name = $_.ProcessName; minutes = (Get-MinSince $_.StartTime $now) } })
}
$facts.jobs = @($jobs)
$freeGb = if ($FreeGB -ge 0) { $FreeGB } else { [int]((Get-PSDrive C).Free / 1GB) }
$facts.disk = [ordered]@{ c_free_gb = $freeGb; low = ($freeGb -lt 50) }

# PRs and the queue count: cache files only, never the network.
function Get-FreshFile($path) { if ((Test-Path $path) -and ((Get-MinSince (Get-Item $path).LastWriteTime $now) -le $CacheMaxAgeMin)) { $path } }
$prs = @(); $prsKnown = $false
$pf = Get-FreshFile $PrFile
if ($pf) {
    try { $prs = @((Get-Content -Raw -Encoding UTF8 $pf | ConvertFrom-Json) | ForEach-Object { $_ }); $prsKnown = $true } catch { $prs = @() }
}
$facts.prs_known = $prsKnown
$prRows = @($prs | ForEach-Object { [pscustomobject]@{ number = $_.number; draft = [bool]$_.isDraft; idle_min = [int]($now.ToUniversalTime() - ([datetime]$_.updatedAt).ToUniversalTime()).TotalMinutes } })
$facts.open_prs = $prRows
$qf = Get-FreshFile $QueueFile
$facts.user_queue_open_items = if ($qf) { [int](Get-Content -Raw $qf).Trim() } else { $null }

# Progress: minutes since the newest signal of work.
$tx = @(Get-ChildItem $ProjectDir -Filter *.jsonl -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike "$ExcludeTranscriptPrefix*" } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 40)
$work = @($tx | Where-Object { -not ((Get-Content $_.FullName -TotalCount 5) -match 'foxglove-stall-check|Foxglove stall check') })
$sessionT = ($work | Select-Object -First 1).LastWriteTime
$newSession = $work | Where-Object { $_.CreationTime -gt $now.AddMinutes(-60) } | Select-Object -First 1
$commitT = (Get-ChildItem (Join-Path $GitDir 'worktrees\*\index'), (Join-Path $GitDir 'index'), (Join-Path $GitDir 'logs\HEAD') -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1).LastWriteTime
$prMin = if ($prRows.Count) { ($prRows | Measure-Object -Property idle_min -Minimum).Minimum } else { $null }
$cands = @((Get-MinSince $sessionT $now), (Get-MinSince $commitT $now), $prMin) | Where-Object { $null -ne $_ }
$any = if ($cands.Count) { ($cands | Measure-Object -Minimum).Minimum } else { $null }
$facts.progress = [ordered]@{ session_min = (Get-MinSince $sessionT $now); commit_min = (Get-MinSince $commitT $now); pr_update_min = $prMin; any_min = $any }

# Grace: a session started in the last 60 min, or the freeze flag cleared in the last 60 min.
$grace = [ordered]@{ active = $false; why = '' }
$fc = if ($state.freeze_cleared_at) { [datetime]$state.freeze_cleared_at } else { $null }
if ($newSession) { $grace.active = $true; $grace.why = 'a session started in the last 60 min' }
elseif ($fc -and (Get-MinSince $fc $now) -lt 60) { $grace.active = $true; $grace.why = 'the game freeze cleared in the last 60 min' }
$facts.grace = $grace

$d = Get-StallDecision -Now $now -State $state -Facts @{
    game_running = $facts.game_running; freeze_flag = $facts.freeze_flag; free_gb = $freeGb; prs = $prRows; prs_known = $prsKnown
    jobs = $jobs; any_min = $any; grace_active = $grace.active
}
if ($d.action -eq 'freeze_lifted') { Remove-Item $flagPath -ErrorAction SilentlyContinue }

$facts.strikes = $d.strikes
$facts.alarm = [ordered]@{ confirmed = ($d.action -eq 'alarm'); rules = @($d.rules); lines = @($d.lines) }
# The head of the file: what is stalled, since when, and when this was last checked.
$status = [ordered]@{ last_check = $now.ToString('o'); stalled = @($d.stalled); action = $d.action }
foreach ($k in $facts.Keys) { $status[$k] = $facts[$k] }

$json = $status | ConvertTo-Json -Depth 6
$d.state | ConvertTo-Json -Depth 4 | Set-Content $statePath -Encoding UTF8
$json | Set-Content (Join-Path $ToolsDir 'stall-status.json') -Encoding UTF8
$alarmPath = Join-Path $ToolsDir 'stall-alarm.txt'
if ($d.action -eq 'alarm') { ($d.lines -join "`n") | Set-Content $alarmPath -Encoding UTF8 } elseif (Test-Path $alarmPath) { Remove-Item $alarmPath -ErrorAction SilentlyContinue }
Write-Output $json
Stop-OpsJob -Status 'ok' -Text "action $($d.action), $(@($d.stalled).Count) stalled"
