# MIT. PC health check as a script (#949): the numbers of the former Claude task, plus leak rates
# per standby hour, appended to CSVs under %LOCALAPPDATA%\Foxglove\ops\. Thresholds are flagged
# here; no LLM. Read-only: it restarts nothing (the Explorer Watchdog task owns that).
#   powershell -File scripts\ops\pc-health-check.ps1 [-TimeLimitSec 120]
param([int]$TimeLimitSec = 120)
. (Join-Path $PSScriptRoot 'ops-common.ps1')

$HealthHeader = 'time,explorer_gdi,explorer_user,explorer_handles,dwm_handles,nvcontainer_max_handles,nvdisplay_handles,nonpaged_mb,paged_mb,commit_mb,free_ram_gb,c_free_gb,top5_private_mb,claude,cargo,rustc,modern_standby,standby_event_time'
$StretchHeader = 'from,to,standby_h,awake_h,gdi_per_standby_h,nv_per_standby_h,nvdisplay_per_standby_h,nonpaged_mb_per_standby_h,reset'
# Normal ranges per standby hour (measured Oct 25-28): the top of each. Flag above twice the top.
$NormalTop = @{ gdi = 1500; nv = 20000; nvdisplay = 3000; nonpaged = 40 }

# Minutes in Modern Standby between $From and $To. $Events: objects with Time and Id (506 enter,
# 507 exit), any order; $StartInStandby: state at $From.
function Get-StandbyMinutes {
    param([object[]]$Events, [datetime]$From, [datetime]$To, [bool]$StartInStandby = $false)
    $inS = $StartInStandby; $cur = $From; $mins = 0.0
    foreach ($e in ($Events | Where-Object { $_.Time -gt $From -and $_.Time -lt $To } | Sort-Object Time)) {
        if ($inS -and $e.Id -eq 507) { $mins += ($e.Time - $cur).TotalMinutes; $inS = $false }
        elseif (-not $inS -and $e.Id -eq 506) { $inS = $true; $cur = $e.Time }
    }
    if ($inS) { $mins += ($To - $cur).TotalMinutes }
    [math]::Round($mins, 1)
}

# The stretches row for two health rows (objects from the CSV) and the standby minutes between.
function Get-LeakRow {
    param($Prev, $Now, [double]$StandbyMin, [bool]$Rebooted = $false)
    $span = ([datetime]$Now.time - [datetime]$Prev.time).TotalHours
    $sh = [math]::Round($StandbyMin / 60, 2)
    $reset = ''
    if ($Rebooted) { $reset = 'reboot' }
    elseif ([double]$Now.explorer_gdi -lt [double]$Prev.explorer_gdi / 2) { $reset = 'explorer' }
    $rate = {
        param($a, $b)
        if ($sh -lt 0.5 -or $reset) { return '' }
        [math]::Round(([double]$b - [double]$a) / $sh, 1)
    }
    [pscustomobject][ordered]@{
        from = $Prev.time; to = $Now.time; standby_h = $sh; awake_h = [math]::Round($span - $sh, 2)
        gdi_per_standby_h = & $rate $Prev.explorer_gdi $Now.explorer_gdi
        nv_per_standby_h = & $rate $Prev.nvcontainer_max_handles $Now.nvcontainer_max_handles
        nvdisplay_per_standby_h = & $rate $Prev.nvdisplay_handles $Now.nvdisplay_handles
        nonpaged_mb_per_standby_h = & $rate $Prev.nonpaged_mb $Now.nonpaged_mb
        reset = $reset
    }
}

# Threshold flags: array of short strings. $Prev and $Leak may be null.
function Get-HealthFlags {
    param($Now, $Prev, $Leak)
    $f = @()
    if ([double]$Now.explorer_gdi -gt 7000) { $f += "taskbar at risk: Explorer GDI $($Now.explorer_gdi)" }
    if ($Now.modern_standby -eq 'standby' -and $Leak -and "$($Leak.gdi_per_standby_h)" -ne '' -and [double]$Leak.gdi_per_standby_h -gt 0) {
        $proj = [double]$Now.explorer_gdi + 6 * [double]$Leak.gdi_per_standby_h
        if ($proj -ge 10000) { $f += "GDI projected $([int]$proj) in 6 h" }
    }
    if ([double]$Now.nonpaged_mb -gt 3000 -or [double]$Now.nvcontainer_max_handles -gt 500000) { $f += 'reboot suggested' }
    if ($Leak) {
        foreach ($k in @(@('gdi', 'gdi_per_standby_h'), @('nv', 'nv_per_standby_h'), @('nvdisplay', 'nvdisplay_per_standby_h'), @('nonpaged', 'nonpaged_mb_per_standby_h'))) {
            $v = "$($Leak.($k[1]))"
            if ($v -ne '' -and [double]$v -gt 2 * $NormalTop[$k[0]]) { $f += "new pattern: $($k[1]) $v" }
        }
        if ("$($Leak.reset)" -eq '' -and [double]$Leak.standby_h -lt 0.25 -and $Prev) {
            if (([double]$Now.explorer_gdi - [double]$Prev.explorer_gdi) -gt 500 -or ([double]$Now.nvcontainer_max_handles - [double]$Prev.nvcontainer_max_handles) -gt 5000) { $f += 'growth while awake' }
        }
    }
    if ([double]$Now.free_ram_gb -lt 8) { $f += "free RAM $($Now.free_ram_gb) GB" }
    if ([double]$Now.c_free_gb -lt 150) { $f += "C: free $($Now.c_free_gb) GB" }
    $f
}

