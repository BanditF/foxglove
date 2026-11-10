# MIT. The per-flash Logi leak probe (#313, #949) as a script. Run it once just BEFORE and once
# just AFTER a flash or USB replug: one read-only sample of LogiOptionsMgr private memory each
# time, no polling, never touches the process. The "after" run prints the delta line to post on
# #313 (it posts nothing itself). Samples go to ops\logi-probe.csv.
#   powershell -File scripts\ops\logi-leak-probe.ps1 -Phase before -Event "flash left"
#   powershell -File scripts\ops\logi-leak-probe.ps1 -Phase after  -Event "flash left"
param(
    [Parameter(Mandatory)][ValidateSet('before', 'after')][string]$Phase,
    [string]$Event = 'unspecified',
    [int]$TimeLimitSec = 30
)
. (Join-Path $PSScriptRoot 'ops-common.ps1')

# Pure: the post line for a before/after pair of byte counts.
function Format-LogiDelta([string]$Event, $Before, $After, [datetime]$T0, [datetime]$T1) {
    $d = [int64]$After - [int64]$Before
    '{0}: LogiOptionsMgr private memory {1} -> {2} bytes (delta {3:+#,0;-#,0;0}); {4:u} -> {5:u}' -f $Event, $Before, $After, $d, $T0.ToUniversalTime(), $T1.ToUniversalTime()
}

if ($MyInvocation.InvocationName -ne '.') {
    # A flash is a user-approved event, and the probe is one cheap read; a game does not block it.
    Start-OpsJob -Name 'logi-leak-probe' -TimeLimitSec $TimeLimitSec -SkipGameCheck
    $csv = Join-Path (Get-OpsDir) 'logi-probe.csv'
    $p = Get-Process LogiOptionsMgr -ErrorAction SilentlyContinue
    $bytes = if ($p) { ($p | Measure-Object PrivateMemorySize64 -Sum).Sum } else { -1 }
    if (-not (Test-Path $csv)) { Set-Content -Encoding UTF8 $csv 'time,phase,event,private_bytes' }
    $now = Get-Date
    Add-Content -Encoding UTF8 $csv ('{0},{1},"{2}",{3}' -f $now.ToString('s'), $Phase, $Event, $bytes)
    if ($bytes -lt 0) { Stop-OpsJob -Status 'warn' -Text 'LogiOptionsMgr is not running' -Code 1 }
    if ($Phase -eq 'before') { Stop-OpsJob -Status 'ok' -Text "before $Event : $bytes bytes" }
    $b = Import-Csv $csv | Where-Object { $_.phase -eq 'before' -and $_.event -eq $Event -and [int64]$_.private_bytes -ge 0 } | Select-Object -Last 1
    if (-not $b) { Stop-OpsJob -Status 'warn' -Text "after $Event : $bytes bytes, no matching before sample" -Code 1 }
    Stop-OpsJob -Status 'ok' -Text (Format-LogiDelta $Event $b.private_bytes $bytes ([datetime]$b.time) $now)
}
