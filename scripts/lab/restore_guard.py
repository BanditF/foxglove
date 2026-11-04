"""Brings the user's Foxglove back when a lab run dies without doing it (labkit.App).

App stops the user's Foxglove for a run and starts it again on the way out. A run that is killed
(a timeout, a closed terminal, an abort mid-case) never gets there: the d7b5f4f IA abort left the
user without Foxglove for almost five minutes. App starts this guard, detached, right after it
has stopped the user's app and started the test app:

    python -I restore_guard.py <lab pid> <test app pid> <path of the user's foxglove.exe>...

It waits for the lab's process to end (at most GUARD_S, then it gives up and does nothing), then:
the test app still running is stopped by its own PID (never by name); with no process named
exactly foxglove running, the user's paths are started again. After a clean run App has already
done both, so the guard finds Foxglove running and ends without touching anything.
"""
import ctypes
import subprocess
import sys
import time

GUARD_S = 3 * 3600  # longer than any lab run; then the guard ends on its own
SYNCHRONIZE, PROCESS_TERMINATE, WAIT_TIMEOUT = 0x00100000, 0x0001, 0x102


def foxglove_running():
    out = subprocess.run(['powershell', '-NoProfile', '-Command',
                          '@(Get-Process -Name foxglove -ErrorAction SilentlyContinue).Count'],
                         capture_output=True, text=True).stdout.strip()
    return out not in ('', '0')


def main(lab_pid, app_pid, paths):
    k = ctypes.windll.kernel32
    k.OpenProcess.restype = ctypes.c_void_p
    k.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    k.TerminateProcess.argtypes = [ctypes.c_void_p, ctypes.c_uint]
    k.CloseHandle.argtypes = [ctypes.c_void_p]
    # Both handles first, while the PIDs are still those processes': a handle keeps naming its
    # process, so a PID Windows hands out again later is never stopped.
    lab = k.OpenProcess(SYNCHRONIZE, False, lab_pid)
    app = k.OpenProcess(SYNCHRONIZE | PROCESS_TERMINATE, False, app_pid)
    if lab:  # an event: the lab's process handle signals when it ends
        gave_up = k.WaitForSingleObject(lab, GUARD_S * 1000) == WAIT_TIMEOUT
        k.CloseHandle(lab)
        if gave_up:
            if app:
                k.CloseHandle(app)
            return
    if app:  # the test app the dead run left behind (a clean run stopped it already)
        if k.WaitForSingleObject(app, 0) == WAIT_TIMEOUT:
            k.TerminateProcess(app, 1)
            k.WaitForSingleObject(app, 10_000)
        k.CloseHandle(app)
        time.sleep(1)  # its HID handles go with it
    if paths and not foxglove_running():
        for p in paths:
            subprocess.Popen([p], creationflags=0x00000008 | 0x00000200)  # detached, own group


if __name__ == '__main__':
    main(int(sys.argv[1]), int(sys.argv[2]), sys.argv[3:])
