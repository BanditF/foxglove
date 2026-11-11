r"""Direct channels (docs/hideout-contract.md §8.4, v1.4 clarifications of #977), stdlib only.

The hub brokers the grant: a consumer's `channel.open` reaches the provider as `channel.create
{channel, consumer, nonce, consumer_pid?, consumer_sid?}`; the provider makes a one-time named
pipe for that consumer and answers `{pipe}`; the hub hands `{pipe, nonce, pid?}` to the consumer
and steps out. Frames never pass through the hub. The same rules as the Rust SDK
(`foxglove_plugin::channel`) and the contract crate (`hideout_contract::channel`); both pass
`tests/hideout-vectors/channels/`.

The wire, any platform:

- `is_channel_pipe(name)` / `is_channel_pipe_in(name, session)`: a consumer connects only to
  `\\.\pipe\hideout-ch-<rest>` or, for a provider in an AppContainer (v1.10, #1092),
  `\\.\pipe\Sessions\<n>\AppContainerNamedObjects\<SID>\hideout-ch-<rest>`;
- the consumer's first frame is the nonce (at most 256 bytes, compared in constant time);
- every frame is `u32` little-endian payload length + payload, at most 64 MiB;
- `read_stream(data, nonce)` reads a whole stream as the provider does (for the vectors);
- `encode_panel` / `decode_panel_payload`: the panel stream frames of §5.4 (12-byte header);
- `pipe_sddl`, `own_namespace_names` and `admits_client`: the provider's ACL, its pipe names in
  an AppContainer and whom it admits (#1092), as pure functions.

Pipes, Windows only:

- `serve_create(module, create, stream)` is a provider's whole `channel.create`: it makes the
  pipe for the consumer `create` names and, on a thread, waits up to 30 s for that consumer
  with the nonce, then calls `stream(conn)`; answer with `{"pipe": name}`. In an AppContainer
  the pipe is made in the module's own namespace and `name` is the full one others open;
- `ChannelPipe.create(name, consumer)` / `accept_nonce(nonce, timeout)` are the same in two
  steps;
- `connect_grant(opened)` (or `connect(pipe, nonce, server_pid)`) is the consumer's end: it
  sends nothing unless the pipe's server is the grant's `pid`.

A `ChannelConn` has `read_frame()`, `write_frame(payload)`, `write(bytes)` and `close()`.
"""

import hmac
import os
import re
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


# (v1.10, #1092) A contained provider's pipe, in its own AppContainer namespace: the whole name,
# strictly, with exactly one segment after the SID (the token rule is checked below).
_CONTAINER_PIPE = re.compile(
    r"\\\\\.\\pipe\\Sessions\\(0|[1-9][0-9]*)\\AppContainerNamedObjects\\(S-1-15-2(?:-[0-9]+)+)\\([^\\/]*)\Z"
)
_PLAIN_DIR = "\\\\.\\pipe\\"


def parse_channel_pipe(name):
    """The whole name, strictly (#1092 review), as the Rust contract's `parse_channel_pipe`:
    `None` when it isn't a channel pipe; `(None, None)` for the plain form; `(session, SID)`
    for a contained provider's pipe in its own namespace. The one segment is `hideout-ch-` and
    letters, digits, `.`, `_` or `-`, not ending in `.`: no further separator, no `.`/`..`, no
    UNC walk."""
    if not isinstance(name, str) or len(name) > MAX_PIPE_NAME or not name.startswith(_PLAIN_DIR):
        return None
    if name.startswith(_PLAIN_DIR + "Sessions\\"):
        m = _CONTAINER_PIPE.match(name)
        if not m:
            return None
        where, leaf = (int(m.group(1)), m.group(2)), m.group(3)
    else:
        where, leaf = (None, None), name[len(_PLAIN_DIR):]
    if not leaf.startswith("hideout-ch-"):
        return None
    rest = leaf[len("hideout-ch-"):]
    if not rest or rest.endswith(".") or not all(c in _NAME_CHARS for c in rest):
        return None
    return where


