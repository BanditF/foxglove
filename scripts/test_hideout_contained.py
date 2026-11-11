"""The Python SDK in an AppContainer (#1092), with real contained children: a copy of this
Python (exe, DLLs, the standard library zipped, a `._pth`, laid out like the embeddable
distribution) and the SDK, started in AppContainer profiles of their own, as the Rust tests do
with copied binaries. Window-free; no hub. The children run scripts/hideout_contained_child.py.

- a contained provider makes its pipe in its own namespace and answers the full name; this
  process reads it, after checking the server pid;
- a contained consumer reads this process's pipe, which lets its SID in (and without that ACE
  it can't open it);
- container to container, pid-bound both ways, and SID-bound only;
- connect_env, contained, opens a router pipe made as the hub makes it for a guest, and checks
  HIDEOUT_HUB_PID before it sends a byte.

Waits are on the children's stdout lines (their own 60 s exit is the fallback) and on the pipe's
connect; no polling."""

import json
import os
import shutil
import sys
import tempfile
import time
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from hideout_module import channel  # noqa: E402

CHILD = ROOT / "scripts" / "hideout_contained_child.py"
# Parts of the standard library a module never needs here, left out of the copy.
SKIP = {"test", "idlelib", "tkinter", "turtledemo", "ensurepip", "site-packages", "lib2to3",
        "pydoc_data", "__pycache__"}
ALL_APPLICATION_PACKAGES = "S-1-15-2-1"

