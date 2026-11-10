# MIT. Foxglove digest as a script (#949). Collects gh PR / issue / merge data, the disk, and the
# tail of the health CSVs into a facts file. With -Summarize the local LLM (tools\local-llm.ps1,
# git-ignored, if present) writes a summary of at most 10 lines from that file ONLY; the raw
# facts stay in the file. No Claude. Read-only.
#   powershell -File scripts\ops\foxglove-digest.ps1 [-SinceHours 12] [-Summarize] [-TimeLimitSec 900]
param(
    [int]$SinceHours = 12,
    [switch]$Summarize,
    [string]$Repo = 'BanditF/foxglove',
    [string]$LlmScript,
    [int]$TimeLimitSec = 900
)
. (Join-Path $PSScriptRoot 'ops-common.ps1')

function Get-GhJson([string[]]$GhArgs) {
    $r = Invoke-Bounded -File 'gh' -Arguments $GhArgs -TimeoutSec 60
    if ($r.Code -ne 0 -or -not $r.Out) { return $null }
    $r.Out | ConvertFrom-Json
}

# Pure: the facts text from already-collected data (any of them may be $null = unavailable).
function Format-Facts {
    param($OpenPrs, $Merged, $Needs, [string]$Queue, [double]$DiskFreeGb, [string[]]$HealthTail, [string[]]$StretchTail, [datetime]$Now = (Get-Date))
    $o = New-Object System.Collections.Generic.List[string]
    $o.Add("# Foxglove facts, $($Now.ToString('s'))")
    $o.Add('')
    $o.Add('## Open PRs')
    if ($null -eq $OpenPrs) { $o.Add('(gh unavailable)') } elseif (-not @($OpenPrs).Count) { $o.Add('none') }
    else { foreach ($p in $OpenPrs) { $o.Add("- #$($p.number) $($p.title) [base $($p.baseRefName)$(if ($p.isDraft) { ', draft' })] updated $($p.updatedAt)") } }
    $o.Add('')
    $o.Add('## Merged since last window')
    if ($null -eq $Merged) { $o.Add('(gh unavailable)') } elseif (-not @($Merged).Count) { $o.Add('none') }
    else { foreach ($p in $Merged) { $o.Add("- #$($p.number) $($p.title) merged $($p.mergedAt)") } }
    $o.Add('')
    $o.Add('## Issues labelled needs-human')
    if ($null -eq $Needs) { $o.Add('(gh unavailable)') } elseif (-not @($Needs).Count) { $o.Add('none') }
    else { foreach ($i in $Needs) { $o.Add("- #$($i.number) $($i.title)") } }
    $o.Add('')
    $o.Add('## Human queue (#90 body)')
    $o.Add($(if ($Queue) { $Queue.Trim() } else { '(unavailable)' }))
    $o.Add('')
    $o.Add('## Host')
    $o.Add("C: free $DiskFreeGb GB")
    $o.Add('')
    $o.Add('## pc-health-log.csv (last lines)')
    $o.Add($(if ($HealthTail) { $HealthTail -join "`n" } else { '(no data yet)' }))
    $o.Add('')
    $o.Add('## standby-stretches.csv (last lines)')
    $o.Add($(if ($StretchTail) { $StretchTail -join "`n" } else { '(no data yet)' }))
    $o -join "`n"
}

# Pure: keep at most $Max non-empty lines.
function Limit-Lines([string]$Text, [int]$Max = 10) {
    (($Text -split "`r?`n") | Where-Object { $_.Trim() } | Select-Object -First $Max) -join "`n"
}

function Find-LocalLlm([string]$Explicit) {
    $repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
    $cands = @($Explicit, $env:FOXGLOVE_LOCAL_LLM, (Join-Path $repo 'tools\local-llm.ps1'),
        (Join-Path $repo '..\tools\local-llm.ps1'), (Join-Path $repo '..\..\tools\local-llm.ps1')) | Where-Object { $_ }
    foreach ($c in $cands) { if (Test-Path $c) { return (Resolve-Path $c).Path } }
    $null
}

function Get-Tail([string]$Path, [int]$N) {
    if (Test-Path $Path) { @(Get-Content -Encoding UTF8 $Path -Tail $N) } else { @() }
}

if ($MyInvocation.InvocationName -ne '.') {
    Start-OpsJob -Name 'foxglove-digest' -TimeLimitSec $TimeLimitSec
    $dir = Get-OpsDir
    $since = (Get-Date).AddHours(-$SinceHours).ToString('yyyy-MM-ddTHH:mm:ssZ')
    $open = Get-GhJson @('pr', 'list', '--repo', $Repo, '--state', 'open', '--json', 'number,title,baseRefName,isDraft,updatedAt')
    $merged = Get-GhJson @('pr', 'list', '--repo', $Repo, '--state', 'merged', '--search', "merged:>=$since", '--json', 'number,title,mergedAt')
    $needs = Get-GhJson @('issue', 'list', '--repo', $Repo, '--label', 'needs-human', '--json', 'number,title')
    $q = Get-GhJson @('issue', 'view', '90', '--repo', $Repo, '--json', 'body')
    $facts = Format-Facts -OpenPrs $open -Merged $merged -Needs $needs -Queue $(if ($q) { $q.body } else { '' }) `
        -DiskFreeGb ([math]::Round((Get-PSDrive C).Free / 1GB, 1)) `
        -HealthTail (Get-Tail (Join-Path $dir 'pc-health-log.csv') 4) -StretchTail (Get-Tail (Join-Path $dir 'standby-stretches.csv') 4)
    $factsFile = Join-Path $dir 'digest-facts.md'
    Set-Content -Encoding UTF8 $factsFile $facts
    $note = "facts in $factsFile ($(@($open).Count) open PRs, $(@($merged).Count) merged)"
    if ($Summarize) {
        $llm = Find-LocalLlm $LlmScript
        if (-not $llm) { Stop-OpsJob -Status 'ok' -Text "$note; no local-llm.ps1 found, no summary" }
        $prompt = Join-Path $dir 'digest-prompt.txt'
        $out = Join-Path $dir 'digest-summary.txt'
        Set-Content -Encoding UTF8 $prompt ("Summarise the facts below in at most 10 short lines: what merged, what is open, what waits on the user, host health. Use ONLY these facts; invent nothing.`n`n" + $facts)
        $r = Invoke-Bounded -File 'powershell' -TimeoutSec 600 -Arguments @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $llm, '-PromptFile', $prompt, '-OutFile', $out, '-TimeoutSec', '300', '-Task', 'foxglove-digest')
        if ($r.Code -eq 3) { Stop-OpsJob -Status 'skipped' -Text "$note; game running, no summary" -Code 3 }
        if ($r.Code -ne 0 -or -not (Test-Path $out)) { Stop-OpsJob -Status 'warn' -Text "$note; local LLM failed ($($r.Code))" -Code 1 }
        Set-Content -Encoding UTF8 $out (Limit-Lines (Get-Content -Raw -Encoding UTF8 $out) 10)
        $note += "; summary in $out"
    }
    Stop-OpsJob -Status 'ok' -Text $note
}