def app_container_of(name):
    """For a contained provider's channel pipe, its AppContainer SID; else None."""
    p = parse_channel_pipe(name)
    return p[1] if p else None


def is_channel_pipe(name):
    """Whether `name` is a channel pipe a consumer may connect to, in any session (the syntax
    alone; a consumer uses `is_channel_pipe_in` with its own session)."""
    return parse_channel_pipe(name) is not None


def is_channel_pipe_in(name, session):
    """`is_channel_pipe`, and a contained provider's pipe only in `session`, the reader's own."""
    p = parse_channel_pipe(name)
    return p is not None and (p[0] is None or p[0] == session)


# What a client gets on a pipe it may only use (the router pipe, a contained consumer's
# channel): FILE_GENERIC_READ and FILE_GENERIC_WRITE without FILE_APPEND_DATA, which on a pipe
# is FILE_CREATE_PIPE_INSTANCE (contract §7.7). Clients open with exactly this.
PIPE_CLIENT_ACCESS = 0x0012019B


def is_app_container_sid(s):
    """`S-1-15-2-` and digits and dashes: an AppContainer's SID (safe in SDDL)."""
    if not isinstance(s, str) or not s.startswith("S-1-15-2-"):
        return False
    rest = s[len("S-1-15-2-"):]
    return bool(rest) and all(c in "0123456789-" for c in rest)


Consumer = namedtuple("Consumer", "pid sid", defaults=(None, None))
Consumer.__doc__ = """Who a channel pipe is for, from `channel.create` (v1.10, #1092): the consumer's
process id and, in an AppContainer, its SID. Both None (an older hub): any process of the user's
may connect, and the nonce alone decides."""


def consumer_of(create):
    """The Consumer a `channel.create` (a models.ChannelCreate or its JSON dict) names."""
    if isinstance(create, dict):
        return Consumer(create.get("consumer_pid"), create.get("consumer_sid"))
    return Consumer(getattr(create, "consumer_pid", None), getattr(create, "consumer_sid", None))


def pipe_sddl(user_sid, consumer_sid=None):
    """A channel pipe's descriptor (§8.4, v1.10): full access for the current user only,
    protected; with `consumer_sid` (an AppContainer consumer) also an allow ACE for that SID
    with `PIPE_CLIENT_ACCESS` and a Low label (no write up), which an AppContainer needs to
    open it. Without one no label is set. A `consumer_sid` that isn't an AppContainer SID is
    refused (ValueError): nothing else reaches the SDDL."""
    sddl = f"D:P(A;;GA;;;{user_sid})"
    if consumer_sid is None:
        return sddl
    if not is_app_container_sid(consumer_sid):
        raise ValueError(f"{consumer_sid!r} is not an AppContainer SID")
    return sddl + f"(A;;0x{PIPE_CLIENT_ACCESS:x};;;{consumer_sid})S:(ML;;NW;;;LW)"


def own_namespace_names(name, namespace):
    r"""(the name to create the pipe by, the name to answer with) for `name`
    (`\\.\pipe\hideout-ch-…`). Outside an AppContainer (`namespace` None) both are
    `name`. In one, `namespace` is `Sessions\<n>\AppContainerNamedObjects\<SID>`: the pipe is
    made as `\\.\pipe\LOCAL\hideout-ch-…` and others open the full name, cut to 256
    characters (unique names are ASCII)."""
    if namespace is None:
        return name, name
    leaf = name[len(_PLAIN_DIR):] if name.startswith(_PLAIN_DIR) else name
    where = _PLAIN_DIR + namespace + "\\"
    full = (where + leaf)[:MAX_PIPE_NAME]
    return _PLAIN_DIR + "LOCAL\\" + full[len(where):], full


