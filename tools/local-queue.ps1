# MIT. The local-compute queue feeder (#1128). One batch per invocation, no loop, no timer:
# reads open issues labelled compute:local-llm, runs each through tools\local-llm.ps1 (the model
# wrapper) and posts ONE comment of at most 20 lines, marked "Local LLM draft (needs review): <model>",
# then adds the label local-review. Never closes an issue, never merges, never edits code.
# A Sonnet agent at low effort reviews every comment before it is used (docs/local-compute.md).
#
#   powershell -File tools\local-queue.ps1 -List        # print the queue and what would happen
#   powershell -File tools\local-queue.ps1 -DryRun      # same plan, plus the game state; nothing runs or posts
#   powershell -File tools\local-queue.ps1              # run one batch
#
# Issue body markers, one per line (docs/local-compute.md):
#   type: log-triage | pr-body-draft | release-notes | test-vectors | docs-prescreen
#   input: <path>     a file inside the repo, its sibling worktrees folder or _logs (log-triage needs it)
#   prompt: <text>    one line of extra instruction (the other types need input or prompt or both)
# Skipped: labelled local-review (already done) or needs-claude, and unknown or missing types.
#
# Safety: exits 0 with "held: game running" if game-check says 3, at the start AND between jobs
# (the model takes VRAM). BelowNormal priority. Hard caps: -JobTimeoutSec per job, -BatchTimeoutSec
# for the batch, -MaxJobs jobs. On a timeout the wrapper's process tree is killed by its exact PID
# and the model is unloaded with lms.exe by exact path (and the server stopped if this run found it
# down), so nothing stays resident. The wrapper appends the tools\local-llm-usage.csv row per job
# (task = local-queue:<type>). Helpers come from scripts\ops (ops-common.ps1, game-check.ps1);
# -OpsScripts points elsewhere if this file is copied out of the repo.
param(
    [switch]$List,
    [Alias('WhatIf')][switch]$DryRun,
    [string]$Repo = 'BanditF/foxglove',
    [int]$JobTimeoutSec = 600,
    [int]$BatchTimeoutSec = 2700,
    [int]$MaxJobs = 5,
    [string]$Model = 'qwen3.6-35b-a3b',
    [string]$LlmScript,                                 # default: env FOXGLOVE_LOCAL_LLM, else tools\local-llm.ps1 next to this script
    [string]$OpsScripts = (Join-Path $PSScriptRoot '..\scripts\ops'),
    [string]$Gh = 'gh',
    [string]$Lms = $(if ($env:FOXGLOVE_LMS) { $env:FOXGLOVE_LMS } else { Join-Path $env:USERPROFILE '.lmstudio\bin\lms.exe' }),
    [string[]]$AllowRoot = @()                          # extra folders an input: path may point into
)
$ErrorActionPreference = 'Stop'
$script:Types = 'log-triage', 'pr-body-draft', 'release-notes', 'test-vectors', 'docs-prescreen'
$script:Instruction = @{
    'log-triage'     = 'Summarise the failures in the log below and their most likely causes, quoting the log line that shows each. List only failures that are in the log. If there are none, say so in one line.'
    'pr-body-draft'  = 'Draft a pull request description (what changed and why, what was tested) from the material below. Use only the material.'
    'release-notes'  = 'Draft short user-facing release notes (bullets) from the material below. Use only the material.'
    'test-vectors'   = 'Draft test vectors (input and expected output, one per line) from the spec below. Use only the spec; mark any gap in it as GAP.'
    'docs-prescreen' = 'Review the documentation text below for contradictions, stale statements and broken references. List concrete findings with the quoted text; no style comments.'
}

function Get-Marker([string]$Body, [string]$Key) {
    $m = [regex]::Match($Body, "(?im)^[ \t>*-]*$Key[ \t]*:[ \t]*(.+?)[ \t]*$")
    if ($m.Success) { $m.Groups[1].Value.Trim().Trim('`', '"', "'") } else { $null }
}

# Pure: the parsed job description of one issue.
function Get-JobSpec($Issue) {
    $body = [string]$Issue.body
    [pscustomobject]@{
        Number = [int]$Issue.number; Title = [string]$Issue.title
        Labels = @($Issue.labels | ForEach-Object { $_.name })
        Type = (Get-Marker $body 'type'); Input = (Get-Marker $body 'input'); Prompt = (Get-Marker $body 'prompt')
    }
}

