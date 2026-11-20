# Live proof of quiet-launch.ps1 (On-call runs this; it OPENS A SMALL WINDOW, on the hidden desktop).
# Windows PowerShell 5.1. Prints PASS/FAIL lines; exit 0 only when every check passes.
#
#   quiet-launch-proof.ps1 [-HoldSec 45]
#
# What it checks, with the user's default desktop as the thing that must not change:
#  1. the launcher starts a tiny WinForms window (titled quiet-launch-proof) on a private desktop
#  2. GetForegroundWindow on the default desktop is the same window before and after
#  3. EnumWindows on the default desktop shows no new top-level window (none owned by the child either)
#  4. UI Automation, run on the hidden desktop through -Run, sees the proof window there
#  5. -Stop tears down: the child is gone, the state file is gone, the desktop name no longer attaches
#  6. a launch with a relative exe path is refused (exit 3) and starts nothing
# The proof window closes itself after -HoldSec, and the launcher has its own hard limit, so nothing lingers.
param([int]$HoldSec = 45)
$ErrorActionPreference = 'Stop'
$launcher = Join-Path $PSScriptRoot 'quiet-launch.ps1'
$labDir = Join-Path $env:LOCALAPPDATA 'Foxglove\lab'
$psExe = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
$results = New-Object System.Collections.ArrayList
function Report([bool]$ok, [string]$what) {
    [void]$results.Add($ok)
    Write-Host ('{0}: {1}' -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $what)
}

Add-Type -TypeDefinition @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class ProofWin {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    public static long Foreground() { return GetForegroundWindow().ToInt64(); }
    // Visible top-level windows of the calling thread's desktop, as "hwnd:pid".
    public static string[] Visible() {
        List<string> l = new List<string>();
        EnumWindows(delegate (IntPtr h, IntPtr x) {
            if (IsWindowVisible(h)) { uint pid; GetWindowThreadProcessId(h, out pid); l.Add(h.ToInt64() + ":" + pid); }
            return true;
        }, IntPtr.Zero);
        return l.ToArray();
    }
}
'@

# 6 first: the refusal needs no window.
& $psExe -NoProfile -ExecutionPolicy Bypass -File $launcher -Exe 'notepad.exe' | Out-Null
Report ($LASTEXITCODE -eq 3) 'relative -Exe is refused (exit 3)'

New-Item -ItemType Directory -Force -Path $labDir | Out-Null
$name = 'ql-proof' + (Get-Random -Maximum 99999)
$ready = 'Local\quiet-launch-proof-' + $PID
$evt = New-Object System.Threading.EventWaitHandle($false, [System.Threading.EventResetMode]::ManualReset, $ready)
$out = Join-Path $labDir "quiet-launch-proof-$name.out"
Remove-Item -LiteralPath $out -Force -ErrorAction SilentlyContinue

$fgBefore = [ProofWin]::Foreground()
$winsBefore = [ProofWin]::Visible()

# The proof window: a small form that closes itself (a one-shot timer), nothing else.
$formCmd = "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = 'quiet-launch-proof'; `$f.Width = 240; `$f.Height = 120; " +
    "`$t = New-Object Windows.Forms.Timer; `$t.Interval = $($HoldSec * 1000); `$t.Add_Tick({ `$f.Close() }); `$t.Start(); [void]`$f.ShowDialog()"
$limit = $HoldSec + 30
# The supervisor runs hidden from an encoded command (an array parameter cannot be passed through -File).
$q = { param($s) "'" + $s.Replace("'", "''") + "'" }
$supText = "& $(& $q $launcher) -Exe $(& $q $psExe) -ArgList @('-NoProfile', '-Command', $(& $q $formCmd)) " +
    "-DesktopName $(& $q $name) -TimeoutSec $limit -Keep -ReadyEvent $(& $q $ready) -OutFile $(& $q $out); exit `$LASTEXITCODE"
$enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($supText))
$sup = Start-Process -FilePath $psExe -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $enc) -WindowStyle Hidden -PassThru
$gotReady = $evt.WaitOne(30000)
Report $gotReady 'the launcher reported ready (desktop created, child started)'
$childPid = 0
if ($gotReady) {
    $line = Get-Content -LiteralPath $out | Where-Object { $_ -like 'QUIET-LAUNCH*' } | Select-Object -First 1
    if ($line -match 'pid=(\d+)') { $childPid = [int]$Matches[1] }
    Report ($childPid -gt 0 -and $line -match "desktop=$name") "launch line has the desktop and pid: $line"

    # 4: UIA run on the hidden desktop sees the window (a bounded wait on UIA's own answer while the form shows).
    $uia = "Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes; `$c = New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ProcessIdProperty), {pid}; " +
        "`$n = 0; for (`$i = 0; `$i -lt 20 -and -not `$n; `$i++) { foreach (`$e in [Windows.Automation.AutomationElement]::RootElement.FindAll('Children', `$c)) { if (`$e.Current.Name -ceq 'quiet-launch-proof') { `$n++ } }; if (-not `$n) { Start-Sleep -Milliseconds 500 } }; " +
        "Write-Output ('UIA-WINDOWS ' + `$n); exit `$(if (`$n) { 0 } else { 2 })"
    $uiaOut = & $psExe -NoProfile -ExecutionPolicy Bypass -File $launcher -Desktop $name -Run $uia -RunTimeoutSec 30
    Report ($LASTEXITCODE -eq 0 -and ($uiaOut -join ' ') -match 'UIA-WINDOWS [1-9]') 'UIA on the hidden desktop sees the proof window'

    # 2 and 3: the default desktop did not change.
    $fgAfter = [ProofWin]::Foreground()
    Report ($fgAfter -eq $fgBefore) "foreground window on the default desktop unchanged ($fgBefore -> $fgAfter)"
    $winsAfter = [ProofWin]::Visible()
    $new = @($winsAfter | Where-Object { $winsBefore -notcontains $_ })
    Report ($new.Count -eq 0) "no new visible top-level window on the default desktop ($($new -join ', '))"
    $owned = @($winsAfter | Where-Object { $_ -like "*:$childPid" })
    Report ($owned.Count -eq 0) 'no default-desktop window belongs to the child'
}

# 5: teardown by -Stop.
& $psExe -NoProfile -ExecutionPolicy Bypass -File $launcher -Stop $name | Out-Null
$ended = $sup.WaitForExit(20000)
Report $ended 'the launcher ended after -Stop'
Report (-not (Test-Path -LiteralPath (Join-Path $labDir "quiet-launch-$name.json"))) 'state file removed'
$alive = $childPid -gt 0 -and [bool](Get-Process -Id $childPid -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -ceq 'powershell' })
Report (-not $alive) 'the child process is gone'
& $psExe -NoProfile -ExecutionPolicy Bypass -File $launcher -Desktop $name -Run 'exit 0' | Out-Null
Report ($LASTEXITCODE -eq 3) 'the torn-down desktop no longer attaches (refused)'
$evt.Dispose()
Remove-Item -LiteralPath $out -Force -ErrorAction SilentlyContinue

$bad = @($results | Where-Object { -not $_ }).Count
Write-Host ("quiet-launch proof: {0} of {1} checks passed" -f ($results.Count - $bad), $results.Count)
exit $(if ($bad) { 1 } else { 0 })
