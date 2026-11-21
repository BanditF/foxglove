r"""A tiny fake "refusing copy" for check B3: serves a named pipe that accepts a client, reads
what it sends, and drops the connection without answering (a Foxglove-looking pipe that never
speaks the protocol). Windows only. Ends after --seconds (checked between clients) or
--connections accepted clients; the caller's process timeout covers a client that never comes.
It never touches the default pipe: the name must be the lab's suffixed form,
\\.\pipe\foxglove-<user>.<suffix>.

  python scripts/lab/fake_pipe_server.py \\.\pipe\foxglove-<user>.<hex> [--seconds 60] [--connections 3]
"""
import argparse
import ctypes
import re
import sys
import time
from ctypes import wintypes

NAME = re.compile(r'^\\\\\.\\pipe\\foxglove-[A-Za-z0-9_-]*\.[0-9A-Za-z]+$')
INVALID = wintypes.HANDLE(-1).value
ERROR_PIPE_CONNECTED = 535
PIPE_ACCESS_DUPLEX = 0x3
FIRST_PIPE_INSTANCE = 0x00080000
PIPE_REJECT_REMOTE_CLIENTS = 0x8


def _k32():
    k = ctypes.WinDLL('kernel32', use_last_error=True)
    k.CreateNamedPipeW.restype = wintypes.HANDLE
    k.CreateNamedPipeW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.DWORD,
                                   wintypes.DWORD, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
    k.ConnectNamedPipe.argtypes = [wintypes.HANDLE, ctypes.c_void_p]
    k.ReadFile.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p, ctypes.c_void_p]
    k.DisconnectNamedPipe.argtypes = [wintypes.HANDLE]
    k.CloseHandle.argtypes = [wintypes.HANDLE]
    return k


def serve(name, seconds, connections):
    if not NAME.fullmatch(name):
        raise SystemExit('refusing: the name must be the lab pipe foxglove-<user>.<suffix>')
    k = _k32()
    h = k.CreateNamedPipeW(name, PIPE_ACCESS_DUPLEX | FIRST_PIPE_INSTANCE, PIPE_REJECT_REMOTE_CLIENTS,
                           1, 4096, 4096, 0, None)
    if h == INVALID:
        raise SystemExit('cannot create %s (error %d)' % (name, ctypes.get_last_error()))
    deadline = time.monotonic() + seconds
    done = 0
    try:
        while done < connections and time.monotonic() < deadline:
            if not k.ConnectNamedPipe(h, None) and ctypes.get_last_error() != ERROR_PIPE_CONNECTED:
                break
            buf = ctypes.create_string_buffer(4096)
            got = wintypes.DWORD(0)
            k.ReadFile(h, buf, 4096, ctypes.byref(got), None)
            print('client sent %d bytes; dropping it' % got.value, flush=True)
            k.DisconnectNamedPipe(h)
            done += 1
    finally:
        k.CloseHandle(h)
    return 0


if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('name')
    ap.add_argument('--seconds', type=float, default=60)
    ap.add_argument('--connections', type=int, default=3)
    a = ap.parse_args()
    sys.exit(serve(a.name, a.seconds, a.connections))
