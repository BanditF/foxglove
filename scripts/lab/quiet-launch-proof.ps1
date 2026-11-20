# Live proof of quiet-launch.ps1 (On-call runs this; it OPENS A SMALL WINDOW, on the hidden desktop).
# Windows PowerShell 5.1. Prints PASS/FAIL/INFO lines; exit 0 only when no check FAILs.
#
#   quiet-launch-proof.ps1 [-HoldSec 45]
#
# Run it from the user's own interactive session (a normal hidden On-call runner), never through
# quiet-launch itself. What it checks:
#  0. this script runs on the user's default desktop (thread desktop = "Default" = the input desktop);
#     otherwise every later check would test nothing
#  1. the launcher starts a tiny WinForms window (titled quiet-launch-proof) plus a console grandchild
#     (cmd /c ping) on a private desktop
#  2. during the whole run, an out-of-context WinEvent hook on the default desktop records every
#     foreground change and every top-level window shown. FAIL only on events from the launch tree
#     (the supervisor, the child, their descendants, the -Run helper); events from any other process
#     (the user, other agents) are INFO, never a failure
#  3. UI Automation, run on the hidden desktop through -Run, sees the proof window there
#     (also proves the child's window is not SW_HIDE-hidden from UIA)
#  4. -Stop tears down: launcher ended, state file gone, child gone, desktop name no longer attaches
#  5. a relative exe path is refused (exit 3); an already-running image is refused (exit 3)
# The proof window closes itself after -HoldSec, and the launcher has its own hard limit.
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
function Info([string]$what) { Write-Host "INFO: $what" }

Add-Type -TypeDefinition @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text; using System.Threading;
public static class ProofWin {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    delegate void HookProc(IntPtr hook, uint ev, IntPtr hwnd, int idObject, int idChild, uint thread, uint time);
    [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr h; public uint m; public UIntPtr w; public IntPtr l; public uint t; public int x, y; }
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint f);
    [DllImport("user32.dll")] static extern IntPtr GetThreadDesktop(uint tid);
    [DllImport("user32.dll")] static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr d);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern bool GetUserObjectInformationW(IntPtr h, int idx, StringBuilder buf, int len, out int needed);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] static extern IntPtr SetWinEventHook(uint min, uint max, IntPtr mod, HookProc p, uint pid, uint tid, uint flags);
    [DllImport("user32.dll")] static extern bool UnhookWinEvent(IntPtr h);
    [DllImport("user32.dll")] static extern int GetMessage(out MSG m, IntPtr h, uint a, uint b);
    [DllImport("user32.dll")] static extern bool PostThreadMessage(uint tid, uint msg, UIntPtr w, IntPtr l);

    public static long Foreground() { return GetForegroundWindow().ToInt64(); }
    public static int ForegroundPid() { uint pid; GetWindowThreadProcessId(GetForegroundWindow(), out pid); return (int)pid; }
    static string Name(IntPtr d) { StringBuilder sb = new StringBuilder(256); int n; GetUserObjectInformationW(d, 2, sb, 512, out n); return sb.ToString(); }
    public static string ThreadDesktopName() { return Name(GetThreadDesktop(GetCurrentThreadId())); }
    public static string InputDesktopName() {
        IntPtr d = OpenInputDesktop(0, false, 0x1); if (d == IntPtr.Zero) return "";
        string n = Name(d); CloseDesktop(d); return n;
    }
    public static string[] Visible() {
        List<string> l = new List<string>();
        EnumWindows(delegate (IntPtr h, IntPtr x) {
            if (IsWindowVisible(h)) { uint pid; GetWindowThreadProcessId(h, out pid); l.Add(h.ToInt64() + ":" + pid); }
            return true;
        }, IntPtr.Zero);
        return l.ToArray();
    }

    // Event log of the default desktop: "fg:<pid>" for each foreground change, "show:<pid>" for each top-level window shown.
    static readonly List<string> log = new List<string>();
    static Thread th; static uint tid; static HookProc keep;
    public static void StartHooks() {
        ManualResetEvent up = new ManualResetEvent(false);
        keep = delegate (IntPtr hook, uint ev, IntPtr hwnd, int idObject, int idChild, uint thread, uint time) {
            uint pid; GetWindowThreadProcessId(hwnd, out pid);
            if (ev == 0x3) { lock (log) log.Add("fg:" + pid); }
            else if (ev == 0x8002 && idObject == 0 && idChild == 0 && GetAncestor(hwnd, 2) == hwnd) { lock (log) log.Add("show:" + pid); }
        };
        th = new Thread(delegate () {
            tid = GetCurrentThreadId();
            IntPtr a = SetWinEventHook(0x3, 0x3, IntPtr.Zero, keep, 0, 0, 0);
            IntPtr b = SetWinEventHook(0x8002, 0x8002, IntPtr.Zero, keep, 0, 0, 0);
            up.Set();
            MSG m; while (GetMessage(out m, IntPtr.Zero, 0, 0) > 0) { }
            UnhookWinEvent(a); UnhookWinEvent(b);
        });
        th.IsBackground = true; th.Start(); up.WaitOne(5000);
    }
    public static string[] StopHooks() {
        PostThreadMessage(tid, 0x12, UIntPtr.Zero, IntPtr.Zero); // WM_QUIT
        th.Join(5000);
        lock (log) return log.ToArray();
    }
}
'@

