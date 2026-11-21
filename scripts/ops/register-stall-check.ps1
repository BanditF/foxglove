# MIT. Registers the hourly 'Foxglove Stall Check' Task Scheduler task (#1126). Writing this file
# registers nothing; a person (or On-call) runs it once:
#   powershell -File scripts\ops\register-stall-check.ps1 [-Unregister] [-WhatIf]
# The task runs stall-check-wake.ps1 (stall-check.ps1 plus a Claude wake only when ction is not none) through wscript and a hidden-window launcher (stall-check-hidden.vbs,
# written next to this script), as the current user, 5 min limit, two bounded gh reads, no GUI.
# -WhatIf prints what would be registered and changes nothing.
param([string]$TaskName = 'Foxglove Stall Check', [int]$EveryMin = 60, [string]$ClaudeExe, [switch]$Unregister, [switch]$WhatIf)
$script = Join-Path $PSScriptRoot 'stall-check-wake.ps1'   # runs stall-check.ps1; wakes Claude only when action is not none
$q = '""'   # a quote inside a VBScript string
$extra = if ($ClaudeExe) { ' -ClaudeExe ' + $q + $ClaudeExe + $q } else { '' }
$launcher = Join-Path $PSScriptRoot 'stall-check-hidden.vbs'
$vbs = "' Runs stall-check-wake.ps1 with no window at all (Task Scheduler launches this through wscript).`r`n" +
       'CreateObject("WScript.Shell").Run "powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File ' +
       $q + $script + $q + ' -TaskName ' + $q + $TaskName + $q + $extra + '", 0, False' + "`r`n"
if ($Unregister) {
    if ($WhatIf) { Write-Host "would unregister '$TaskName'"; exit 0 }
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "unregistered '$TaskName'"; exit 0
}
if ($WhatIf) { Write-Host "would write $launcher and register '$TaskName' every $EveryMin min: wscript.exe `"$launcher`""; exit 0 }
Set-Content -Path $launcher -Value $vbs -Encoding ASCII
$action = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument "`"$launcher`""
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes $EveryMin)
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit (New-TimeSpan -Minutes 5) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Force `
    -Description 'Foxglove stall check (#1126): writes worktrees\_tools\stall-status.json, two bounded gh reads, no GUI.' | Out-Null
Write-Host "registered '$TaskName' every $EveryMin min"
