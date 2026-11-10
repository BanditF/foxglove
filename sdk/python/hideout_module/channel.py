"""Direct channels (docs/hideout-contract.md §8.4, v1.4 clarifications of #977), stdlib only.

The hub brokers the grant: a consumer's `channel.open` reaches the provider as `channel.create
{channel, consumer, nonce}`; the provider makes a one-time named pipe with a current-user ACL and
answers `{pipe}`; the hub hands `{pipe, nonce}` to the consumer and steps out. Frames never pass
through the hub. The same rules as the Rust SDK (`foxglove_plugin::channel`) and the contract
crate (`hideout_contract::channel`); both pass `tests/hideout-vectors/channels/`.

The wire, any platform:

- `is_channel_pipe(name)`: a consumer connects only to `\\\\.\\pipe\\hideout-ch-<rest>` (letters,
  digits, `.`, `_`, `-`; at most 256 characters);
- the consumer's first frame is the nonce (at most 256 bytes, compared in constant time);
- every frame is `u32` little-endian payload length + payload, at most 64 MiB;
- `read_stream(data, nonce)` reads a whole stream as the provider does (for the vectors);
- `encode_panel` / `decode_panel_payload`: the panel stream frames of §5.4 (12-byte header).

Pipes, Windows only:

- `serve(module, channel, nonce, stream)` is a provider's whole `channel.create`: it makes the
  pipe and, on a thread, waits up to 30 s for the consumer with the nonce, then calls
  `stream(conn)`; answer with `{"pipe": name}`;
- `ChannelPipe.create(name)` / `accept_nonce(nonce, timeout)` are the same in two steps;
- `connect(pipe, nonce)` is the consumer's end.

A `ChannelConn` has `read_frame()`, `write_frame(payload)`, `write(bytes)` and `close()`.
"""

import hmac
import os
import struct
import sys
import threading
import time
from collections import namedtuple

MAX_FRAME = 64 * 1024 * 1024
MAX_NONCE = 256
ACCEPT_TIMEOUT_S = 30
PIPE_PREFIX = "\\\\.\\pipe\\hideout-ch-"
MAX_PIPE_NAME = 256
# The panel stream frames' header and cap (§5.4, §8.4).
PANEL_HEADER = 12
MAX_PANEL_FRAME = 4 * 1024 * 1024
IMAGE, CANVAS, TEXT = 1, 2, 3

_NAME_CHARS = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")


def is_channel_pipe(name):
    """Whether `name` is a local channel pipe a consumer may connect to."""
    if not isinstance(name, str) or not name.startswith(PIPE_PREFIX) or len(name) > MAX_PIPE_NAME:
        return False
    rest = name[len(PIPE_PREFIX):]
    return bool(rest) and all(c in _NAME_CHARS for c in rest)


def nonce_matches(got, nonce):
    """Whether the first frame `got` (bytes) is `nonce`, in constant time."""
    want = nonce.encode("utf-8")
    return bool(want) and hmac.compare_digest(bytes(got), want)


def encode_frame(payload):
    """`u32` little-endian length + payload."""
    payload = bytes(payload)
    if len(payload) > MAX_FRAME:
        raise ValueError("frame too long")
    return struct.pack("<I", len(payload)) + payload


def read_stream(data, nonce):
    """A whole stream as the provider reads it: (frames after the nonce, end), where end is
    "closed", "nonce", "truncated" or "too-long" (the vectors' names)."""
    frames, at, first = [], 0, True
    while True:
        if at == len(data):
            return frames, "nonce" if first else "closed"
        if at + 4 > len(data):
            return frames, "nonce" if first else "truncated"
        (n,) = struct.unpack_from("<I", data, at)
        if n > (MAX_NONCE if first else MAX_FRAME):
            return frames, "nonce" if first else "too-long"
        if at + 4 + n > len(data):
            return frames, "nonce" if first else "truncated"
        payload = bytes(data[at + 4:at + 4 + n])
        at += 4 + n
        if first:
            if not nonce_matches(payload, nonce):
                return frames, "nonce"
            first = False
        else:
            frames.append(payload)


Image = namedtuple("Image", "seq width height pixels")
Canvas = namedtuple("Canvas", "seq colours")
Text = namedtuple("Text", "seq append")


