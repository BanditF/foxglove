# Quiet launcher: starts a program on a PRIVATE HIDDEN DESKTOP so it can never take focus or show a
# window on the user's default desktop (the user may be gaming). Windows PowerShell 5.1, P/Invoke only.
#
#   quiet-launch.ps1 -Exe C:\path\app.exe [-ArgList a,b] [-WorkDir C:\dir] [-Env @{FOXGLOVE_CONFIG_DIR='C:\lab\cfg'}]
#                    [-TimeoutSec 120] [-Run 'uia_tree.ps1 -ProcessId {pid} ...'] [-Keep]
#                    [-RunTimeoutSec 60] [-DesktopName ql-abc123] [-ReadyEvent 'Local\quiet-launch-x'] [-OutFile f.txt]
#   quiet-launch.ps1 -Desktop ql-abc123 -Run '<command>'     attach: run a command on a live launch's desktop
#   quiet-launch.ps1 -Stop ql-abc123                         ask a live launch to tear down now
#   quiet-launch.ps1 -SelfTest                               pure parts only: no desktop, no process
#
# What it does: creates a desktop (CreateDesktopW), starts -Exe on it (STARTUPINFO.lpDesktop =
# winsta0\<name>) inside a kill-on-close job object, prints one line
#     QUIET-LAUNCH desktop=<name> pid=<pid> image=<exe leaf> timeout=<s>
# and stays resident as the supervisor. It ends when: the child exits, -TimeoutSec passes (a hard
# limit, default 120, max 3600), -Stop is asked, or (with -Run and no -Keep) the -Run commands are done.
#
# -Run (repeatable) is a PowerShell command line run in a hidden powershell.exe STARTED ON THE SAME
# DESKTOP, so a UI Automation client (uia_tree.ps1) sees that desktop's windows: a UIA client sees
# only the desktop of the thread it runs on, and this puts the whole process there. {pid} and
# {desktop} in the text are replaced. Its output is printed here, its exit code kept. Each run is
# limited by -RunTimeoutSec and by the overall limit.
#
# Teardown: TerminateJobObject (the child's whole process tree, by the job we created: exact, no
# names, no wildcards), then wait for the child's handle, then CloseDesktop. Pid reuse cannot hit
# another process: we hold the child's handle from the start. Same user only (we create everything).
#
# Refuses: relative or missing -Exe paths, bad desktop/event names, -TimeoutSec outside 1..3600,
# -Desktop not recorded by a live supervisor of this user. Log: %LOCALAPPDATA%\Foxglove\lab\quiet-launch.log.
# State files (one per live launch): %LOCALAPPDATA%\Foxglove\lab\quiet-launch-<name>.json.
#
# Exit codes: 0 ok (or the last -Run's exit code when given), 3 refused, 4 timed out (hard limit hit),
# 5 the launch failed. Never shows a console: start this script itself hidden (docs/lab-quiet-launch.md).
param(
    [string]$Exe = '',
    [Alias('Args')][string[]]$ArgList = @(),
    [string]$WorkDir = '',
    [hashtable]$Env = @{},
    [int]$TimeoutSec = 120,
    [string[]]$Run = @(),
    [int]$RunTimeoutSec = 60,
    [switch]$Keep,
    [string]$DesktopName = '',
    [string]$Desktop = '',
    [string]$Stop = '',
    [string]$ReadyEvent = '',
    [string]$OutFile = '',
    [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }

# ---- pure parts (tested by -SelfTest) --------------------------------------------------------

$LabDir = Join-Path $(if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { [IO.Path]::GetTempPath() }) 'Foxglove\lab'

function Test-DesktopName([string]$n) { $n -cmatch '^ql-[a-z0-9]{4,24}$' }
function New-DesktopName { 'ql-' + ([guid]::NewGuid().ToString('N').Substring(0, 8)) }
function Test-EventName([string]$n) { $n -cmatch '^Local\\quiet-launch-[A-Za-z0-9-]{1,40}$' }

# Why this launch is refused, or $null when fine.
function Get-ExeProblem([string]$exe) {
    if (-not $exe) { return 'give -Exe (an absolute path)' }
    if ($exe -notmatch '^[A-Za-z]:\\') { return "-Exe must be an absolute drive path, got '$exe'" }
    if ($exe -match '[*?<>|"]') { return "-Exe has wildcard or invalid characters: '$exe'" }
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { return "-Exe not found: '$exe'" }
    return $null
}
function Get-TimeoutProblem([int]$t, [string]$what) {
    if ($t -lt 1 -or $t -gt 3600) { return "$what must be 1..3600 seconds, got $t" }
    return $null
}

# One argument quoted the way CommandLineToArgvW reads it back.
function ConvertTo-QuotedArg([string]$a) {
    if ($a -ne '' -and $a -notmatch '[\s"]') { return $a }
    $sb = New-Object Text.StringBuilder
    [void]$sb.Append('"')
    $bs = 0
    foreach ($ch in $a.ToCharArray()) {
        if ($ch -eq '\') { $bs++; continue }
        if ($ch -eq '"') { [void]$sb.Append('\', $bs * 2 + 1); [void]$sb.Append('"'); $bs = 0; continue }
        if ($bs) { [void]$sb.Append('\', $bs); $bs = 0 }
        [void]$sb.Append($ch)
    }
    if ($bs) { [void]$sb.Append('\', $bs * 2) }
    [void]$sb.Append('"')
    $sb.ToString()
}
function ConvertTo-CommandLine([string]$exe, [string[]]$a) {
    (@($exe) + @($a | ForEach-Object { $_ }) | ForEach-Object { ConvertTo-QuotedArg $_ }) -join ' '
}

# The environment block (KEY=VALUE NUL ... NUL NUL): the current environment with -Env on top.
function New-EnvBlock([hashtable]$over, $base) {
    $m = New-Object 'System.Collections.Generic.SortedDictionary[string,string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($k in $base.Keys) { $m[[string]$k] = [string]$base[$k] }
    foreach ($k in $over.Keys) {
        if (-not $k -or ([string]$k).Contains('=') -or ([string]$k).Contains("`0")) { throw "bad -Env name '$k'" }
        $m[[string]$k] = [string]$over[$k]
    }
    $sb = New-Object Text.StringBuilder
    foreach ($kv in $m.GetEnumerator()) { [void]$sb.Append($kv.Key).Append('=').Append($kv.Value).Append([char]0) }
    [void]$sb.Append([char]0)
    $sb.ToString()
}

function Expand-RunText([string]$text, [int]$pid_, [string]$desktop) {
    $text.Replace('{pid}', [string]$pid_).Replace('{desktop}', $desktop)
}

# ---- Win32 -----------------------------------------------------------------------------------

$nativeSource = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
namespace QuietLaunch {
public class Spawned { public IntPtr Process; public int Pid; }
public static class Native {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO {
        public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
    [StructLayout(LayoutKind.Sequential)]
    struct BASIC_LIMIT {
        public long PerProcessUserTimeLimit, PerJobUserTimeLimit; public uint LimitFlags;
        public UIntPtr MinWs, MaxWs; public uint ActiveProcessLimit; public UIntPtr Affinity;
        public uint PriorityClass, SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct IO_COUNTERS { public ulong a, b, c, d, e, f; }
    [StructLayout(LayoutKind.Sequential)]
    struct EXT_LIMIT {
        public BASIC_LIMIT Basic; public IO_COUNTERS Io;
        public UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct BASIC_ACCOUNTING {
        public long TotalUser, TotalKernel, PeriodUser, PeriodKernel;
        public uint PageFaults, TotalProcesses, ActiveProcesses, TotalTerminated;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateDesktopW(string name, IntPtr dev, IntPtr dm, uint flags, uint access, IntPtr sa);
    [DllImport("user32.dll", SetLastError = true)] static extern bool CloseDesktop(IntPtr d);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags,
        IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateJobObjectW(IntPtr sa, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr job, int cls, IntPtr info, int len);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool QueryInformationJobObject(IntPtr job, int cls, IntPtr info, int len, IntPtr ret);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr p);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateJobObject(IntPtr job, uint code);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateProcess(IntPtr p, uint code);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint ResumeThread(IntPtr t);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr p, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForMultipleObjects(uint n, IntPtr[] h, bool all, uint ms);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateEventW(IntPtr sa, bool manual, bool initial, string name);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr OpenEventW(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetEvent(IntPtr e);

    public static IntPtr NewDesktop(string name) {
        IntPtr d = CreateDesktopW(name, IntPtr.Zero, IntPtr.Zero, 0, 0x10000000, IntPtr.Zero);
        if (d == IntPtr.Zero) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "CreateDesktopW");
        return d;
    }
    public static void DropDesktop(IntPtr d) { if (d != IntPtr.Zero) CloseDesktop(d); }
    public static IntPtr NewJob() {
        IntPtr j = CreateJobObjectW(IntPtr.Zero, null);
        if (j == IntPtr.Zero) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "CreateJobObjectW");
        EXT_LIMIT li = new EXT_LIMIT();
        li.Basic.LimitFlags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        int n = Marshal.SizeOf(typeof(EXT_LIMIT));
        IntPtr p = Marshal.AllocHGlobal(n);
        try {
            Marshal.StructureToPtr(li, p, false);
            if (!SetInformationJobObject(j, 9, p, n))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "SetInformationJobObject");
        } finally { Marshal.FreeHGlobal(p); }
        return j;
    }
    // job may be Zero (then not suspended, not in a job). The child is created suspended, put in the
    // job, then resumed, so nothing it starts can escape the job.
    public static Spawned Spawn(string app, string cmdline, string cwd, string env, string desktop, IntPtr job) {
        STARTUPINFO si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(typeof(STARTUPINFO));
        si.lpDesktop = "winsta0\\" + desktop;
        si.dwFlags = 0x1; // STARTF_USESHOWWINDOW
        si.wShowWindow = 0; // SW_HIDE
        uint flags = 0x400 | 0x08000000 | 0x4; // UNICODE_ENVIRONMENT | NO_WINDOW | SUSPENDED
        IntPtr envp = IntPtr.Zero;
        if (env != null) envp = Marshal.StringToHGlobalUni(env);
        PROCESS_INFORMATION pi;
        try {
            StringBuilder cmd = new StringBuilder(cmdline);
            if (!CreateProcessW(app, cmd, IntPtr.Zero, IntPtr.Zero, false, flags, envp, cwd, ref si, out pi))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "CreateProcessW");
        } finally { if (envp != IntPtr.Zero) Marshal.FreeHGlobal(envp); }
        if (job != IntPtr.Zero && !AssignProcessToJobObject(job, pi.hProcess)) {
            int err = Marshal.GetLastWin32Error();
            TerminateProcess(pi.hProcess, 1); CloseHandle(pi.hThread); CloseHandle(pi.hProcess);
            throw new System.ComponentModel.Win32Exception(err, "AssignProcessToJobObject");
        }
        ResumeThread(pi.hThread);
        CloseHandle(pi.hThread);
        Spawned s = new Spawned(); s.Process = pi.hProcess; s.Pid = pi.dwProcessId;
        return s;
    }
    // 0 = the process ended, 1 = the event was set, 258 = timed out.
    public static int WaitFor(IntPtr proc, IntPtr ev, uint ms) {
        IntPtr[] hs = ev == IntPtr.Zero ? new IntPtr[] { proc } : new IntPtr[] { proc, ev };
        uint r = WaitForMultipleObjects((uint)hs.Length, hs, false, ms);
        return (int)r;
    }
    public static uint ExitCode(IntPtr p) { uint c; GetExitCodeProcess(p, out c); return c; }
    public static void KillProcess(IntPtr p) { TerminateProcess(p, 1); }
    public static void KillJob(IntPtr j) { TerminateJobObject(j, 1); }
    public static uint ActiveInJob(IntPtr j) {
        int n = Marshal.SizeOf(typeof(BASIC_ACCOUNTING));
        IntPtr p = Marshal.AllocHGlobal(n);
        try {
            if (!QueryInformationJobObject(j, 1, p, n, IntPtr.Zero)) return 0;
            return ((BASIC_ACCOUNTING)Marshal.PtrToStructure(p, typeof(BASIC_ACCOUNTING))).ActiveProcesses;
        } finally { Marshal.FreeHGlobal(p); }
    }
    public static void Close(IntPtr h) { if (h != IntPtr.Zero) CloseHandle(h); }
    public static IntPtr NewEvent(string name) { return CreateEventW(IntPtr.Zero, true, false, name); }
    public static bool SignalNamedEvent(string name) {
        IntPtr e = OpenEventW(0x2, false, name); // EVENT_MODIFY_STATE
        if (e == IntPtr.Zero) return false;
        bool ok = SetEvent(e); CloseHandle(e); return ok;
    }
    public static bool SetEv(IntPtr e) { return SetEvent(e); }
}
}
'@

function Initialize-Native {
    if (-not ('QuietLaunch.Native' -as [type])) { Add-Type -TypeDefinition $nativeSource -Language CSharp }
}

# ---- self test -------------------------------------------------------------------------------

if ($SelfTest) {
    $fail = @()
    function Check($ok, $what) { if (-not $ok) { $script:fail += $what } }
    Check ((Test-DesktopName 'ql-abc12345') -and (Test-DesktopName (New-DesktopName))) 'generated and sample desktop names pass'
    foreach ($bad in '', 'Default', 'ql-', 'ql-AB', 'ql-ab', 'ql-abc\x', 'winsta0\ql-abc123', 'ql-abc*', 'ql-abc 123', 'ql-abcdefghijklmnopqrstuvwxyz0') {
        Check (-not (Test-DesktopName $bad)) "desktop name '$bad' is refused"
    }
    Check ((Test-EventName 'Local\quiet-launch-proof-1') -and -not (Test-EventName 'Global\quiet-launch-x') -and -not (Test-EventName 'Local\other') -and -not (Test-EventName 'Local\quiet-launch-')) 'event names are Local\quiet-launch-*'
    Check ($null -ne (Get-ExeProblem '')) 'empty -Exe refused'
    foreach ($rel in 'notepad.exe', '.\app.exe', '..\app.exe', 'bin\app.exe', '\\server\share\app.exe', '\app.exe') {
        Check ($null -ne (Get-ExeProblem $rel)) "non-absolute -Exe '$rel' refused"
    }
    Check ($null -ne (Get-ExeProblem 'C:\*\app.exe')) 'wildcard -Exe refused'
    Check ($null -ne (Get-ExeProblem 'C:\definitely\not\here\app.exe')) 'missing -Exe refused'
    Check ($null -eq (Get-ExeProblem $PSCommandPath.Replace('.ps1', '.ps1'))) 'an existing absolute file passes'
    Check (($null -ne (Get-TimeoutProblem 0 't')) -and ($null -ne (Get-TimeoutProblem 3601 't')) -and ($null -eq (Get-TimeoutProblem 120 't')) -and ($null -eq (Get-TimeoutProblem 1 't'))) 'timeout bounds are 1..3600'
    Check ((ConvertTo-QuotedArg 'abc') -eq 'abc') 'plain arg is not quoted'
    Check ((ConvertTo-QuotedArg 'a b') -eq '"a b"') 'space arg is quoted'
    Check ((ConvertTo-QuotedArg '') -eq '""') 'empty arg is ""'
    Check ((ConvertTo-QuotedArg 'say "hi"') -eq '"say \"hi\""') 'embedded quotes are escaped'
    Check ((ConvertTo-QuotedArg 'C:\dir with space\') -eq '"C:\dir with space\\"') 'trailing backslash is doubled inside quotes'
    Check ((ConvertTo-CommandLine 'C:\a b\x.exe' @('-p', 'x y')) -eq '"C:\a b\x.exe" -p "x y"') 'command line joins quoted parts'
    $blk = New-EnvBlock @{ FOXGLOVE_CONFIG_DIR = 'C:\lab\cfg'; Path = 'X' } @{ Path = 'Y'; Other = '1' }
    Check ($blk -ceq "FOXGLOVE_CONFIG_DIR=C:\lab\cfg$([char]0)Other=1$([char]0)Path=X$([char]0)$([char]0)") 'env block: override wins, sorted, double NUL end'
    $threw = $false
    try { New-EnvBlock @{ 'A=B' = 'x' } @{} | Out-Null } catch { $threw = $true }
    Check $threw 'env name with = is refused'
    Check ((Expand-RunText 'uia -ProcessId {pid} on {desktop}' 42 'ql-x1') -eq 'uia -ProcessId 42 on ql-x1') 'placeholders expand'
    # The C# compiles (this only compiles; nothing is created or started).
    $compiled = $true
    try { Initialize-Native } catch { $compiled = $false; $fail += "native source does not compile: $_" }
    Check $compiled 'P/Invoke source compiles'
    # Teardown rules in the text: job + handles only, never names or wildcards.
    $self = Get-Content -LiteralPath $PSCommandPath -Raw
    foreach ($w in ('Stop' + '-Process'), ('task' + 'kill'), ('Get-Process -' + 'Name'), ('Get-Process ' + '*'), ('pk' + 'ill')) {
        Check (-not $self.Contains($w)) "the script never uses $w"
    }
    $ns = '[QuietLaunch.' + 'Native]::'
    Check ($self.IndexOf($ns + 'KillJob($hJob)') -gt 0 -and $self.IndexOf($ns + 'KillJob($hJob)') -lt $self.IndexOf($ns + 'DropDesktop($hDesk)')) 'the job is killed before the desktop is closed'
    if ($fail.Count) { $fail | ForEach-Object { Write-Host "FAIL: $_" }; exit 1 }
    Write-Host 'quiet-launch self-test: ok'
    exit 0
}

# ---- logging and state -----------------------------------------------------------------------

New-Item -ItemType Directory -Force -Path $LabDir | Out-Null
$LogFile = Join-Path $LabDir 'quiet-launch.log'
function Write-Log([string]$msg) {
    $line = '{0:yyyy-MM-dd HH:mm:ss.fff} [{1}] {2}' -f (Get-Date), $PID, $msg
    try { [IO.File]::AppendAllText($LogFile, $line + "`r`n", (New-Object Text.UTF8Encoding($false))) } catch { }
}
function Say([string]$msg) {
    Write-Host $msg
    if ($OutFile) { try { [IO.File]::AppendAllText($OutFile, $msg + "`r`n", (New-Object Text.UTF8Encoding($false))) } catch { } }
}
function Refuse([string]$why) { Write-Log "refused: $why"; Say "refused: $why"; exit 3 }
function StatePath([string]$name) { Join-Path $LabDir "quiet-launch-$name.json" }

# A live supervisor of this user recorded for that desktop name, or $null.
function Get-LiveState([string]$name) {
    $f = StatePath $name
    if (-not (Test-Path -LiteralPath $f)) { return $null }
    try { $st = Get-Content -LiteralPath $f -Raw | ConvertFrom-Json } catch { return $null }
    $p = Get-Process -Id ([int]$st.supervisorPid) -ErrorAction SilentlyContinue
    if (-not $p -or $p.StartTime.ToUniversalTime().ToString('o') -ne $st.supervisorStart) { return $null }
    if ($st.user -cne "$env:USERDOMAIN\$env:USERNAME") { return $null }
    $st
}

# Runs one command text in a hidden powershell on `desktopName`; prints its output, returns its exit code.
function Invoke-OnDesktop([string]$desktopName, [string]$text, [int]$limitSec, [IntPtr]$job, [IntPtr]$stopEv) {
    $tmp = Join-Path $LabDir ("quiet-run-{0}.txt" -f [guid]::NewGuid().ToString('N').Substring(0, 8))
    # The text goes in a temp .ps1 so an `exit` in it ends only that script (output is still flushed).
    $body = Join-Path $LabDir ("quiet-run-{0}.ps1" -f [IO.Path]::GetFileNameWithoutExtension($tmp).Substring(10))
    [IO.File]::WriteAllText($body, $text, (New-Object Text.UTF8Encoding($true)))
    $script = "`$ErrorActionPreference='Continue'; `$global:LASTEXITCODE=0; & '$body' *>&1 | Out-File -LiteralPath '$tmp' -Encoding utf8; exit `$LASTEXITCODE"
    $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
    $psExe = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $cl = ConvertTo-CommandLine $psExe @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $enc)
    Write-Log "run on ${desktopName}: $text"
    $s = [QuietLaunch.Native]::Spawn($psExe, $cl, $PSScriptRoot, $null, $desktopName, $job)
    $code = 5
    try {
        $r = [QuietLaunch.Native]::WaitFor($s.Process, $stopEv, [uint32]($limitSec * 1000))
        if ($r -eq 0) { $code = [int][QuietLaunch.Native]::ExitCode($s.Process) }
        else {
            [QuietLaunch.Native]::KillProcess($s.Process)   # this exact handle only
            $code = if ($r -eq 258) { 4 } else { 5 }
            Say "run stopped ($(if ($r -eq 258) { 'timed out after ' + $limitSec + ' s' } else { 'stop asked' })): $text"
        }
    } finally { [QuietLaunch.Native]::Close($s.Process); Remove-Item -LiteralPath $body -Force -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath $tmp) {
        Get-Content -LiteralPath $tmp -Encoding UTF8 | ForEach-Object { Say $_ }
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
    Write-Log "run exit $code"
    $code
}

# ---- modes -----------------------------------------------------------------------------------

if ($Stop) {
    if (-not (Test-DesktopName $Stop)) { Refuse "bad -Stop name '$Stop'" }
    Initialize-Native
    if (-not (Get-LiveState $Stop)) { Say "no live launch named $Stop (nothing to stop)"; exit 0 }
    $ok = [QuietLaunch.Native]::SignalNamedEvent("Local\quiet-launch-stop-$Stop")
    Write-Log "stop asked for $Stop ($ok)"
    Say "stop asked for $Stop"
    exit 0
}

if ($Desktop) {
    if (-not (Test-DesktopName $Desktop)) { Refuse "bad -Desktop name '$Desktop'" }
    if (-not $Run.Count) { Refuse '-Desktop needs -Run' }
    if ($Exe) { Refuse '-Desktop attaches to a live launch; do not give -Exe' }
    $problem = Get-TimeoutProblem $RunTimeoutSec '-RunTimeoutSec'
    if ($problem) { Refuse $problem }
    $st = Get-LiveState $Desktop
    if (-not $st) { Refuse "no live quiet-launch of yours owns desktop $Desktop" }
    Initialize-Native
    $last = 0
    foreach ($r in $Run) { $last = Invoke-OnDesktop $Desktop (Expand-RunText $r ([int]$st.pid) $Desktop) $RunTimeoutSec ([IntPtr]::Zero) ([IntPtr]::Zero) }
    exit $last
}

# Launch + supervise.
$problem = Get-ExeProblem $Exe
if (-not $problem) { $problem = Get-TimeoutProblem $TimeoutSec '-TimeoutSec' }
if (-not $problem) { $problem = Get-TimeoutProblem $RunTimeoutSec '-RunTimeoutSec' }
if (-not $problem -and $WorkDir -and ($WorkDir -notmatch '^[A-Za-z]:\\' -or -not (Test-Path -LiteralPath $WorkDir -PathType Container))) { $problem = "-WorkDir must be an existing absolute directory, got '$WorkDir'" }
if (-not $problem -and $DesktopName -and -not (Test-DesktopName $DesktopName)) { $problem = "bad -DesktopName '$DesktopName' (ql-<4..24 lowercase letters or digits>)" }
if (-not $problem -and $ReadyEvent -and -not (Test-EventName $ReadyEvent)) { $problem = "bad -ReadyEvent '$ReadyEvent' (Local\quiet-launch-<name>)" }
if ($problem) { Refuse $problem }
$name = if ($DesktopName) { $DesktopName } else { New-DesktopName }
if (Get-LiveState $name) { Refuse "desktop $name is already in use by a live launch" }
$exeFull = [IO.Path]::GetFullPath($Exe)
$image = [IO.Path]::GetFileName($exeFull)
$cwd = if ($WorkDir) { $WorkDir } else { [IO.Path]::GetDirectoryName($exeFull) }
$envBlock = New-EnvBlock $Env ([Environment]::GetEnvironmentVariables())
$cmdline = ConvertTo-CommandLine $exeFull $ArgList

Initialize-Native
$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSec)
$hDesk = [IntPtr]::Zero; $hJob = [IntPtr]::Zero; $hStop = [IntPtr]::Zero; $hReady = [IntPtr]::Zero
$child = $null; $exitCode = 0; $stateFile = StatePath $name
try {
    $hDesk = [QuietLaunch.Native]::NewDesktop($name)
    $hJob = [QuietLaunch.Native]::NewJob()
    $hStop = [QuietLaunch.Native]::NewEvent("Local\quiet-launch-stop-$name")
    $child = [QuietLaunch.Native]::Spawn($exeFull, $cmdline, $cwd, $envBlock, $name, $hJob)
    $me = Get-Process -Id $PID
    @{
        desktop = $name; pid = $child.Pid; image = $image; supervisorPid = $PID
        supervisorStart = $me.StartTime.ToUniversalTime().ToString('o'); user = "$env:USERDOMAIN\$env:USERNAME"
        deadlineUtc = $deadline.ToString('o')
    } | ConvertTo-Json | Set-Content -LiteralPath $stateFile -Encoding UTF8
    Write-Log "started $image pid $($child.Pid) on desktop $name (timeout $TimeoutSec s)"
    Say "QUIET-LAUNCH desktop=$name pid=$($child.Pid) image=$image timeout=$TimeoutSec"
    if ($ReadyEvent) {
        $hReady = [QuietLaunch.Native]::NewEvent($ReadyEvent)
        [void][QuietLaunch.Native]::SetEv($hReady)
    }

    $timedOut = $false
    foreach ($r in $Run) {
        $left = [int][Math]::Floor(($deadline - [DateTime]::UtcNow).TotalSeconds)
        if ($left -lt 1) { $timedOut = $true; break }
        $exitCode = Invoke-OnDesktop $name (Expand-RunText $r $child.Pid $name) ([Math]::Min($RunTimeoutSec, $left)) $hJob $hStop
    }
    if (-not $timedOut -and ($Keep -or -not $Run.Count)) {
        $left = [int][Math]::Floor(($deadline - [DateTime]::UtcNow).TotalSeconds)
        $r = [QuietLaunch.Native]::WaitFor($child.Process, $hStop, [uint32][Math]::Max(0, $left * 1000))
        if ($r -eq 258) { $timedOut = $true }
        Write-Log "wait ended: $(@{ 0 = 'child exited'; 1 = 'stop asked'; 258 = 'timeout' }[$r])"
    }
    if ($timedOut) { Say "hard limit of $TimeoutSec s reached: tearing down"; $exitCode = 4 }
}
catch {
    Write-Log "failed: $_"; Say "launch failed: $_"; $exitCode = 5
}
finally {
    # Teardown order: the whole job (child's tree), wait for the child, then the desktop.
    if ($hJob -ne [IntPtr]::Zero) { [QuietLaunch.Native]::KillJob($hJob) }
    if ($child) {
        [void][QuietLaunch.Native]::WaitFor($child.Process, [IntPtr]::Zero, 10000)
        [QuietLaunch.Native]::Close($child.Process)
    }
    $left = if ($hJob -ne [IntPtr]::Zero) { [QuietLaunch.Native]::ActiveInJob($hJob) } else { 0 }
    if ($left) { Write-Log "warning: $left process(es) still in the job at teardown" }
    [QuietLaunch.Native]::Close($hJob)
    [QuietLaunch.Native]::DropDesktop($hDesk)
    [QuietLaunch.Native]::Close($hStop)
    [QuietLaunch.Native]::Close($hReady)
    Remove-Item -LiteralPath $stateFile -Force -ErrorAction SilentlyContinue
    Write-Log "torn down desktop $name (exit $exitCode)"
}
exit $exitCode
