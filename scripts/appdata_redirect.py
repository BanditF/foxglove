"""Redirected AppData (#894), the Python twin of crates/redirect (stdlib only).

A script started from a packaged app's shell (a Claude Code session, say) runs inside that app's
file-system container: Windows puts its new files under %APPDATA% and %LOCALAPPDATA% into that
app's private copy (%LOCALAPPDATA%\\Packages\\<that app>\\LocalCache\\...), which nothing outside the
container reads. The flasher's flash-history.log and the lab's lab-injections.log landed there.

The signal: GetFinalPathNameByHandleW on an open handle names the file Windows really opened,
which then lies under another package's LocalCache. `check(path)` asks for an existing file;
`note()` is the line a tool prints when it wrote into the copy.
"""
import os
from pathlib import Path

OWN_PACKAGE = 'BanditF.Foxglove'  # Foxglove's own package is not another app's
_PACKAGES = '\\appdata\\local\\packages\\'


def _plain(p):
    p = str(p)
    if p.startswith('\\\\?\\UNC\\'):
        p = '\\\\' + p[len('\\\\?\\UNC\\'):]
    elif p.startswith('\\\\?\\'):
        p = p[len('\\\\?\\'):]
    return p.replace('/', '\\')


def redirected(requested, real):
    """(package, real path) when `real` (what Windows opened for `requested`) is another app's
    package copy; None for the same file (any case, with or without \\\\?\\), a link elsewhere, or
    Foxglove's own package."""
    real_plain = _plain(real)
    asked, got = _plain(requested).lower(), real_plain.lower()
    if asked == got:
        return None
    at = got.find(_PACKAGES)
    if at < 0:
        return None
    at += len(_PACKAGES)
    end = got.find('\\', at)
    if end < 0 or not got.startswith('\\localcache\\', end):
        return None
    package = real_plain[at:end]
    if package.lower().startswith(OWN_PACKAGE.lower() + '_'):
        return None
    return package, real_plain


def final_path(path):
    """The path Windows really opened for the existing file `path`, or None (not Windows, no
    such file, or Windows couldn't say)."""
    if os.name != 'nt':
        return None
    import ctypes
    from ctypes import wintypes
    k32 = ctypes.WinDLL('kernel32', use_last_error=True)
    k32.CreateFileW.restype = wintypes.HANDLE
    k32.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
                                wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    k32.GetFinalPathNameByHandleW.restype = wintypes.DWORD
    k32.GetFinalPathNameByHandleW.argtypes = [wintypes.HANDLE, wintypes.LPWSTR, wintypes.DWORD,
                                              wintypes.DWORD]
    k32.CloseHandle.argtypes = [wintypes.HANDLE]
    share = 0x1 | 0x2 | 0x4  # read, write, delete: never in another writer's way
    handle = k32.CreateFileW(str(path), 0, share, None, 3, 0x02000000, None)  # OPEN_EXISTING, BACKUP_SEMANTICS
    if handle in (None, wintypes.HANDLE(-1).value):
        return None
    try:
        buf = ctypes.create_unicode_buffer(1024)
        n = k32.GetFinalPathNameByHandleW(handle, buf, len(buf), 0)  # FILE_NAME_NORMALIZED
        return buf.value if 0 < n < len(buf) else None
    finally:
        k32.CloseHandle(handle)


def check(path, final=final_path):
    """(package, real path) when the existing file `path` is really another app's copy, else None."""
    real = final(path)
    return redirected(path, real) if real else None


def note(what, requested, found):
    """The line a tool prints when `what`, asked for at `requested`, went to the copy `found`."""
    package, real = found
    return (f'{what} is in another app\'s private copy: {real} (asked for {requested}). This shell runs '
            f'inside {package}\'s package, so Windows redirects AppData; run from a normal terminal '
            f'(Windows Terminal, Start menu) to write the real file')


def unc_path(path):
    r"""`path` (C:\Users\u\...) as the administrative share \\localhost\C$\Users\u\..., which a
    packaged app's file-system container does not redirect (#911); None for a path without a drive."""
    p = _plain(path)
    if len(p) < 3 or p[1] != ':' or p[2] != '\\' or not p[0].isalpha():
        return None
    return '\\\\localhost\\' + p[0].upper() + '$' + p[2:]


class Notice:
    """Checks a file once after a write and prints the note at most once a run."""

    def __init__(self, what, final=final_path):
        self.what, self.final, self.done, self.found = what, final, False, None

    def after_write(self, path, out):
        if self.done:
            return None
        self.done = True
        found = check(Path(path), self.final)
        if found:
            self.found = found
            print(note(self.what, path, found), file=out, flush=True)
        return found