class Dropped(ValueError):
    """A panel frame the reader drops (§8.4): its body does not match its header."""


def decode_panel_payload(p):
    """One panel stream payload (no length prefix) as Image, Canvas or Text; raises Dropped."""
    p = bytes(p)
    if len(p) < PANEL_HEADER:
        raise Dropped("shorter than the header")
    if len(p) > MAX_PANEL_FRAME:
        raise Dropped("over the frame size cap")
    kind, flags, count, height, reserved, seq = struct.unpack_from("<BBHHHI", p)
    if flags or reserved:
        raise Dropped("flags and reserved are 0")
    body = p[PANEL_HEADER:]

    def rgba(n):
        if len(body) != n * 4:
            raise Dropped("body length does not match header")
        return [list(body[i:i + 4]) for i in range(0, len(body), 4)]

    if kind == IMAGE:
        return Image(seq, count, height, rgba(count * height))
    if kind == CANVAS:
        return Canvas(seq, rgba(count))
    if kind == TEXT:
        if len(body) != count:
            raise Dropped("text length does not match header")
        try:
            return Text(seq, body.decode("utf-8"))
        except UnicodeDecodeError:
            raise Dropped("text isn't UTF-8") from None
    raise Dropped("unknown frame kind")


def decode_panel_wire(w):
    """One length-prefixed panel frame; raises Dropped."""
    w = bytes(w)
    if len(w) < 4:
        raise Dropped("no length prefix")
    if struct.unpack_from("<I", w)[0] != len(w) - 4:
        raise Dropped("length prefix does not match")
    return decode_panel_payload(w[4:])


def encode_panel(frame):
    """An Image, Canvas or Text as a length-prefixed panel frame."""
    if isinstance(frame, Image):
        kind, count, height = IMAGE, frame.width, frame.height
        body = b"".join(bytes(px) for px in frame.pixels)
    elif isinstance(frame, Canvas):
        kind, count, height = CANVAS, len(frame.colours), 0
        body = b"".join(bytes(c) for c in frame.colours)
    elif isinstance(frame, Text):
        body = frame.append.encode("utf-8")
        kind, count, height = TEXT, len(body), 0
    else:
        raise TypeError("not a panel frame")
    return encode_frame(struct.pack("<BBHHHI", kind, 0, count, height, 0, frame.seq) + body)


# ---- pipes (Windows) -------------------------------------------------------------------------

_counter = 0
_counter_lock = threading.Lock()


def unique_pipe_name(module, channel):
    """`\\\\.\\pipe\\hideout-ch-<pid>-<n>-<module>-<channel>`, unique per process and call; other
    characters than letters, digits, - and _ become -; cut to 256 characters."""
    global _counter
    with _counter_lock:
        n = _counter
        _counter += 1

    def clean(s):
        return "".join(c if (c.isascii() and c.isalnum()) or c in "-_" else "-" for c in s)

    return f"{PIPE_PREFIX}{os.getpid()}-{n}-{clean(module)}-{clean(channel)}"[:MAX_PIPE_NAME]


class ChannelError(OSError):
    """A channel that could not be opened: late (`timed_out`), wrong nonce, or refused."""

    def __init__(self, message, timed_out=False):
        super().__init__(message)
        self.timed_out = timed_out


