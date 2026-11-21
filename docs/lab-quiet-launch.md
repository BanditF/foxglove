# Quiet launcher (lab)

`scripts/lab/quiet-launch.ps1` starts a program on a **private hidden desktop** (CreateDesktopW,
`STARTUPINFO.lpDesktop = winsta0\<name>`), so it can never take focus or show a window on the user's
default desktop (for example while they game). Windows PowerShell 5.1, P/Invoke only, same user only.

## Usage

Arrays and hashtables do **not** bind through `powershell -File` (`-ArgList a,b` arrives as one
string, `-Env @{}` fails). From a script, call it with `-Command`/`-EncodedCommand` (real arrays), or
use the JSON forms `-ArgJson '["a","b"]'` and `-EnvJson '{"FOXGLOVE_CONFIG_DIR":"C:\\lab\\cfg"}'`.
Through `-File`, `-Run` is one string.

```powershell
# start, hold until the child exits / -Stop / the hard limit (run THIS script hidden, see below)
quiet-launch.ps1 -Exe C:\path\foxglove.exe -Env @{FOXGLOVE_CONFIG_DIR='C:\lab\cfg'} -TimeoutSec 120
#   -> QUIET-LAUNCH desktop=ql-1a2b3c4d pid=1234 image=foxglove.exe timeout=120

# start, run a UIA client on that desktop, tear down
quiet-launch.ps1 -Exe C:\path\foxglove.exe -Env @{FOXGLOVE_CONFIG_DIR='C:\lab\cfg'} `
    -Run 'C:\...\scripts\lab\uia_tree.ps1 -ProcessId {pid} -Expect Button:Start'

