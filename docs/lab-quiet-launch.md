# Quiet launcher (lab)

`scripts/lab/quiet-launch.ps1` starts a program on a **private hidden desktop** (CreateDesktopW,
`STARTUPINFO.lpDesktop = winsta0\<name>`), so it can never take focus or show a window on the user's
default desktop (for example while they game). Windows PowerShell 5.1, P/Invoke only, same user only.

## Usage

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

Parameters: `-Exe` (absolute drive path, required), `-ArgList` (alias `-Args`), `-WorkDir` (default: the
exe's folder), `-Env` (hashtable laid over the current environment, so `FOXGLOVE_CONFIG_DIR`
isolation works), `-TimeoutSec` (hard limit, 1..3600, default 120), `-Run` (repeatable),
`-RunTimeoutSec` (per run, default 60), `-Keep` (stay up after the `-Run`s until exit/stop/limit),
`-DesktopName` (`ql-` plus 4..24 lowercase letters or digits; default random), `-ReadyEvent`
(`Local\quiet-launch-<name>`, set once the child runs), `-OutFile` (also append the printed lines).

Exit codes: 0 ok (or the last `-Run`'s exit code), 3 refused, 4 hard limit hit, 5 launch failed.

## UIA on the hidden desktop

A UI Automation client only sees the desktop of the thread it runs on. `-Run` therefore starts a
hidden `powershell.exe` **on that desktop** (the whole process, not a thread) and runs your text
there as a script, with `{pid}` and `{desktop}` replaced. `uia_tree.ps1 -ProcessId {pid} ...` works
unchanged (it also needs `FOXGLOVE_APP_OK=1` in the environment of the caller, which `-Run` inherits).

## Teardown and safety

- The child is created suspended, put in a kill-on-close **job object**, then resumed, so its whole
  tree is in the job. Teardown is TerminateJobObject, wait for the child's handle, then CloseDesktop
  (never before the processes are gone). We hold the child's handle from the start, so pid reuse
  cannot hit another process. No name matching, no wildcards, no `taskkill`.
- A `-Run` helper that overruns is killed by its own handle only.
- The hard limit is mandatory (default 120 s): the supervisor wakes on child exit, `-Stop` (a named
  event) or the deadline; there is no polling.
- Refuses relative, missing or wildcard exe paths, bad names, and `-Desktop` names not owned by a live
  supervisor of this user (state file `quiet-launch-<name>.json` with the supervisor's pid and start time).
- Log: `%LOCALAPPDATA%\Foxglove\lab\quiet-launch.log`; state files live next to it and are removed at teardown.
- Never a visible console: start the launcher itself hidden, e.g.
  `Start-Process powershell -WindowStyle Hidden -ArgumentList ...` or from a hidden session. The child
  is created with CREATE_NO_WINDOW and SW_HIDE on top of living on the hidden desktop.

## Limits

- GPU/DirectX apps may not render on a non-default desktop (no swap chain to the display); UIA and the
  app's logic still work. wgpu/Direct3D apps (Foxglove) may start but draw nothing.
- Own window station is not used (not needed: a desktop in `winsta0` has its own focus and input
  queue). The desktop has no input: nothing on it receives the user's keys or mouse.
- Clipboard, tray and global hooks belong to the window station, so they are shared with the user's
  session: a lab program that writes the clipboard or registers a global hotkey still affects it.
- The default-desktop proof is `scripts/lab/quiet-launch-proof.ps1` (opens a small window on the hidden
  desktop; On-call runs it, PASS/FAIL lines). Window-free tests: `python -m unittest test_quiet_launch`
  in `scripts/lab`.