# Pure: $null if the job can run, else the reason it is skipped.
function Get-SkipReason($Spec) {
    if ($Spec.Labels -contains 'local-review') { return 'already has local-review' }
    if ($Spec.Labels -contains 'needs-claude') { return 'labelled needs-claude' }
    if (-not $Spec.Type) { return 'no type: marker' }
    if ($script:Types -notcontains $Spec.Type) { return "unknown type '$($Spec.Type)'" }
    if ($Spec.Type -eq 'log-triage' -and -not $Spec.Input) { return 'log-triage needs input:' }
    if (-not $Spec.Input -and -not $Spec.Prompt) { return 'needs input: or prompt:' }
    $null
}

# Pure: the full path if $Path lies inside one of $Roots (no .. escapes), else $null.
function Resolve-AllowedInput([string]$Path, [string[]]$Roots) {
    try { $full = [IO.Path]::GetFullPath($Path) } catch { return $null }
    foreach ($r in $Roots) {
        $root = [IO.Path]::GetFullPath($r).TrimEnd('\') + '\'
        if ($full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { return $full }
    }
    $null
}

function Get-InputRoots([string[]]$Extra) {
    $repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $roots = @($repo)
    $parent = Split-Path -Parent $repo
    if ((Split-Path -Leaf $parent) -eq 'worktrees') { $roots += $parent }
    foreach ($c in (Join-Path $parent 'worktrees'), (Join-Path $parent '_logs')) { if (Test-Path $c) { $roots += $c } }
    $roots + @($Extra | Where-Object { $_ })
}

# Pure: the prompt text. $Material is already read (tail for logs, head otherwise).
function New-JobPrompt($Spec, [string]$Material) {
    $p = New-Object System.Collections.Generic.List[string]
    $p.Add($script:Instruction[$Spec.Type])
    $p.Add('Answer in at most 18 lines. Plain text, no preamble. Invent nothing that is not in the input.')
    if ($Spec.Prompt) { $p.Add("Extra instruction from the issue: $($Spec.Prompt)") }
    if ($Material) { $p.Add("--- INPUT ---`n$Material`n--- END INPUT ---") }
    $p -join "`n`n"
}

# Pure: the comment, never more than 20 lines (header + at most 19).
function Format-Comment([string]$Text, [string]$ModelName) {
    $lines = New-Object System.Collections.Generic.List[string]
    $prevBlank = $true
    foreach ($l in ($Text -split "`r?`n")) {
        $t = $l.TrimEnd()
        if (-not $t) { if ($prevBlank) { continue }; $prevBlank = $true } else { $prevBlank = $false }
        $lines.Add($t)
    }
    while ($lines.Count -and -not $lines[$lines.Count - 1]) { $lines.RemoveAt($lines.Count - 1) }
    $body = @($lines | Select-Object -First 19)
    "Local LLM draft (needs review): $ModelName`n" + ($body -join "`n")
}

function Read-Material([string]$Path, [string]$Type) {
    $raw = [IO.File]::ReadAllText($Path, [Text.Encoding]::UTF8)
    $max = 24000   # about 6k tokens, well inside the wrapper's 16k context
    if ($raw.Length -le $max) { return $raw }
    if ($Type -eq 'log-triage') { return "(start of log cut)`n" + $raw.Substring($raw.Length - $max) }
    $raw.Substring(0, $max) + "`n(rest cut)"
}

# Runs a command with a hard wait; on timeout kills the whole tree by exact PID. @{Code; Out; Err}; Code -1 = killed.
function Invoke-Hard([string]$File, [string[]]$Arguments, [int]$TimeoutSec) {
    $o = [IO.Path]::GetTempFileName(); $e = [IO.Path]::GetTempFileName()
    try {
        $argLine = ($Arguments | ForEach-Object { if ($_ -match '[\s"]' -or $_ -eq '') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ } }) -join ' '
        $sp = @{ FilePath = $File; NoNewWindow = $true; PassThru = $true; RedirectStandardOutput = $o; RedirectStandardError = $e }
        if ($argLine) { $sp.ArgumentList = $argLine }
        $p = Start-Process @sp
        $null = $p.Handle
        if (-not $p.WaitForExit([Math]::Max(1, $TimeoutSec) * 1000)) {
            & taskkill.exe /T /F /PID $p.Id 2>&1 | Out-Null
            return @{ Code = -1; Out = ''; Err = 'timeout' }
        }
        $p.WaitForExit()
        @{ Code = $p.ExitCode; Out = [IO.File]::ReadAllText($o); Err = [IO.File]::ReadAllText($e) }
    } finally { Remove-Item $o, $e -ErrorAction SilentlyContinue }
}

function Test-ServerUp {
    if ($env:LQ_SERVER_UP) { return $env:LQ_SERVER_UP -eq '1' }   # tests pin it: the real port may be up (pilot TTL leaves it)
    try { Invoke-RestMethod 'http://127.0.0.1:1234/v1/models' -TimeoutSec 2 | Out-Null; $true } catch { $false }
}

# After a kill the wrapper's own finally never ran: unload by exact lms.exe path, stop the server if we found it down.
function Stop-Resident([string]$ModelName, [bool]$ServerWasUp) {
    if (-not (Test-Path $Lms)) { return }
    $null = Invoke-Hard $Lms @('unload', $ModelName) 60
    if (-not $ServerWasUp) { $null = Invoke-Hard $Lms @('server', 'stop') 60 }
}

function Test-GameRunning {
    $gc = Join-Path $OpsScripts 'game-check.ps1'
    $r = Invoke-Hard (Get-Command powershell).Source @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $gc, '-Quiet') 60
    $r.Code -eq 3
}

