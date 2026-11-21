# MIT. Window-free tests for scripts/ops/stall-check-wake.ps1: fake stall-check results (JSON files), a stub
# claude (a .cmd that records its arguments), -DryRun for the disable step. Nothing real is launched or changed.
#   powershell -File scripts\ops\test-stall-wake.ps1
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$fails = 0
function Check([string]$name, $cond) { if ($cond) { Write-Host "ok   $name" } else { Write-Host "FAIL $name"; $script:fails++ } }
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("stall-wake-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $tmp | Out-Null
$ps = (Get-Command powershell).Source
$s = Join-Path $here 'stall-check-wake.ps1'
$stub = Join-Path $tmp 'claude-stub.cmd'
$called = Join-Path $tmp 'called.txt'
Set-Content $stub "@echo %* > `"$called`"" -Encoding ASCII
function Run([string]$json, [switch]$Dry) {
    $f = Join-Path $tmp 'action.json'; Set-Content $f $json -Encoding UTF8
    Remove-Item $called -ErrorAction SilentlyContinue
    $a = @('-NoProfile', '-File', $s, '-ActionFile', $f, '-ClaudeExe', $stub, '-TaskName', 'No Such Foxglove Test Task')
    if ($Dry) { $a += '-DryRun' }
    & $ps @a 2>&1 | Out-String
}
try {
    . $s
    Check 'plan: none -> nothing' ($null -eq (Get-WakePlan ([pscustomobject]@{ action = 'none' })))
    $p = Get-WakePlan ([pscustomobject]@{ action = 'alarm'; alarm = [pscustomobject]@{ lines = @('a: 120 min', 'e: 40 GB') } })
    Check 'plan: alarm carries every line, no disable' ($p.message -eq "Stall check:`na: 120 min`ne: 40 GB" -and -not $p.disable)
    Check 'plan: freeze_lifted message' ((Get-WakePlan ([pscustomobject]@{ action = 'freeze_lifted' })).message -match 'freeze lifted')
    Check 'plan: pause disables the task' ((Get-WakePlan ([pscustomobject]@{ action = 'pause' })).disable)

    $o = Run '{"action":"none"}'
    Check 'none: exit 0, claude not called' ($LASTEXITCODE -eq 0 -and -not (Test-Path $called) -and $o -match 'no wake')
    $o = Run '{"action":"alarm","alarm":{"lines":["a: 120 min"]}}'
    Check 'alarm: claude called once with the Hub id and the text' ((Test-Path $called) -and ((Get-Content $called -Raw) -match 'local_a0a18f0e') -and ((Get-Content $called -Raw) -match 'SendMessage'))
    $o = Run '{"action":"pause"}' -Dry
    Check 'pause (dry): says it would disable, claude not called' ($o -match 'DRYRUN would disable' -and -not (Test-Path $called))
    $o = Run '{"action":"freeze_lifted"}' -Dry
    Check 'freeze_lifted (dry): no disable' ($o -notmatch 'would disable' -and $o -match 'freeze lifted')
    $o = Run 'not json at all'
    Check 'garbage: exit 0, nothing called' ($LASTEXITCODE -eq 0 -and -not (Test-Path $called))

    # Launcher text: register -WhatIf changes nothing and the script parses.
    $r = Join-Path $here 'register-stall-check.ps1'
    $err = $null; [void][System.Management.Automation.Language.Parser]::ParseFile($r, [ref]$null, [ref]$err)
    Check 'register script parses' (-not $err)
    $w = & $ps -NoProfile -File $r -WhatIf 2>&1 | Out-String
    Check 'register -WhatIf names the launcher' ($w -match 'would write .*stall-check-hidden\.vbs')
}
finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
if ($fails) { Write-Host "$fails FAILED"; exit 1 }
Write-Host 'all passed'