function Get-GuiCounts {
    Add-Type -ErrorAction SilentlyContinue -Namespace OpsNative -Name Gui -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetGuiResources(System.IntPtr h, uint f);
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern System.IntPtr OpenProcess(uint a, bool i, uint pid);
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern bool CloseHandle(System.IntPtr h);
'@
    $p = Get-Process explorer -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $p) { return @(0, 0) }
    $h = [OpsNative.Gui]::OpenProcess(0x1000, $false, [uint32]$p.Id)
    $r = @([OpsNative.Gui]::GetGuiResources($h, 0), [OpsNative.Gui]::GetGuiResources($h, 1))
    [void][OpsNative.Gui]::CloseHandle($h)
    $r
}

function Get-StandbyEvents([datetime]$Since) {
    try {
        Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Microsoft-Windows-Kernel-Power'; Id = 506, 507; StartTime = $Since } -ErrorAction Stop |
            ForEach-Object { [pscustomobject]@{ Time = $_.TimeCreated; Id = $_.Id } }
    } catch { @() }
}

function Measure-Now {
    $gui = Get-GuiCounts
    $hc = { param($n) $x = Get-Process $n -ErrorAction SilentlyContinue; if ($x) { ($x | Measure-Object HandleCount -Maximum).Maximum } else { 0 } }
    $mem = Get-CimInstance Win32_PerfRawData_PerfOS_Memory
    $os = Get-CimInstance Win32_OperatingSystem
    $top = Get-Process | Sort-Object PrivateMemorySize64 -Descending | Select-Object -First 5 |
        ForEach-Object { '{0}:{1}' -f $_.ProcessName, [int]($_.PrivateMemorySize64 / 1MB) }
    $last = Get-StandbyEvents (Get-Date).AddDays(-3) | Sort-Object Time | Select-Object -Last 1
    $state = if ($last -and $last.Id -eq 506) { 'standby' } else { 'awake' }
    [pscustomobject][ordered]@{
        time = Get-Date -Format s
        explorer_gdi = $gui[0]; explorer_user = $gui[1]
        explorer_handles = & $hc 'explorer'; dwm_handles = & $hc 'dwm'
        nvcontainer_max_handles = & $hc 'nvcontainer'; nvdisplay_handles = & $hc 'NVDisplay.Container'
        nonpaged_mb = [int]($mem.PoolNonpagedBytes / 1MB); paged_mb = [int]($mem.PoolPagedBytes / 1MB)
        commit_mb = [int]($mem.CommittedBytes / 1MB)
        free_ram_gb = [math]::Round($os.FreePhysicalMemory / 1MB, 2)
        c_free_gb = [math]::Round((Get-PSDrive C).Free / 1GB, 1)
        top5_private_mb = $top -join ';'
        claude = @(Get-Process claude -ErrorAction SilentlyContinue).Count
        cargo = @(Get-Process cargo -ErrorAction SilentlyContinue).Count
        rustc = @(Get-Process rustc -ErrorAction SilentlyContinue).Count
        modern_standby = $state
        standby_event_time = if ($last) { $last.Time.ToString('s') } else { '' }
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    Start-OpsJob -Name 'pc-health-check' -TimeLimitSec $TimeLimitSec
    $dir = Get-OpsDir
    $healthCsv = Join-Path $dir 'pc-health-log.csv'
    $stretchCsv = Join-Path $dir 'standby-stretches.csv'
    $prev = $null
    if (Test-Path $healthCsv) { $prev = Import-Csv $healthCsv | Select-Object -Last 1 }
    $now = Measure-Now
    if (-not (Test-Path $healthCsv)) { Set-Content -Encoding UTF8 $healthCsv $HealthHeader }
    Add-Content -Encoding UTF8 $healthCsv (($now | ConvertTo-Csv -NoTypeInformation | Select-Object -Skip 1))
    $leak = $null
    if ($prev) {
        $from = [datetime]$prev.time
        $ev = @(Get-StandbyEvents $from.AddDays(-3))
        $before = $ev | Where-Object { $_.Time -le $from } | Sort-Object Time | Select-Object -Last 1
        $mins = Get-StandbyMinutes -Events $ev -From $from -To ([datetime]$now.time) -StartInStandby ([bool]($before -and $before.Id -eq 506))
        $boot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
        $leak = Get-LeakRow -Prev $prev -Now $now -StandbyMin $mins -Rebooted ($boot -gt $from)
        if (-not (Test-Path $stretchCsv)) { Set-Content -Encoding UTF8 $stretchCsv $StretchHeader }
        Add-Content -Encoding UTF8 $stretchCsv (($leak | ConvertTo-Csv -NoTypeInformation | Select-Object -Skip 1))
    }
    $flags = @(Get-HealthFlags -Now $now -Prev $prev -Leak $leak)
    $rate = if ($leak -and "$($leak.gdi_per_standby_h)" -ne '') { ", GDI $($leak.gdi_per_standby_h)/standby h" } else { '' }
    $text = "GDI $($now.explorer_gdi), nonpaged $($now.nonpaged_mb) MB, nv $($now.nvcontainer_max_handles), RAM free $($now.free_ram_gb) GB, C: $($now.c_free_gb) GB$rate"
    if ($flags.Count) { Stop-OpsJob -Status 'FLAG' -Text ("$text | " + ($flags -join '; ')) -Code 1 }
    Stop-OpsJob -Status 'ok' -Text $text
}