function Get-Queue {
    $r = Invoke-Hard $Gh @('issue', 'list', '-R', $Repo, '-l', 'compute:local-llm', '--state', 'open', '--json', 'number,title,body,labels') 60
    if ($r.Code -ne 0) { throw "gh issue list failed ($($r.Code)): $($r.Err)" }
    if (-not $r.Out.Trim()) { return @() }
    $parsed = ConvertFrom-Json $r.Out   # 5.1 emits a JSON array as one object: enumerate with foreach
    @(foreach ($i in $parsed) { Get-JobSpec $i }) | Sort-Object Number
}

function Show-Plan($Queue) {
    if (-not $Queue.Count) { Write-Host 'queue empty (no open compute:local-llm issues)'; return }
    $n = 0
    foreach ($s in $Queue) {
        $why = Get-SkipReason $s
        if (-not $why -and $s.Input -and -not (Resolve-AllowedInput $s.Input (Get-InputRoots $AllowRoot))) { $why = 'input: path outside the allowed folders' }
        if (-not $why) { $n++; if ($n -gt $MaxJobs) { $why = "over -MaxJobs $MaxJobs" } }
        $state = if ($why) { "skip ($why)" } else { 'would run' }
        Write-Host ("#{0} [{1}] {2}: {3}" -f $s.Number, $(if ($s.Type) { $s.Type } else { '-' }), $s.Title, $state)
    }
}