# attach to a live launch / end it
quiet-launch.ps1 -Desktop ql-1a2b3c4d -Run '<command with {pid}>'
quiet-launch.ps1 -Stop ql-1a2b3c4d
quiet-launch.ps1 -SelfTest        # pure parts, starts nothing
```

Parameters: `-Exe` (absolute drive path, required), `-ArgList` (alias `-Args`) / `-ArgJson`, `-WorkDir`
(default: the exe's folder), `-Env` / `-EnvJson` (laid over the current environment, so
`FOXGLOVE_CONFIG_DIR` isolation works; NUL in a value is refused), `-TimeoutSec` (hard limit, 1..3600,
default 120), `-Run` (repeatable from `-Command`), `-RunTimeoutSec` (per run, default 60), `-Keep`
(stay up after the `-Run`s until exit/stop/limit), `-AllowRunning` (see below), `-DesktopName`
(`ql-` plus 4..24 lowercase letters or digits; default random), `-ReadyEvent`
(`Local\quiet-launch-<name>`, set once the child runs), `-OutFile` (also append the printed lines).

Exit codes: 0 ok (or the last `-Run`'s exit code), 3 refused, 4 hard limit hit, 5 launch failed.

## UIA on the hidden desktop

A UI Automation client only sees the desktop of the thread it runs on. `-Run` therefore starts a
hidden `powershell.exe` **on that desktop** (the whole process, not a thread) and runs your text
there as a script, with `{pid}` and `{desktop}` replaced. `uia_tree.ps1 -ProcessId {pid} ...` works
unchanged (it also needs `FOXGLOVE_APP_OK=1` in the environment of the caller, which `-Run` inherits).
The main child is started without SW_HIDE (the private desktop already hides it), so its windows stay
visible to UIA; only the `-Run` powershell helper gets SW_HIDE.

## Teardown and safety

- The child is created suspended, put in a kill-on-close **job object**, then resumed, so its whole
  tree is in the job. Teardown is TerminateJobObject, wait for the child's handle, then CloseDesktop
  (never before the processes are gone). We hold the child's handle from the start, so pid reuse
  cannot hit another process. No name matching, no wildcards, no `taskkill`.
- Every `-Run` helper (in the launching call and in attach mode) gets its own kill-on-close job: an
  overrun, a `-Stop` or the death of the launcher kills the helper's whole tree. A `-Run` loop stops at
  `-Stop`. Attach runs are capped at the launch's recorded hard limit (`deadlineUtc`).
- The hard limit is mandatory (default 120 s): the supervisor wakes on child exit, `-Stop` (a named
  event) or the deadline; there is no polling.
- Refuses relative, missing or wildcard exe paths, bad names, a window station other than `WinSta0`,
  an image that is already running (below), and `-Desktop` names not owned by a live supervisor
  recorded in this user's profile (`quiet-launch-<name>.json`, with the supervisor's pid and start
  time; the owner is matched by user name from the environment, so this is a guard against mistakes,
  not a security boundary between users).
- Log: `%LOCALAPPDATA%\Foxglove\lab\quiet-launch.log`; state files live next to it and are removed at teardown.
- Never a visible console: start the launcher itself with CreateProcess and no window, not
  `Start-Process -WindowStyle Hidden` (that goes through ShellExecute and, with Windows Terminal as the
  default terminal, can still show a window). From PowerShell:
  `$psi = New-Object Diagnostics.ProcessStartInfo; $psi.FileName = <powershell.exe>; $psi.Arguments = ...;
  $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true; [Diagnostics.Process]::Start($psi)`
  (the proof script does exactly this). The child gets CREATE_NO_WINDOW as well.

## Paths that still reach the default desktop

The private desktop only isolates windows the child creates itself. These go around it:

- **A copy already running.** Single-instance apps (Foxglove: `Local\Foxglove.Panel`) hand off to the
  running instance, which shows itself or takes the foreground on the user's desktop. The launcher
  therefore **refuses when a process with the same image name already runs** (exact name, no
  wildcard; `-AllowRunning` overrides for programs that are not single-instance). It cannot see a
  copy that starts later, or one under another image name.
- **URLs and files opened with ShellExecute** hand off to a running browser or Explorer on the default
  desktop. Lab programs must not open links or documents.
- **Toast notifications and DCOM out-of-process servers** are shown by the user's shell/session.
- **Elevation prompts** (`runas`) switch the screen to the secure desktop mid-game. Lab programs
  must not elevate.
- **Windows Terminal as the default terminal:** a console grandchild may be handed to a terminal window
  on the default desktop. The proof runs a `cmd /c ping` grandchild to catch this on the machine.

## Limits

- GPU/DirectX apps may not render on a non-default desktop; UIA and the app's logic still work.
  wgpu/Direct3D apps (Foxglove) may start but draw nothing. GPU and CPU load are still shared with
  the user's game.
- A separate window station is not used (it would also need its own clipboard and shell, and the
  default desktop must stay untouched). The hidden desktop has its own focus and input queue: nothing
  on it receives the user's keys or mouse.
- Shared per **window station** (so shared with the user's session): the clipboard and the global
  atom table. Per **desktop** (not shared): windows, window hooks (SetWindowsHookEx global and
  low-level hooks only see the desktop of the caller), and the foreground. The tray: Shell_TrayWnd
  lives on the default desktop, so tray icons usually do not appear for a program on the hidden desktop.
  RegisterHotKey collisions with the user's session are unverified.
- The default-desktop proof is `scripts/lab/quiet-launch-proof.ps1` (opens a small window on the hidden
  desktop; On-call runs it from the user's interactive session; PASS/FAIL/INFO lines). It first
  asserts it runs on the default desktop, records every foreground change and shown top-level window
  there with WinEvent hooks during the whole run, and fails only on events from the launch tree
  (unrelated windows or alt-tabs are INFO). Window-free tests: `python -m unittest test_quiet_launch`
  in `scripts/lab`.

## Troubleshooting: `CreateProcessW failed, GetLastError=87` (fixed 11/20)

Every `-Run` helper failed to start, so the proof's "UIA on the hidden desktop sees the proof window"
check failed (12/13). Cause: the helper is spawned with no environment of its own, and Windows
PowerShell turns `$null` into an **empty string** for a string parameter. An empty string is not a valid
environment block (a single NUL, no terminator), so `CreateProcessW` answered `ERROR_INVALID_PARAMETER`.
`Spawn` now treats null or empty as "inherit" (NULL block). Not the cause, each checked: the
`lpDesktop` string (`winsta0\<name>`), handle inheritance, the token or the Claude-package shell
(the main child, with a real env block, always started). Errors now print the API name, the numeric
`GetLastError` and the system text; `-SelfTest` covers both (no process started). Proof is 13/13.