if os.name == "nt":
    import ctypes
    import _winapi
    from ctypes import wintypes

    _k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _adv = ctypes.WinDLL("advapi32", use_last_error=True)

    _PIPE_ACCESS_DUPLEX = 0x3
    _FILE_FLAG_FIRST_PIPE_INSTANCE = 0x00080000
    _FILE_FLAG_OVERLAPPED = 0x40000000
    _PIPE_REJECT_REMOTE_CLIENTS = 0x8
    _GENERIC_RW = 0x80000000 | 0x40000000
    _OPEN_EXISTING = 3
    _SECURITY_SQOS_PRESENT = 0x00100000
    _SECURITY_IDENTIFICATION = 0x00010000
    _INVALID = ctypes.c_void_p(-1).value
    _ERROR_BROKEN_PIPE = 109
    _ERROR_IO_PENDING = 997
    _WAIT_TIMEOUT = 258
    _INFINITE = 0xFFFFFFFF

    class _SA(ctypes.Structure):
        _fields_ = [("nLength", wintypes.DWORD), ("lpSecurityDescriptor", ctypes.c_void_p),
                    ("bInheritHandle", wintypes.BOOL)]

    _k32.CreateNamedPipeW.restype = ctypes.c_void_p
    _k32.CreateNamedPipeW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.DWORD,
                                      wintypes.DWORD, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
    _k32.CreateFileW.restype = ctypes.c_void_p
    _k32.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p,
                                 wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
    _k32.GetCurrentProcess.restype = ctypes.c_void_p
    _k32.LocalFree.argtypes = [ctypes.c_void_p]
    _k32.CloseHandle.argtypes = [ctypes.c_void_p]
    _adv.OpenProcessToken.argtypes = [ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(ctypes.c_void_p)]
    _adv.GetTokenInformation.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD,
                                         ctypes.POINTER(wintypes.DWORD)]
    _adv.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p)]
    _adv.ConvertStringSecurityDescriptorToSecurityDescriptorW.argtypes = [
        wintypes.LPCWSTR, wintypes.DWORD, ctypes.POINTER(ctypes.c_void_p), ctypes.c_void_p]

    def _fail(what):
        raise ctypes.WinError(ctypes.get_last_error(), what)

    def current_user_sid():
        """The current user's SID string, from the process token."""
        token = ctypes.c_void_p()
        if not _adv.OpenProcessToken(_k32.GetCurrentProcess(), 0x8, ctypes.byref(token)):  # TOKEN_QUERY
            _fail("OpenProcessToken")
        try:
            size = wintypes.DWORD()
            _adv.GetTokenInformation(token, 1, None, 0, ctypes.byref(size))  # TokenUser
            buf = ctypes.create_string_buffer(size.value)
            if not _adv.GetTokenInformation(token, 1, buf, size, ctypes.byref(size)):
                _fail("GetTokenInformation")
            sid = ctypes.c_void_p.from_buffer(buf).value  # TOKEN_USER.User.Sid
            text = ctypes.c_void_p()
            if not _adv.ConvertSidToStringSidW(sid, ctypes.byref(text)):
                _fail("ConvertSidToStringSidW")
            try:
                return ctypes.wstring_at(text.value)
            finally:
                _k32.LocalFree(text)
        finally:
            _k32.CloseHandle(token)

    def _wait(ov, deadline):
        """Waits for an overlapped operation until `deadline` (monotonic) or forever; returns
        (n, err), or cancels and raises ChannelError(timed_out=True)."""
        if deadline is not None:
            ms = max(0, int((deadline - time.monotonic()) * 1000))
            if _winapi.WaitForMultipleObjects([ov.event], False, ms) == _WAIT_TIMEOUT:
                ov.cancel()
                n, err = ov.GetOverlappedResult(True)
                if err == 0:
                    return n, err
                raise ChannelError("the channel's consumer did not come in time", timed_out=True)
        return ov.GetOverlappedResult(True)

    class ChannelConn:
        """A connected channel, either end. One thread may read while another writes."""

        def __init__(self, handle):
            self._h = handle
            self._lock = threading.Lock()
            self.deadline = None

        def _read(self, n):
            try:
                ov, err = _winapi.ReadFile(self._h, n, overlapped=True)
            except OSError as e:
                if getattr(e, "winerror", None) == _ERROR_BROKEN_PIPE:
                    return b""
                raise
            got, err = _wait(ov, self.deadline)
            if err == _ERROR_BROKEN_PIPE:
                return b""
            return bytes(ov.getbuffer())[:got]

        def _read_exact(self, n):
            out = bytearray()
            while len(out) < n:
                part = self._read(min(n - len(out), 1 << 20))
                if not part:
                    return bytes(out)
                out += part
            return bytes(out)

        def write(self, data):
            data = bytes(data)
            with self._lock:
                while data:
                    ov, err = _winapi.WriteFile(self._h, data, overlapped=True)
                    n, err = _wait(ov, None)
                    if err:
                        raise OSError(err, "channel write failed")
                    data = data[n:]

        def write_frame(self, payload):
            self.write(encode_frame(payload))

        def read_frame(self):
            """One payload, or None when the other side closed between frames."""
            head = self._read_exact(4)
            if not head:
                return None
            if len(head) < 4:
                raise EOFError("the channel ended inside a length prefix")
            (n,) = struct.unpack("<I", head)
            if n > MAX_FRAME:
                raise ValueError("frame too long")
            body = self._read_exact(n)
            if len(body) < n:
                raise EOFError("the channel ended inside a frame")
            return body

        def read_nonce(self, nonce):
            """Checks the first frame is `nonce`; raises ChannelError (PermissionError-like)."""
            head = self._read_exact(4)
            n = struct.unpack("<I", head)[0] if len(head) == 4 else MAX_NONCE + 1
            got = self._read_exact(n) if n <= MAX_NONCE else b""
            if n > MAX_NONCE or len(got) < n or not nonce_matches(got, nonce):
                raise ChannelError("the channel's first frame is not the nonce")

        def close(self):
            h, self._h = self._h, None
            if h is not None:
                _winapi.CloseHandle(h)

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            self.close()

        def __del__(self):
            try:
                self.close()
            except Exception:
                pass

    class ChannelPipe:
        """A one-time channel pipe: one instance, the first-instance flag, remote clients
        rejected, a DACL for the current user's SID alone."""

        def __init__(self, handle, name):
            self.handle, self.name = handle, name

        @classmethod
        def create(cls, name):
            sd = ctypes.c_void_p()
            sddl = f"D:P(A;;GA;;;{current_user_sid()})"
            if not _adv.ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, ctypes.byref(sd), None):
                _fail("ConvertStringSecurityDescriptorToSecurityDescriptorW")
            try:
                sa = _SA(ctypes.sizeof(_SA), sd.value, False)
                h = _k32.CreateNamedPipeW(
                    name, _PIPE_ACCESS_DUPLEX | _FILE_FLAG_FIRST_PIPE_INSTANCE | _FILE_FLAG_OVERLAPPED,
                    _PIPE_REJECT_REMOTE_CLIENTS, 1, 65536, 65536, 0, ctypes.byref(sa))
            finally:
                _k32.LocalFree(sd)
            if h is None or h == _INVALID:
                _fail(f"CreateNamedPipeW {name}")
            return cls(h, name)

        def accept_nonce(self, nonce, timeout=ACCEPT_TIMEOUT_S):
            """Waits up to `timeout` seconds for the consumer and its nonce. Late or wrong raises
            ChannelError and closes the pipe: the channel is spent."""
            deadline = time.monotonic() + timeout
            h, self.handle = self.handle, None
            conn = ChannelConn(h)
            try:
                ov = _winapi.ConnectNamedPipe(h, overlapped=True)
                _wait(ov, deadline)
                conn.deadline = deadline
                conn.read_nonce(nonce)
                conn.deadline = None
                return conn
            except BaseException:
                conn.close()
                raise

        def close(self):
            h, self.handle = self.handle, None
            if h is not None:
                _winapi.CloseHandle(h)

    def connect(pipe, nonce):
        """The consumer's end: connects to a local `hideout-ch-` pipe (ValueError otherwise),
        with identification-level impersonation only, and sends the nonce first."""
        if not is_channel_pipe(pipe):
            raise ValueError("not a local hideout-ch- channel pipe")
        h = _k32.CreateFileW(pipe, _GENERIC_RW, 0, None, _OPEN_EXISTING,
                             _FILE_FLAG_OVERLAPPED | _SECURITY_SQOS_PRESENT | _SECURITY_IDENTIFICATION, None)
        if h is None or h == _INVALID:
            _fail(f"connect {pipe}")
        conn = ChannelConn(h)
        conn.write_frame(nonce.encode("utf-8"))
        return conn

    def serve(module, channel, nonce, stream, timeout=ACCEPT_TIMEOUT_S):
        """A provider's `channel.create`: makes the pipe, waits on a thread for the consumer with
        `nonce`, then calls `stream(conn)` there. Returns the pipe name to answer with."""
        pipe = ChannelPipe.create(unique_pipe_name(module, channel))

        def run():
            try:
                conn = pipe.accept_nonce(nonce, timeout)
            except OSError as e:
                print(f"hideout channel {channel}: not opened: {e}", file=sys.stderr, flush=True)
                return
            with conn:
                stream(conn)

        threading.Thread(target=run, name=f"hideout-channel-{channel}", daemon=True).start()
        return pipe.name