function Invoke-Batch {
    $queue = Get-Queue
    $started = Get-Date
    $res = @{ Posted = 0; Skipped = 0; Failed = 0; Held = $false; Notes = @() }
    $llm = if ($LlmScript) { $LlmScript } elseif ($env:FOXGLOVE_LOCAL_LLM) { $env:FOXGLOVE_LOCAL_LLM } else { Join-Path $PSScriptRoot 'local-llm.ps1' }
    $roots = Get-InputRoots $AllowRoot
    $work = Join-Path (Get-OpsDir) 'local-queue'
    New-Item -ItemType Directory -Force $work | Out-Null
    $ran = 0; $serverWasUp = $null; $killed = $false
    try {
        foreach ($s in $queue) {
            $why = Get-SkipReason $s
            $in = $null
            if (-not $why -and $s.Input) {
                $in = Resolve-AllowedInput $s.Input $roots
                if (-not $in) { $why = 'input: path outside the allowed folders' }
                elseif (-not (Test-Path -LiteralPath $in -PathType Leaf)) { $why = 'input: file not found' }
            }
            if ($why) { $res.Skipped++; $res.Notes += "#$($s.Number) skipped: $why"; continue }
            if ($ran -ge $MaxJobs) { $res.Notes += "max jobs ($MaxJobs) reached"; break }
            $left = [int]($BatchTimeoutSec - ((Get-Date) - $started).TotalSeconds)
            if ($left -le 30) { $res.Notes += 'batch time cap reached'; break }
            if (Test-GameRunning) { $res.Held = $true; break }
            if ($null -eq $serverWasUp) { $serverWasUp = Test-ServerUp }
            $ran++
            $material = if ($in) { Read-Material $in $s.Type } else { '' }
            $pf = Join-Path $work "job-$($s.Number).in.txt"; $of = Join-Path $work "job-$($s.Number).out.txt"
            Remove-Item $of -ErrorAction SilentlyContinue
            [IO.File]::WriteAllText($pf, (New-JobPrompt $s $material), (New-Object Text.UTF8Encoding $false))
            $cap = [Math]::Min($JobTimeoutSec, $left)
            $r = Invoke-Hard (Get-Command powershell).Source @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $llm,
                '-PromptFile', $pf, '-OutFile', $of, '-Model', $Model, '-TimeoutSec', [string][Math]::Min(300, $cap), '-Task', "local-queue:$($s.Type)") $cap
            if ($r.Code -eq 3) { $res.Held = $true; break }
            if ($r.Code -eq -1) {
                $killed = $true; $res.Failed++; $res.Notes += "#$($s.Number) killed at the $cap s cap"
                Stop-Resident $Model ([bool]$serverWasUp)
                continue
            }
            if ($r.Code -ne 0 -or -not (Test-Path $of) -or -not ([IO.File]::ReadAllText($of).Trim())) {
                $res.Failed++; $res.Notes += "#$($s.Number) model run failed ($($r.Code))"; continue
            }
            $cf = Join-Path $work "job-$($s.Number).comment.md"
            [IO.File]::WriteAllText($cf, (Format-Comment ([IO.File]::ReadAllText($of)) $Model), (New-Object Text.UTF8Encoding $false))
            $c = Invoke-Hard $Gh @('issue', 'comment', "$($s.Number)", '-R', $Repo, '-F', $cf) 60
            if ($c.Code -ne 0) { $res.Failed++; $res.Notes += "#$($s.Number) comment failed ($($c.Code))"; continue }
            $l = Invoke-Hard $Gh @('issue', 'edit', "$($s.Number)", '-R', $Repo, '--add-label', 'local-review') 60
            if ($l.Code -ne 0) { $res.Notes += "#$($s.Number) commented but label failed ($($l.Code))" }
            $res.Posted++
        }
    } finally {
        Remove-Item (Join-Path $work 'job-*') -ErrorAction SilentlyContinue
    }
    $res
}

if ($MyInvocation.InvocationName -ne '.') {
    . (Join-Path $OpsScripts 'ops-common.ps1')
    if ($List) { Show-Plan (Get-Queue); exit 0 }
    if ($DryRun) {
        Show-Plan (Get-Queue)
        Write-Host $(if (Test-GameRunning) { 'dry run: would hold, game running' } else { "dry run: no game, would run (caps: $JobTimeoutSec s/job, $BatchTimeoutSec s batch, $MaxJobs jobs)" })
        exit 0
    }
    # The game checks are ours (exit 0 with "held" instead of the ops exit 3); the watchdog is a backstop above the batch cap.
    Start-OpsJob -Name 'local-queue' -TimeLimitSec ($BatchTimeoutSec + 120) -SkipGameCheck
    if (Test-GameRunning) { Stop-OpsJob -Status 'held' -Text 'game running' -Code 0 }
    $r = Invoke-Batch
    $note = "posted $($r.Posted), skipped $($r.Skipped), failed $($r.Failed)" + $(if ($r.Notes.Count) { '; ' + ($r.Notes -join '; ') } else { '' })
    if ($r.Held) { Stop-OpsJob -Status 'held' -Text "game running; $note" -Code 0 }
    if ($r.Failed) { Stop-OpsJob -Status 'warn' -Text $note -Code 1 }
    Stop-OpsJob -Status 'ok' -Text $note
}