# 0: the default desktop, or nothing below means anything.
$threadDesk = [ProofWin]::ThreadDesktopName()
$inputDesk = [ProofWin]::InputDesktopName()
Report ($threadDesk -ieq 'Default' -and $inputDesk -ieq 'Default') "the proof runs on the user's default desktop (thread '$threadDesk', input '$inputDesk')"
if ($results[0] -ne $true) { Write-Host 'quiet-launch proof: stopped (not on the default desktop)'; exit 1 }

# 5a: refusals need no window.
& $psExe -NoProfile -ExecutionPolicy Bypass -File $launcher -Exe 'notepad.exe' | Out-Null
Report ($LASTEXITCODE -eq 3) 'relative -Exe is refused (exit 3)'
# This very powershell is running, so launching powershell.exe without -AllowRunning must be refused.
& $psExe -NoProfile -ExecutionPolicy Bypass -File $launcher -Exe $psExe | Out-Null
Report ($LASTEXITCODE -eq 3) 'an image that is already running is refused (exit 3)'

New-Item -ItemType Directory -Force -Path $labDir | Out-Null
$name = 'ql-proof' + (Get-Random -Maximum 99999)
$ready = 'Local\quiet-launch-proof-' + $PID
$evt = New-Object System.Threading.EventWaitHandle($false, [System.Threading.EventResetMode]::ManualReset, $ready)
$out = Join-Path $labDir "quiet-launch-proof-$name.out"
Remove-Item -LiteralPath $out -Force -ErrorAction SilentlyContinue

# The proof window: a small form that closes itself (a one-shot timer), after a console grandchild runs.
$formCmd = "& `$env:SystemRoot\System32\cmd.exe /c ping -n 3 127.0.0.1 | Out-Null; " +
    "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = 'quiet-launch-proof'; `$f.Width = 240; `$f.Height = 120; " +
    "`$t = New-Object Windows.Forms.Timer; `$t.Interval = $($HoldSec * 1000); `$t.Add_Tick({ `$f.Close() }); `$t.Start(); [void]`$f.ShowDialog()"
$limit = $HoldSec + 30
$q = { param($s) "'" + $s.Replace("'", "''") + "'" }
$supText = "& $(& $q $launcher) -Exe $(& $q $psExe) -AllowRunning -ArgList @('-NoProfile', '-Command', $(& $q $formCmd)) " +
    "-DesktopName $(& $q $name) -TimeoutSec $limit -Keep -ReadyEvent $(& $q $ready) -OutFile $(& $q $out); exit `$LASTEXITCODE"
$enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($supText))