if os.name == "nt":
    import ctypes
    import msvcrt
    import _winapi
    from ctypes import wintypes

    _k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _adv = ctypes.WinDLL("advapi32", use_last_error=True)
    _env = ctypes.WinDLL("userenv", use_last_error=True)

    class _SecurityCapabilities(ctypes.Structure):
        _fields_ = [("AppContainerSid", ctypes.c_void_p), ("Capabilities", ctypes.c_void_p),
                    ("CapabilityCount", wintypes.DWORD), ("Reserved", wintypes.DWORD)]

    class _StartupInfo(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("lpReserved", wintypes.LPWSTR), ("lpDesktop", wintypes.LPWSTR),
                    ("lpTitle", wintypes.LPWSTR), ("dwX", wintypes.DWORD), ("dwY", wintypes.DWORD),
                    ("dwXSize", wintypes.DWORD), ("dwYSize", wintypes.DWORD),
                    ("dwXCountChars", wintypes.DWORD), ("dwYCountChars", wintypes.DWORD),
                    ("dwFillAttribute", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
                    ("wShowWindow", wintypes.WORD), ("cbReserved2", wintypes.WORD),
                    ("lpReserved2", ctypes.c_void_p), ("hStdInput", ctypes.c_void_p),
                    ("hStdOutput", ctypes.c_void_p), ("hStdError", ctypes.c_void_p)]

    class _StartupInfoEx(ctypes.Structure):
        _fields_ = [("StartupInfo", _StartupInfo), ("lpAttributeList", ctypes.c_void_p)]

    class _ProcessInformation(ctypes.Structure):
        _fields_ = [("hProcess", ctypes.c_void_p), ("hThread", ctypes.c_void_p),
                    ("dwProcessId", wintypes.DWORD), ("dwThreadId", wintypes.DWORD)]

    _env.CreateAppContainerProfile.restype = ctypes.c_long
    _env.CreateAppContainerProfile.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.LPCWSTR,
                                               ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(ctypes.c_void_p)]
    _env.DeriveAppContainerSidFromAppContainerName.restype = ctypes.c_long
    _env.DeriveAppContainerSidFromAppContainerName.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(ctypes.c_void_p)]
    _env.DeleteAppContainerProfile.restype = ctypes.c_long
    _env.DeleteAppContainerProfile.argtypes = [wintypes.LPCWSTR]
    _adv.FreeSid.argtypes = [ctypes.c_void_p]
    _adv.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p)]
    _k32.LocalFree.argtypes = [ctypes.c_void_p]
    _k32.InitializeProcThreadAttributeList.argtypes = [ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD,
                                                       ctypes.POINTER(ctypes.c_size_t)]
    _k32.UpdateProcThreadAttribute.argtypes = [ctypes.c_void_p, wintypes.DWORD, ctypes.c_size_t,
                                               ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_void_p]
    _k32.DeleteProcThreadAttributeList.argtypes = [ctypes.c_void_p]
    _k32.CreateProcessW.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p,
                                    wintypes.BOOL, wintypes.DWORD, ctypes.c_void_p, wintypes.LPCWSTR,
                                    ctypes.c_void_p, ctypes.c_void_p]
    _k32.ResumeThread.argtypes = [ctypes.c_void_p]
    _k32.CloseHandle.argtypes = [ctypes.c_void_p]

    _ATTR_HANDLE_LIST = 0x00020002
    _ATTR_SECURITY_CAPABILITIES = 0x00020009
    _EXTENDED_STARTUPINFO_PRESENT = 0x00080000
    _CREATE_SUSPENDED = 0x4
    _CREATE_UNICODE_ENVIRONMENT = 0x400
    _CREATE_NO_WINDOW = 0x08000000
    _STARTF_USESTDHANDLES = 0x100
    _ALREADY_EXISTS = ctypes.c_long(0x800700B7).value

    def _check(ok, what):
        if not ok:
            raise ctypes.WinError(ctypes.get_last_error(), what)

    class Profile:
        """An AppContainer profile of its own for one child; deleted on close."""

        def __init__(self, name):
            self.name = name
            self.psid = ctypes.c_void_p()
            hr = _env.CreateAppContainerProfile(name, name, name, None, 0, ctypes.byref(self.psid))
            if hr == _ALREADY_EXISTS:
                hr = _env.DeriveAppContainerSidFromAppContainerName(name, ctypes.byref(self.psid))
            if hr:
                raise OSError(f"AppContainer profile {name}: HRESULT {hr & 0xFFFFFFFF:#x}")
            text = ctypes.c_void_p()
            _check(_adv.ConvertSidToStringSidW(self.psid, ctypes.byref(text)), "ConvertSidToStringSidW")
            self.sid = ctypes.wstring_at(text.value)
            _k32.LocalFree(text)

        def close(self):
            _adv.FreeSid(self.psid)
            _env.DeleteAppContainerProfile(self.name)

    class Child:
        """The copied Python running scripts/hideout_contained_child.py in `profile`'s
        AppContainer: its stdin, stdout and log are the only handles it inherits."""

        def __init__(self, lab, profile, env=None):
            self.log_path = lab / f"{profile.name}.log"
            r_in, self._w_in = _winapi.CreatePipe(None, 0)
            self._r_out, w_out = _winapi.CreatePipe(None, 0)
            log = open(self.log_path, "wb")
            h_log = msvcrt.get_osfhandle(log.fileno())
            inherit = (r_in, w_out, h_log)
            for h in inherit:
                os.set_handle_inheritable(h, True)

            size = ctypes.c_size_t()
            _k32.InitializeProcThreadAttributeList(None, 2, 0, ctypes.byref(size))
            attrs = ctypes.create_string_buffer(size.value)
            _check(_k32.InitializeProcThreadAttributeList(attrs, 2, 0, ctypes.byref(size)), "attribute list")
            caps = _SecurityCapabilities(profile.psid, None, 0, 0)
            handles = (ctypes.c_void_p * 3)(*inherit)
            try:
                _check(_k32.UpdateProcThreadAttribute(attrs, 0, _ATTR_SECURITY_CAPABILITIES, ctypes.byref(caps),
                                                      ctypes.sizeof(caps), None, None), "security capabilities")
                _check(_k32.UpdateProcThreadAttribute(attrs, 0, _ATTR_HANDLE_LIST, handles,
                                                      ctypes.sizeof(handles), None, None), "handle list")
                si = _StartupInfoEx()
                si.StartupInfo.cb = ctypes.sizeof(si)
                si.StartupInfo.dwFlags = _STARTF_USESTDHANDLES
                si.StartupInfo.hStdInput, si.StartupInfo.hStdOutput, si.StartupInfo.hStdError = inherit
                si.lpAttributeList = ctypes.cast(attrs, ctypes.c_void_p)
                # Windows sets an AppContainer up from the user's profile variables
                # (ERROR_ENVVAR_NOT_FOUND without them): this environment, without the
                # PYTHON* (the `._pth` ignores them anyway) and HIDEOUT_* ones, and `env`.
                block = {k: v for k, v in os.environ.items()
                         if not k.upper().startswith(("PYTHON", "HIDEOUT_"))}
                block.update(env or {})
                text = "".join(f"{k}={v}\0" for k, v in sorted(block.items(), key=lambda kv: kv[0].upper())) + "\0"
                env_block = (ctypes.c_wchar * len(text))(*text)  # NULs inside: not a C string
                exe = lab / "python" / "python.exe"
                cmd = ctypes.create_unicode_buffer(f'"{exe}" -X utf8 "{lab / "python" / "child.py"}"')
                pi = _ProcessInformation()
                _check(_k32.CreateProcessW(
                    str(exe), cmd, None, None, True,
                    _EXTENDED_STARTUPINFO_PRESENT | _CREATE_SUSPENDED | _CREATE_UNICODE_ENVIRONMENT | _CREATE_NO_WINDOW,
                    env_block, str(lab / "python"), ctypes.byref(si), ctypes.byref(pi)), "CreateProcessW")
            finally:
                _k32.DeleteProcThreadAttributeList(attrs)
                _winapi.CloseHandle(r_in)
                _winapi.CloseHandle(w_out)
                log.close()
            self._process, self.pid = pi.hProcess, pi.dwProcessId
            _k32.ResumeThread(pi.hThread)
            _k32.CloseHandle(pi.hThread)
            self._stdin = open(msvcrt.open_osfhandle(self._w_in, 0), "wb", buffering=0)
            self._stdout = open(msvcrt.open_osfhandle(self._r_out, os.O_RDONLY), "rb")
            self.hello = self.answer()

        def send(self, **cmd):
            self._stdin.write(json.dumps(cmd).encode() + b"\n")

        def answer(self):
            """The child's next line; its end (or its 60 s fallback) fails with its log."""
            line = self._stdout.readline()
            if not line:
                raise AssertionError(f"the contained child ended: {self.log_path.read_text(errors='replace')}")
            return json.loads(line)

        def close(self):
            self._stdin.close()
            if _winapi.WaitForSingleObject(self._process, 10000) != 0:  # the stdin end, else
                _winapi.TerminateProcess(self._process, 1)
            _winapi.CloseHandle(self._process)
            self._stdout.close()