def admits_client(consumer, pid, container_of):
    """Whether a channel pipe for `consumer` admits the connected client `pid`: None when it
    does, else the refusal in words. Its process id when the hub named one; else its
    AppContainer SID when the hub named one (`container_of(pid)`, the client's SID or None; an
    OSError reading it is a refusal); else anyone (an older hub: the nonce decides)."""
    if consumer.pid is not None:
        if consumer.pid != pid:
            return f"the channel's client is pid {pid}, not the consumer's {consumer.pid}"
        return None
    if consumer.sid is None:
        return None
    try:
        got = container_of(pid)
    except OSError as e:
        return f"the channel's client (pid {pid}): {e}"
    return None if got == consumer.sid else f"the channel's client (pid {pid}) isn't the consumer"


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
    _k32.OpenProcess.restype = ctypes.c_void_p
    _k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    _k32.GetNamedPipeClientProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.ULONG)]
    _k32.GetNamedPipeServerProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.ULONG)]
    _k32.GetFullPathNameW.restype = wintypes.DWORD
    _k32.GetFullPathNameW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.LPWSTR, ctypes.c_void_p]
    _k32.GetAppContainerNamedObjectPath.argtypes = [ctypes.c_void_p, ctypes.c_void_p, wintypes.ULONG,
                                                    wintypes.LPWSTR, ctypes.POINTER(wintypes.ULONG)]

    _TOKEN_QUERY = 0x8
    _PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    _TOKEN_USER, _TOKEN_SESSION_ID, _TOKEN_IS_APP_CONTAINER, _TOKEN_APP_CONTAINER_SID = 1, 12, 29, 31

    def _fail(what):
        raise ctypes.WinError(ctypes.get_last_error(), what)

    def _sid_string(sid):
        text = ctypes.c_void_p()
        if not _adv.ConvertSidToStringSidW(sid, ctypes.byref(text)):
            _fail("ConvertSidToStringSidW")
        try:
            return ctypes.wstring_at(text.value)
        finally:
            _k32.LocalFree(text)

    def _token_info(token, cls):
        size = wintypes.DWORD()
        _adv.GetTokenInformation(token, cls, None, 0, ctypes.byref(size))
        buf = ctypes.create_string_buffer(max(size.value, 8))
        if not _adv.GetTokenInformation(token, cls, buf, len(buf), ctypes.byref(size)):
            _fail("GetTokenInformation")
        return buf

    def _with_token(process, read):
        token = ctypes.c_void_p()
        if not _adv.OpenProcessToken(process, _TOKEN_QUERY, ctypes.byref(token)):
            _fail("OpenProcessToken")
        try:
            return read(token)
        finally:
            _k32.CloseHandle(token)

    def current_user_sid():
        """The current user's SID string, from the process token."""
        # TOKEN_USER.User.Sid is the buffer's first pointer.
        return _with_token(_k32.GetCurrentProcess(),
                           lambda t: _sid_string(ctypes.c_void_p.from_buffer(_token_info(t, _TOKEN_USER)).value))

    def _token_app_container(token):
        if not ctypes.c_uint32.from_buffer(_token_info(token, _TOKEN_IS_APP_CONTAINER)).value:
            return None
        # TOKEN_APPCONTAINER_INFORMATION.TokenAppContainer is the buffer's first pointer.
        return _sid_string(ctypes.c_void_p.from_buffer(_token_info(token, _TOKEN_APP_CONTAINER_SID)).value)

    def process_app_container(pid=None):
        """The AppContainer SID of process `pid` (this process when None), or None outside one.
        Raises OSError when its token can't be read."""
        if pid is None:
            return _with_token(_k32.GetCurrentProcess(), _token_app_container)
        p = _k32.OpenProcess(_PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
        if not p:
            _fail(f"OpenProcess {pid}")
        try:
            return _with_token(p, _token_app_container)
        finally:
            _k32.CloseHandle(p)

    def own_session():
        """This process's session id, from its token (as the Rust SDK: it works in an
        AppContainer too)."""
        return _with_token(_k32.GetCurrentProcess(),
                           lambda t: ctypes.c_uint32.from_buffer(_token_info(t, _TOKEN_SESSION_ID)).value)

    def own_pipe_namespace():
        r"""Where this process makes named pipes and others open them (§8.4, v1.10, #1092):
        None outside an AppContainer; in one, `Sessions\<n>\AppContainerNamedObjects\<SID>`
        (the path after `\\.\pipe\`), which it writes as `\\.\pipe\LOCAL\`."""
        if process_app_container() is None:
            return None
        buf = ctypes.create_unicode_buffer(512)
        n = wintypes.ULONG()
        if not _k32.GetAppContainerNamedObjectPath(None, None, len(buf), buf, ctypes.byref(n)):
            _fail("GetAppContainerNamedObjectPath")
        path = buf.value.strip("\\")
        # Windows answers relative to this session; others reach it under `Sessions\<n>\`.
        return path if path.startswith("Sessions\\") else f"Sessions\\{own_session()}\\{path}"

    def _pipe_pid(h, which):
        pid = wintypes.ULONG()
        if not which(h, ctypes.byref(pid)):
            _fail(which.__name__)
        return pid.value

    def close_handle(h):
        _k32.CloseHandle(h)

    def check_server(handle, pid):
        """Raises PermissionError unless the server of the connected pipe client `handle` is
        process `pid` (the grant's `pid`, or `HIDEOUT_HUB_PID`): nothing is sent before."""
        got = _pipe_pid(handle, _k32.GetNamedPipeServerProcessId)
        if got != pid:
            raise PermissionError(f"the pipe is served by pid {got}, not {pid}: nothing sent")

    def open_pipe(name, overlapped=False):
        """Opens a named pipe as a client, with `PIPE_CLIENT_ACCESS` (what an AppContainer is
        let in with: no create-instance) and identification-level impersonation only. Returns
        the handle."""
        flags = _SECURITY_SQOS_PRESENT | _SECURITY_IDENTIFICATION
        if overlapped:
            flags |= _FILE_FLAG_OVERLAPPED
        h = _k32.CreateFileW(name, PIPE_CLIENT_ACCESS, 0, None, _OPEN_EXISTING, flags, None)
        if h is None or h == _INVALID:
            _fail(f"connect {name}")
        return h

    def stays_a_local_pipe(pipe):
        """Whether Windows' own normalisation (GetFullPathNameW) leaves `pipe` exactly as it is,
        under `\\\\.\\pipe\\`: no `.`/`..` walk, no slash turned into a separator, no alias."""
        buf = ctypes.create_unicode_buffer(1024)
        n = _k32.GetFullPathNameW(pipe, len(buf), buf, None)
        if n == 0 or n >= len(buf):
            return False
        return buf.value == pipe and buf.value.startswith(_PLAIN_DIR)

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
            try:
                got, err = _wait(ov, self.deadline)
            except OSError as e:
                # A read still pending when the other end closes: GetOverlappedResult raises
                # ERROR_BROKEN_PIPE rather than returning it (#1006). The end, as above.
                if getattr(e, "winerror", None) == _ERROR_BROKEN_PIPE:
                    return b""
                raise
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
        rejected, a DACL for the current user's SID alone, plus the consumer's AppContainer
        SID when it has one (`pipe_sddl`). `name` is the name to answer `channel.create`
        with."""

        def __init__(self, handle, name, consumer=Consumer()):
            self.handle, self.name, self.consumer = handle, name, consumer

        @classmethod
        def create(cls, name, consumer=Consumer()):
            """Makes the pipe `name` (use `unique_pipe_name`) for `consumer` (§8.4, v1.10):
            a contained consumer's SID gets client rights and the pipe a Low label; accepting
            admits only the consumer's pid (or, without one, its SID); in an AppContainer the
            pipe is made in this process's own namespace. Fails if the name is taken."""
            sd = ctypes.c_void_p()
            sddl = pipe_sddl(current_user_sid(), consumer.sid)
            make, name = own_namespace_names(name, own_pipe_namespace())
            if not _adv.ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, ctypes.byref(sd), None):
                _fail("ConvertStringSecurityDescriptorToSecurityDescriptorW")
            try:
                sa = _SA(ctypes.sizeof(_SA), sd.value, False)
                h = _k32.CreateNamedPipeW(
                    make, _PIPE_ACCESS_DUPLEX | _FILE_FLAG_FIRST_PIPE_INSTANCE | _FILE_FLAG_OVERLAPPED,
                    _PIPE_REJECT_REMOTE_CLIENTS, 1, 65536, 65536, 0, ctypes.byref(sa))
            finally:
                _k32.LocalFree(sd)
            if h is None or h == _INVALID:
                _fail(f"CreateNamedPipeW {make}")
            return cls(h, name, consumer)

        def accept_nonce(self, nonce, timeout=ACCEPT_TIMEOUT_S):
            """Waits up to `timeout` seconds for the consumer and its nonce. Another client
            than the consumer, late or a wrong nonce raises ChannelError and closes the pipe:
            the channel is spent."""
            deadline = time.monotonic() + timeout
            h, self.handle = self.handle, None
            conn = ChannelConn(h)
            try:
                ov = _winapi.ConnectNamedPipe(h, overlapped=True)
                _wait(ov, deadline)
                # Before a byte is read: only the consumer the hub named (#1092).
                why = admits_client(self.consumer, _pipe_pid(h, _k32.GetNamedPipeClientProcessId),
                                    process_app_container)
                if why:
                    raise ChannelError(why)
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

    def connect(pipe, nonce, server_pid=None):
        """The consumer's end: connects to a channel pipe (ValueError otherwise: the whole name
        strictly, a contained provider's only in this session, and only a name Windows leaves
        as it is), with client rights and identification-level impersonation only. With
        `server_pid` (the grant's `pid`, v1.10) nothing is sent, the nonce least of all,
        unless the pipe's server is that process (PermissionError). Then the nonce first.
        Prefer `connect_grant`."""
        if not is_channel_pipe_in(pipe, own_session()) or not stays_a_local_pipe(pipe):
            raise ValueError("not a local hideout-ch- channel pipe")
        h = open_pipe(pipe, overlapped=True)
        conn = ChannelConn(h)
        try:
            if server_pid is not None:
                check_server(h, server_pid)
            conn.write_frame(nonce.encode("utf-8"))
        except BaseException:
            conn.close()
            raise
        return conn

    def connect_grant(opened):
        """`connect` for a `channel.open` result (a models.ChannelOpened or its JSON dict):
        `{pipe, nonce, pid?}`."""
        if isinstance(opened, dict):
            return connect(opened["pipe"], opened["nonce"], opened.get("pid"))
        return connect(opened.pipe, opened.nonce, getattr(opened, "pid", None))

    def serve_create(module, create, stream, timeout=ACCEPT_TIMEOUT_S):
        """`serve` for the hub's whole `channel.create` (a models.ChannelCreate or its dict):
        the pipe admits only the consumer it names, an AppContainer one included."""
        if isinstance(create, dict):
            channel, nonce = create["channel"], create["nonce"]
        else:
            channel, nonce = create.channel, create.nonce
        return serve(module, channel, nonce, stream, timeout, consumer_of(create))

    def serve(module, channel, nonce, stream, timeout=ACCEPT_TIMEOUT_S, consumer=Consumer()):
        """A provider's `channel.create`: makes the pipe for `consumer`, waits on a thread for
        it with `nonce`, then calls `stream(conn)` there. Returns the pipe name to answer with.
        Prefer `serve_create`, which passes the consumer the hub named."""
        pipe = ChannelPipe.create(unique_pipe_name(module, channel), consumer)

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