$childPid = 0; $sup = $null; $treePids = @{}
try {
    [ProofWin]::StartHooks()
    $fgBefore = [ProofWin]::Foreground()
    $winsBefore = [ProofWin]::Visible()

    # The supervisor starts with CreateProcess (no shell, no console): UseShellExecute=false + CreateNoWindow.
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $psExe
    $psi.Arguments = "-NoProfile -ExecutionPolicy Bypass -EncodedCommand $enc"
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.WindowStyle = 'Hidden'
    $sup = [Diagnostics.Process]::Start($psi)
    $treePids[$sup.Id] = 'supervisor'

    $gotReady = $evt.WaitOne(30000)
    Report $gotReady 'the launcher reported ready (desktop created, child started)'
    if ($gotReady) {
        $line = $null
        if (Test-Path -LiteralPath $out) { $line = Get-Content -LiteralPath $out | Where-Object { $_ -like 'QUIET-LAUNCH*' } | Select-Object -First 1 }
        if ($line -match 'pid=(\d+)') { $childPid = [int]$Matches[1]; $treePids[$childPid] = 'child' }
        Report ($childPid -gt 0 -and $line -match "desktop=$name") "launch line has the desktop and pid: $line"

        # 3: UIA on the hidden desktop sees the window (bounded wait on UIA's own answer while the form comes up).
        $uia = "Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes; `$c = New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ProcessIdProperty), {pid}; " +
            "`$n = 0; for (`$i = 0; `$i -lt 30 -and -not `$n; `$i++) { foreach (`$e in [Windows.Automation.AutomationElement]::RootElement.FindAll('Children', `$c)) { if (`$e.Current.Name -ceq 'quiet-launch-proof') { `$n++ } }; if (-not `$n) { Start-Sleep -Milliseconds 500 } }; " +
            "Write-Output ('UIA-WINDOWS ' + `$n); exit `$(if (`$n) { 0 } else { 2 })"
        $uiaOut = & $psExe -NoProfile -ExecutionPolicy Bypass -File $launcher -Desktop $name -Run $uia -RunTimeoutSec 40
        Report ($LASTEXITCODE -eq 0 -and ($uiaOut -join ' ') -match 'UIA-WINDOWS [1-9]') 'UIA on the hidden desktop sees the proof window'

        # The launch tree now: everything below the supervisor (child, console grandchild, -Run helper if still alive).
        $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId)
        $queue = New-Object System.Collections.Queue; $queue.Enqueue($sup.Id)
        while ($queue.Count) {
            $p = $queue.Dequeue()
            foreach ($c in ($all | Where-Object { $_.ParentProcessId -eq $p })) { if (-not $treePids.ContainsKey([int]$c.ProcessId)) { $treePids[[int]$c.ProcessId] = 'descendant' }; $queue.Enqueue($c.ProcessId) }
        }
    }
}
finally {
    # Stop first, always, so the launcher never lingers; then read the hook log.
    & $psExe -NoProfile -ExecutionPolicy Bypass -File $launcher -Stop $name | Out-Null
    $ended = $true
    if ($sup) { $ended = $sup.WaitForExit(20000) }
    $events = [ProofWin]::StopHooks()
}

# 2: the events of the whole run on the default desktop.
$fgAfter = [ProofWin]::Foreground()
$mine = @($events | Where-Object { $treePids.ContainsKey([int]($_.Split(':')[1])) })
$others = @($events | Where-Object { -not $treePids.ContainsKey([int]($_.Split(':')[1])) })
Report ($mine.Count -eq 0) "no foreground change or window shown by the launch tree on the default desktop ($($mine -join ', '); tree pids $(($treePids.Keys | Sort-Object) -join ','))"
if ($others.Count) { Info "$($others.Count) event(s) from other processes (not the launch tree), ignored: $(($others | Select-Object -Unique) -join ', ')" }
$winsAfter = [ProofWin]::Visible()
$ownedNow = @($winsAfter | Where-Object { $treePids.ContainsKey([int]($_.Split(':')[1])) })
Report ($ownedNow.Count -eq 0) 'no default-desktop window belongs to the launch tree at the end'
if ($fgAfter -ne $fgBefore) { Info "foreground differs from the start ($fgBefore -> $fgAfter); it is a failure only when it was the launch tree (see above)" }
Report ($treePids.Keys -notcontains [ProofWin]::ForegroundPid()) 'the foreground window at the end is not the launch tree'
$new = @($winsAfter | Where-Object { $winsBefore -notcontains $_ -and -not $treePids.ContainsKey([int]($_.Split(':')[1])) })
if ($new.Count) { Info "new windows from other processes during the run: $($new -join ', ')" }

# 4: teardown.
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
