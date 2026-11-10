# MIT. Monthly driver check as a script (#949): installed BIOS, GPU, Intel/Realtek/NVIDIA driver
# and Windows versions, plus `winget upgrade` output, written to ops\driver-check-<yyyy-mm>.md.
# No LLM, no web lookups by the script itself (winget talks to its own sources). Installs nothing.
#   powershell -File scripts\ops\monthly-driver-check.ps1 [-TimeLimitSec 300]
param([int]$TimeLimitSec = 300, [string]$OutFile)
. (Join-Path $PSScriptRoot 'ops-common.ps1')

# Pure: the number of upgradable packages in `winget upgrade` text (rows between the dashes line
# and the "N upgrades available" footer).
function Get-WingetUpgradeCount([string]$Text) {
    $lines = $Text -split "`r?`n"
    $sep = -1
    for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^-{5,}\s*$') { $sep = $i; break } }
    if ($sep -lt 0) { return 0 }
    $n = 0
    for ($i = $sep + 1; $i -lt $lines.Count; $i++) {
        $l = $lines[$i]
        if (-not $l.Trim() -or $l -match 'upgrades? available|explicit targeting') { break }
        $n++
    }
    $n
}

function Format-DriverReport {
    param($Bios, $Gpus, $Drivers, $Os, [string]$Winget, [int]$UpgradeCount, [datetime]$Now = (Get-Date))
    $o = New-Object System.Collections.Generic.List[string]
    $o.Add("# Driver check $($Now.ToString('yyyy-MM-dd'))")
    $o.Add('')
    $o.Add('## BIOS')
    $o.Add("$($Bios.SMBIOSBIOSVersion), released $($Bios.ReleaseDate)")
    $o.Add('')
    $o.Add('## Windows')
    $o.Add("$($Os.Caption) build $($Os.BuildNumber).$($Os.UBR) ($($Os.Version))")
    $o.Add('')
    $o.Add('## GPUs')
    foreach ($g in $Gpus) { $o.Add("- $($g.Name): $($g.DriverVersion), $($g.DriverDate)") }
    $o.Add('')
    $o.Add('## Intel / Realtek / NVIDIA drivers')
    foreach ($d in $Drivers) { $o.Add("- $($d.DeviceName): $($d.DriverVersion), $($d.DriverDate)") }
    $o.Add('')
    $o.Add("## winget upgrade ($UpgradeCount available)")
    $o.Add('```')
    $o.Add($Winget.Trim())
    $o.Add('```')
    $o.Add('')
    $o.Add('Update in plain words: NVIDIA via the NVIDIA app; Intel via Intel Driver and Support Assistant; BIOS from HP support (do not interrupt it); the rest via Windows Update > Optional updates. This script installs nothing.')
    $o -join "`n"
}

if ($MyInvocation.InvocationName -ne '.') {
    Start-OpsJob -Name 'monthly-driver-check' -TimeLimitSec $TimeLimitSec
    $bios = Get-CimInstance Win32_BIOS
    $gpus = Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, @{ n = 'DriverDate'; e = { $_.DriverDate.ToString('yyyy-MM-dd') } }
    $drivers = Get-CimInstance Win32_PnPSignedDriver | Where-Object { $_.DeviceName -and $_.Manufacturer -match 'Intel|Realtek|NVIDIA' } |
        Sort-Object DeviceName | Select-Object DeviceName, DriverVersion, @{ n = 'DriverDate'; e = { if ($_.DriverDate) { $_.DriverDate.ToString('yyyy-MM-dd') } } }
    $cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    $os = [pscustomobject]@{ Caption = (Get-CimInstance Win32_OperatingSystem).Caption; BuildNumber = $cv.CurrentBuildNumber; UBR = $cv.UBR; Version = $cv.DisplayVersion }
    $w = Invoke-Bounded -File 'winget' -Arguments @('upgrade', '--disable-interactivity') -TimeoutSec 120
    $wt = if ($w.Code -eq -1) { '(winget timed out)' } else { [string]$w.Out }
    $count = Get-WingetUpgradeCount $wt
    $out = if ($OutFile) { $OutFile } else { Join-Path (Get-OpsDir) ("driver-check-{0}.md" -f (Get-Date -Format 'yyyy-MM')) }
    Set-Content -Encoding UTF8 $out (Format-DriverReport -Bios $bios -Gpus $gpus -Drivers $drivers -Os $os -Winget $wt -UpgradeCount $count)
    $gpuTxt = ($gpus | ForEach-Object { "$($_.Name) $($_.DriverVersion)" }) -join ', '
    $status = if ($count -gt 0) { 'FLAG' } else { 'ok' }
    Stop-OpsJob -Status $status -Text "BIOS $($bios.SMBIOSBIOSVersion), $gpuTxt, Windows $($os.BuildNumber).$($os.UBR), $count winget upgrades; report $out" -Code $(if ($count -gt 0) { 1 } else { 0 })
}
