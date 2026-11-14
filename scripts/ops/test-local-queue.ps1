# MIT. Window-free tests for tools/local-queue.ps1 (#1128). Stubs only: a fake gh, a fake local-llm
# wrapper, a fake game-check and a fake lms.exe. No model, no LM Studio, no network, no real gh.
# Run by test-ops.ps1 (and so by check.ps1). Exit 0 if all pass.
#   powershell -File scripts\ops\test-local-queue.ps1
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$queue = Join-Path $here '..\..\tools\local-queue.ps1'
$fails = 0
function Check([string]$name, $cond) {
    if ($cond) { Write-Host "ok   $name" } else { Write-Host "FAIL $name"; $script:fails++ }
}
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("lq-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $tmp | Out-Null
$ps = (Get-Command powershell).Source
$env:FOXGLOVE_OPS_DIR = Join-Path $tmp 'ops'
$env:LQ_DIR = $tmp
$env:LQ_SERVER_UP = '0'   # never probe the real LM Studio port (a pilot may have left it up)
try {
    # --- stubs ---
    $stub = Join-Path $tmp 'stub'; New-Item -ItemType Directory $stub | Out-Null
    Copy-Item (Join-Path $here 'ops-common.ps1') $stub
    # game-check: exit 3 once its call counter reaches $env:LQ_GAME_AT (0 = never).
    Set-Content -Encoding UTF8 (Join-Path $stub 'game-check.ps1') @'
param([switch]$Quiet)
$f = Join-Path $env:LQ_DIR 'game-calls.txt'
$n = 1 + [int]$(if (Test-Path $f) { Get-Content $f } else { 0 })
Set-Content $f $n
if ([int]$env:LQ_GAME_AT -gt 0 -and $n -ge [int]$env:LQ_GAME_AT) { exit 3 }
exit 0
'@
    # fake gh.ps1 behind gh.cmd: list prints $LQ_DIR\issues.json; comment/edit are logged, bodies kept.
    Set-Content (Join-Path $stub 'gh.cmd') "@echo off`r`npowershell -NoProfile -ExecutionPolicy Bypass -File `"%~dp0gh.ps1`" %*`r`nexit /b %ERRORLEVEL%"
    Set-Content -Encoding UTF8 (Join-Path $stub 'gh.ps1') @'
Add-Content (Join-Path $env:LQ_DIR 'gh.log') ($args -join ' ')
if ($args[0] -eq 'issue' -and $args[1] -eq 'list') { Get-Content -Raw (Join-Path $env:LQ_DIR 'issues.json'); exit 0 }
if ($args[0] -eq 'issue' -and $args[1] -eq 'comment') {
    $i = [array]::IndexOf($args, '-F'); Copy-Item $args[$i + 1] (Join-Path $env:LQ_DIR ("comment-$($args[2]).txt")); exit 0 }
exit 0
'@
    # fake wrapper: modes via $env:LQ_LLM = ok | long | hang | fail | game. Logs its args and the prompt.
    Set-Content -Encoding UTF8 (Join-Path $stub 'llm.ps1') @'
param([string[]]$PromptFile, [string[]]$OutFile, [string]$Model, [int]$TimeoutSec, [string]$Task)
Add-Content (Join-Path $env:LQ_DIR 'llm.log') "$Task|$Model|$TimeoutSec|$((Get-Process -Id $PID).PriorityClass)"
Copy-Item $PromptFile[0] (Join-Path $env:LQ_DIR ("prompt-" + [IO.Path]::GetFileName($PromptFile[0])))
switch ($env:LQ_LLM) {
    'hang' { Start-Sleep -Seconds 60 }
    'fail' { exit 1 }
    'game' { exit 3 }
    'long' { 1..40 | ForEach-Object { "line $_" } | Set-Content $OutFile[0]; exit 0 }
    default { "FAIL one: boom`nsecond line" | Set-Content $OutFile[0]; exit 0 }
}
'@
    Set-Content (Join-Path $stub 'lms.cmd') "@echo off`r`necho %* >> `"%LQ_DIR%\lms.log`"`r`nexit /b 0"
    $log = Join-Path $tmp 'logs'; New-Item -ItemType Directory $log | Out-Null
    $myLog = Join-Path $log 'gate.log'; Set-Content $myLog "test a ... ok`ntest b ... FAILED`n"
    $issuesFile = Join-Path $tmp 'issues.json'
    function Set-Issues($arr) { $arr | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $issuesFile }
    function Issue($n, $body, $labels = @('compute:local-llm')) { @{ number = $n; title = "t$n"; body = $body; labels = @($labels | ForEach-Object { @{ name = $_ } }) } }
    function Reset-State { Remove-Item (Join-Path $tmp '*.txt'), (Join-Path $tmp '*.log') -ErrorAction SilentlyContinue; $env:LQ_GAME_AT = '0'; $env:LQ_LLM = 'ok' }
    function Run-Queue([string[]]$More = @()) {
        $a = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $queue, '-OpsScripts', $stub, '-Gh', (Join-Path $stub 'gh.cmd'),
            '-LlmScript', (Join-Path $stub 'llm.ps1'), '-Lms', (Join-Path $stub 'lms.cmd'), '-AllowRoot', $log) + $More
        $script:out = & $ps @a 2>&1 | Out-String
        $script:code = $LASTEXITCODE
    }
    function Read-Log($n) { $p = Join-Path $tmp $n; if (Test-Path $p) { @(Get-Content $p) } else { @() } }

    # --- pure functions ---
    . $queue -OpsScripts $stub
    $s = Get-JobSpec ([pscustomobject]@{ number = 7; title = 'x'; body = "intro`r`ntype: log-triage`r`ninput: ``C:\a\b.log```r`nprompt: focus on rust"; labels = @(@{ name = 'compute:local-llm' }) })
    Check 'spec: type, input and prompt parsed' ($s.Type -eq 'log-triage' -and $s.Input -eq 'C:\a\b.log' -and $s.Prompt -eq 'focus on rust')
    Check 'skip: runnable job has no reason' (-not (Get-SkipReason $s))
    Check 'skip: local-review' ((Get-SkipReason ([pscustomobject]@{ Labels = @('local-review'); Type = 'log-triage'; Input = 'x' })) -match 'local-review')
    Check 'skip: needs-claude' ((Get-SkipReason ([pscustomobject]@{ Labels = @('needs-claude'); Type = 'log-triage'; Input = 'x' })) -match 'needs-claude')
    Check 'skip: unknown type' ((Get-SkipReason ([pscustomobject]@{ Labels = @(); Type = 'poem'; Input = 'x' })) -match 'unknown type')
    Check 'skip: log-triage needs input' ((Get-SkipReason ([pscustomobject]@{ Labels = @(); Type = 'log-triage'; Input = $null; Prompt = 'p' })) -match 'needs input')
    Check 'skip: other types need input or prompt' ((Get-SkipReason ([pscustomobject]@{ Labels = @(); Type = 'release-notes'; Input = $null; Prompt = $null })) -match 'needs input')
    Check 'path: inside root' ([bool](Resolve-AllowedInput 'C:\r\a\b.log' @('C:\r')))
    Check 'path: .. escape refused' (-not (Resolve-AllowedInput 'C:\r\..\x\b.log' @('C:\r')))
    Check 'path: sibling prefix refused' (-not (Resolve-AllowedInput 'C:\rr\b.log' @('C:\r')))
    $c = Format-Comment ((1..40 | ForEach-Object { "l$_" }) -join "`n") 'm1'
    Check 'comment: marker and at most 20 lines' (@($c -split "`n").Count -eq 20 -and $c.StartsWith('Local LLM draft (needs review): m1'))

    # --- queue list and skips ---
    Reset-State
    Set-Issues @(
        (Issue 1 "type: log-triage`ninput: $myLog"),
        (Issue 2 "type: log-triage`ninput: $myLog" @('compute:local-llm', 'local-review')),
        (Issue 3 "type: poem`nprompt: x"),
        (Issue 4 "type: log-triage`ninput: C:\Windows\win.ini"))
    Run-Queue @('-List')
    Check 'list: exit 0, runs nothing' ($code -eq 0 -and -not (Read-Log 'llm.log').Count -and -not ((Read-Log 'gh.log') -match 'comment|edit'))
    Check 'list: shows run and skips' ($out -match '#1 .*would run' -and $out -match '#2 .*local-review' -and $out -match "#3 .*unknown type 'poem'" -and $out -match '#4 .*outside')
    Reset-State
    Run-Queue @('-DryRun')
    Check 'dry run: nothing runs or posts' ($code -eq 0 -and -not (Read-Log 'llm.log').Count -and -not ((Read-Log 'gh.log') -match 'comment|edit') -and $out -match 'dry run: no game')

    # --- a real batch on the stubs ---
    Reset-State
    Run-Queue
    $gl = (Read-Log 'gh.log') -join "`n"
    Check 'batch: exit 0, one job posted, three skipped' ($code -eq 0 -and $out -match 'posted 1, skipped 3, failed 0')
    Check 'batch: reads the labelled open issues' ($gl -match 'issue list -R BanditF/foxglove -l compute:local-llm --state open --json number,title,body,labels')
    Check 'batch: comment then local-review label, no close' ($gl -match 'issue comment 1 ' -and $gl -match 'issue edit 1 .*--add-label local-review' -and $gl -notmatch 'close|merge')
    $cm = @(Read-Log 'comment-1.txt')
    Check 'batch: comment is marked and at most 20 lines' ($cm[0] -eq 'Local LLM draft (needs review): qwen3.6-35b-a3b' -and $cm.Count -le 20)
    Check 'batch: wrapper got task, model, BelowNormal priority' (@(Read-Log 'llm.log')[0] -match '^local-queue:log-triage\|qwen3\.6-35b-a3b\|300\|BelowNormal$')
    Check 'batch: prompt carries the log' ((Get-Content -Raw (Join-Path $tmp 'prompt-job-1.in.txt')) -match 'test b \.\.\. FAILED')
    Check 'batch: ops summary line written' ((Get-Content (Join-Path $env:FOXGLOVE_OPS_DIR 'summary.log')) -match 'local-queue ok')

    # --- long output is cut to 20 lines ---
    Reset-State; $env:LQ_LLM = 'long'
    Set-Issues @((Issue 5 "type: release-notes`nprompt: for 0.2"))
    Run-Queue
    Check 'long output: cut to 20 lines' ((Read-Log 'comment-5.txt').Count -eq 20)

    # --- game gate: at the start, and between jobs ---
    Reset-State; $env:LQ_GAME_AT = '1'
    Set-Issues @((Issue 1 "type: log-triage`ninput: $myLog"))
    Run-Queue
    Check 'game at start: exit 0, held, no model, no gh write' ($code -eq 0 -and $out -match 'held: game running' -and -not (Read-Log 'llm.log').Count -and -not ((Read-Log 'gh.log') -match 'comment|edit'))
    Reset-State; $env:LQ_GAME_AT = '3'   # start check = 1, before job 1 = 2, before job 2 = 3
    Set-Issues @((Issue 1 "type: log-triage`ninput: $myLog"), (Issue 2 "type: log-triage`ninput: $myLog"))
    Run-Queue
    Check 'game between jobs: first posted, second held' ($code -eq 0 -and $out -match 'held: game running' -and $out -match 'posted 1' -and (Read-Log 'llm.log').Count -eq 1)
    Reset-State; $env:LQ_LLM = 'game'
    Set-Issues @((Issue 1 "type: log-triage`ninput: $myLog"))
    Run-Queue
    Check 'wrapper exit 3: held, exit 0, nothing posted' ($code -eq 0 -and $out -match 'held: game running' -and -not ((Read-Log 'gh.log') -match 'comment'))

    # --- caps ---
    Reset-State
    Set-Issues @(1..4 | ForEach-Object { Issue $_ "type: log-triage`ninput: $myLog" })
    Run-Queue @('-MaxJobs', '2')
    Check 'max jobs: two run' ((Read-Log 'llm.log').Count -eq 2 -and $out -match 'max jobs')
    Reset-State; $env:LQ_LLM = 'hang'
    Set-Issues @((Issue 1 "type: log-triage`ninput: $myLog"))
    Run-Queue @('-JobTimeoutSec', '3', '-BatchTimeoutSec', '100')
    $lms = (Read-Log 'lms.log') -join "`n"
    Check 'job timeout: killed, exit 1, nothing posted' ($code -eq 1 -and $out -match 'killed at the 3 s cap' -and -not ((Read-Log 'gh.log') -match 'comment'))
    Check 'job timeout: model unloaded and server stopped' ($lms -match 'unload qwen3\.6-35b-a3b' -and $lms -match 'server stop')
    Reset-State; $env:LQ_LLM = 'fail'
    Run-Queue
    Check 'wrapper failure: exit 1, no comment, no label' ($code -eq 1 -and $out -match 'failed 1' -and -not ((Read-Log 'gh.log') -match 'comment|edit'))
}
finally {
    Remove-Item Env:FOXGLOVE_OPS_DIR, Env:LQ_DIR, Env:LQ_GAME_AT, Env:LQ_LLM, Env:LQ_SERVER_UP -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
if ($fails) { Write-Host "$fails local-queue test(s) failed"; exit 1 }
Write-Host 'all local-queue tests passed'