def copy_python(dest):
    """This Python as an embeddable-style folder: exe and DLLs, the standard library zipped,
    a `._pth` (no site, no registry, no PYTHON* variables), the SDK and the child."""
    base = Path(sys.base_prefix)
    ver = f"python{sys.version_info.major}{sys.version_info.minor}"
    dest.mkdir(parents=True)
    shutil.copy2(base / "python.exe", dest)
    for dll in base.glob("*.dll"):
        shutil.copy2(dll, dest)
    shutil.copytree(base / "DLLs", dest / "DLLs", ignore=shutil.ignore_patterns("*.ico", "_tkinter*", "tcl*", "tk*"))
    lib = base / "Lib"
    with zipfile.ZipFile(dest / f"{ver}.zip", "w", zipfile.ZIP_STORED) as z:
        for path in lib.rglob("*.py"):
            rel = path.relative_to(lib)
            if not SKIP.intersection(rel.parts):
                z.write(path, rel.as_posix())
    shutil.copytree(ROOT / "sdk" / "python" / "hideout_module", dest / "sdk" / "hideout_module",
                    ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copy2(CHILD, dest / "child.py")
    (dest / f"{ver}._pth").write_text(f"{ver}.zip\nDLLs\nsdk\n.\n", encoding="utf-8")


def _accept_raw(pipe, timeout=20):
    """Starts accepting on a ChannelPipe without the nonce check (a stand-in router pipe);
    returns a function that waits for the client and gives its ChannelConn."""
    h, pipe.handle = pipe.handle, None
    ov = _winapi.ConnectNamedPipe(h, overlapped=True)

    def wait():
        channel._wait(ov, time.monotonic() + timeout)
        return channel.ChannelConn(h)

    return wait


@unittest.skipUnless(os.name == "nt", "AppContainers are Windows")
class Contained(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.lab = Path(tempfile.mkdtemp(prefix="hideout-pyac-"))
        copy_python(cls.lab / "python")
        # Read and execute for AppContainers on the copy only (inherited by what it holds).
        import subprocess
        subprocess.run(["icacls", str(cls.lab / "python"), "/grant", f"*{ALL_APPLICATION_PACKAGES}:(OI)(CI)RX", "/Q"],
                       check=True, stdout=subprocess.DEVNULL)
        cls.n = 0

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.lab, ignore_errors=True)

    def setUp(self):
        self.closers = []

    def tearDown(self):
        for close in reversed(self.closers):
            close()

    def child(self, env=None):
        type(self).n += 1
        profile = Profile(f"hideout.pytest.{os.getpid()}.{self.n}")
        self.closers.append(profile.close)
        c = Child(self.lab, profile, env)
        self.closers.append(c.close)
        self.assertEqual(c.hello["sid"], profile.sid, "the child runs in its own AppContainer")
        self.assertEqual(c.hello["pid"], c.pid)
        return c

    def test_contained_provider_answers_its_own_namespace(self):
        p = self.child()
        self.assertEqual(p.hello["ns"],
                         f"Sessions\\{channel.own_session()}\\AppContainerNamedObjects\\{p.hello['sid']}")
        p.send(do="provide", nonce="n-1", consumer_pid=os.getpid())
        pipe = p.answer()["pipe"]
        self.assertEqual(channel.parse_channel_pipe(pipe), (channel.own_session(), p.hello["sid"]), pipe)
        # A grant naming another server: nothing is sent, and the provider never sees a nonce.
        with self.assertRaises(PermissionError):
            channel.connect(pipe, "n-1", server_pid=os.getpid())
        self.assertFalse(p.answer()["accepted"], "the channel is spent")

        p.send(do="provide", nonce="n-2", consumer_pid=os.getpid())
        pipe = p.answer()["pipe"]
        with channel.connect_grant({"pipe": pipe, "nonce": "n-2", "pid": p.pid}) as conn:
            self.assertEqual([conn.read_frame() for _ in range(5)], [bytes([i]) * 3 for i in range(5)])
        self.assertEqual(p.answer(), {"accepted": True, "end": None})

    def test_contained_consumer_needs_its_ace(self):
        c = self.child()
        for consumer, ok in ((channel.Consumer(pid=c.pid), False),
                             (channel.Consumer(pid=c.pid, sid=c.hello["sid"]), True),
                             (channel.Consumer(sid=c.hello["sid"]), True)):
            with self.subTest(consumer=consumer):
                pipe = channel.ChannelPipe.create(channel.unique_pipe_name("test.module", "ac"), consumer)
                c.send(do="consume", pipe=pipe.name, nonce="n-c", pid=os.getpid())
                if not ok:
                    self.assertEqual(c.answer()["winerror"], 5, "no ACE for its SID: access denied")
                    pipe.close()
                    continue
                with pipe.accept_nonce("n-c", 20) as conn:
                    for i in range(5):
                        conn.write_frame(bytes([i]) * 2)
                    self.assertEqual(c.answer()["frames"], [(bytes([i]) * 2).hex() for i in range(5)])

    def test_container_to_container(self):
        consumer = self.child()
        provider = self.child()
        provider.send(do="provide", nonce="n-cc", consumer_pid=consumer.pid, consumer_sid=consumer.hello["sid"])
        pipe = provider.answer()["pipe"]
        self.assertEqual(channel.app_container_of(pipe), provider.hello["sid"])
        consumer.send(do="consume", pipe=pipe, nonce="n-cc", pid=provider.pid)
        self.assertEqual(consumer.answer()["frames"], [(bytes([i]) * 3).hex() for i in range(5)])
        self.assertEqual(provider.answer(), {"accepted": True, "end": None})

    def test_another_container_is_not_admitted(self):
        named, other = self.child(), self.child()
        provider = self.child()
        # The pipe lets `named` in by SID only; `other` can't even open it.
        provider.send(do="provide", nonce="n-x", consumer_sid=named.hello["sid"])
        pipe = provider.answer()["pipe"]
        other.send(do="consume", pipe=pipe, nonce="n-x", pid=provider.pid)
        self.assertEqual(other.answer()["winerror"], 5)
        # This process may open it (the user's ACE) but isn't the consumer: dropped unread.
        try:
            with channel.connect(pipe, "n-x", server_pid=provider.pid) as conn:
                self.assertIsNone(conn.read_frame())
        except BrokenPipeError:
            pass  # dropped before the nonce was even written: the same refusal
        self.assertFalse(provider.answer()["accepted"])

    def test_connect_env_checks_the_hub_pid(self):
        for hub_pid, sent in ((os.getpid(), True), (4, False)):
            with self.subTest(hub_pid=hub_pid):
                name = channel.unique_pipe_name("test.hub", "router")
                c = self.child({"HIDEOUT_PIPE": name, "HIDEOUT_TOKEN": "t-1", "HIDEOUT_INSTANCE": "i-1",
                                "HIDEOUT_HUB_PID": str(hub_pid)})
                # A router pipe as the hub makes it for a guest: client rights and a Low label.
                pipe = channel.ChannelPipe.create(name, channel.Consumer(sid=c.hello["sid"]))
                accepted = _accept_raw(pipe)
                c.send(do="router")
                got = c.answer()
                with accepted() as conn:
                    line = conn._read_exact(1)
                    if sent:
                        self.assertTrue(got["sent"])
                        line += conn._read_exact(200)
                        self.assertIn(b'"token":"t-1"', line)
                    else:
                        self.assertIn("PermissionError", got["error"])
                        self.assertEqual(line, b"", "nothing sent, the token least of all")


if __name__ == "__main__":
    unittest.main()
