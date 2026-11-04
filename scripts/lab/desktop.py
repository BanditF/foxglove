"""Desktop primitives for the 0.2 sitting's former eyes-and-hands steps (#618; stdlib only, Windows):
real input (SendInput: a mouse click, a key tap), lab-owned probe windows that record the clicks
and the focus they get, the tray icon's rectangle (Shell_NotifyIconGetRect), and screen captures of
one monitor (GDI, what DWM composes: the wallpaper too) with a small PNG writer for the report.

Every input goes through labkit.injecting (the "user is working" gate leaves it out) and puts the
mouse back where it was. Nothing here launches an app; the probe windows are the lab's own and
close with their `with` block. The pure parts (coordinates, the flicker analysis, the PNG) are
tested offline in scripts/test_sitting_lab.py.
"""
import ctypes
import struct
import threading
import time
import zlib
from pathlib import Path

from labkit import injecting

# ----------------------------------------------------------------------------- pure parts


def absolute(x, y, virtual):
    """Screen pixel (x, y) as SendInput's MOUSEEVENTF_ABSOLUTE | VIRTUALDESK coordinates (0-65535
    across the virtual desktop `virtual` = [left, top, width, height])."""
    left, top, width, height = virtual
    ax = round((x - left) * 65535 / max(width - 1, 1))
    ay = round((y - top) * 65535 / max(height - 1, 1))
    return max(0, min(65535, ax)), max(0, min(65535, ay))


def luma(bgra):
    """Mean luma (0-255) of a BGRA buffer (GetDIBits' 32-bit rows)."""
    n = len(bgra) // 4
    if not n:
        return 0.0
    b = sum(bgra[0::4])
    g = sum(bgra[1::4])
    r = sum(bgra[2::4])
    return (0.114 * b + 0.587 * g + 0.299 * r) / n


def change(a, b):
    """Mean absolute difference (0-255) of two same-size BGRA buffers, colour channels only."""
    n = min(len(a), len(b))
    if not n:
        return 0.0
    total = sum(abs(a[i] - b[i]) for i in range(n) if i % 4 != 3)
    return total / (n * 3 / 4)


def flicker(baseline, frames, dark_ratio=0.35, jump=3.0, floor=6.0):
    """The HUD's flicker (#586) from its captures: `baseline` the luma of captures before the game
    (the HUD running), `frames` [(t, luma, change from the previous frame)] after the Win key.
    A frame is a flicker when it goes dark (luma under `dark_ratio` of the baseline's mean: the
    wallpaper unloaded or reloading) or jumps (a change over `jump` times the baseline's largest
    frame-to-frame change, and at least `floor`: a flash). Returns (mean, [(t, why)])."""
    lumas = [l for l, _ in baseline]
    mean = sum(lumas) / len(lumas) if lumas else 0.0
    steady = max([c for _, c in baseline[1:]] or [0.0])
    limit = max(steady * jump, floor)
    seen = []
    for t, l, c in frames:
        if mean and l < mean * dark_ratio:
            seen.append((t, f'dark: luma {l:.0f} against {mean:.0f}'))
        elif c > limit:
            seen.append((t, f'jump: change {c:.0f} over {limit:.0f}'))
    return mean, seen


def png(width, height, rgb):
    """An 8-bit RGB PNG of `rgb` (rows top-down, width*3 bytes each) as bytes."""
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xFFFFFFFF)
    raw = b''.join(b'\x00' + rgb[y * width * 3:(y + 1) * width * 3] for y in range(height))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0)) +
            chunk(b'IDAT', zlib.compress(raw, 6)) + chunk(b'IEND', b''))


def bgra_to_rgb(bgra):
    out = bytearray(len(bgra) // 4 * 3)
    out[0::3] = bgra[2::4]
    out[1::3] = bgra[1::4]
    out[2::3] = bgra[0::4]
    return bytes(out)


def monitor_matches(device_id, piece):
    """Whether a monitor's device interface name (\\\\?\\DISPLAY#SAM0E3C#...) is the one Wallpaper
    Engine's key names (//?/DISPLAY#SAM0E3C#..., or a piece of it such as SAM)."""
    norm = lambda s: s.replace('\\', '/').lower()
    return norm(piece) in norm(device_id)


# ----------------------------------------------------------------------------- Windows


def _win():
    from ctypes import wintypes
    u, g, k = ctypes.windll.user32, ctypes.windll.gdi32, ctypes.windll.kernel32
    try:  # physical pixels, as the app reports its rects
        u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    except (AttributeError, OSError):
        pass
    return wintypes, u, g, k


INPUT_MOUSE, INPUT_KEYBOARD = 0, 1
MOUSEEVENTF_MOVE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP = 0x1, 0x2, 0x4
MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_VIRTUALDESK = 0x8000, 0x4000
KEYEVENTF_KEYUP = 0x2
VK_LWIN, VK_ESCAPE, VK_MENU = 0x5B, 0x1B, 0x12


class _MI(ctypes.Structure):
    _fields_ = [('dx', ctypes.c_long), ('dy', ctypes.c_long), ('mouseData', ctypes.c_ulong),
                ('dwFlags', ctypes.c_ulong), ('time', ctypes.c_ulong), ('dwExtraInfo', ctypes.c_size_t)]


class _KI(ctypes.Structure):
    _fields_ = [('wVk', ctypes.c_ushort), ('wScan', ctypes.c_ushort), ('dwFlags', ctypes.c_ulong),
                ('time', ctypes.c_ulong), ('dwExtraInfo', ctypes.c_size_t)]


class _U(ctypes.Union):
    _fields_ = [('mi', _MI), ('ki', _KI), ('pad', ctypes.c_byte * 32)]


class _INPUT(ctypes.Structure):
    _fields_ = [('type', ctypes.c_ulong), ('u', _U)]


def _send(*inputs):
    arr = (_INPUT * len(inputs))(*inputs)
    return ctypes.windll.user32.SendInput(len(inputs), arr, ctypes.sizeof(_INPUT))


def virtual_desktop():
    _, u, _, _ = _win()
    return [u.GetSystemMetrics(76), u.GetSystemMetrics(77), u.GetSystemMetrics(78), u.GetSystemMetrics(79)]


def cursor():
    wintypes, u, _, _ = _win()
    p = wintypes.POINT()
    u.GetCursorPos(ctypes.byref(p))
    return p.x, p.y


# Where the last click() pressed: (cursor (x, y), the window under it then), for a report's detail.
last_landed = None

# A person's mouse rests on a target before pressing it. Targets that track the hover (DWM's
# caption buttons, the Windows 11 taskbar's tray icons) ignored a press that came in the same
# batch as the move: the d7b5f4f batch's minimize and tray clicks went in (3 events) and did
# nothing, while a plain window's probe took them.
HOVER_S, PRESS_S = 0.25, 0.06


def click(x, y, what):
    """A real left click at screen pixel (x, y) through SendInput (Windows routes it as the mouse:
    hit test, window regions, WS_EX_TRANSPARENT): the move, a rest of HOVER_S, the press, PRESS_S,
    the release, then the mouse back where it was. Returns the number of input events Windows took
    (3 when all went in; 0 when UIPI or the secure desktop refused); last_landed says where the
    press was."""
    global last_landed
    wintypes, u, _, _ = _win()
    back = cursor()
    ax, ay = absolute(x, y, virtual_desktop())
    flags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK
    move = _INPUT(INPUT_MOUSE, _U(mi=_MI(ax, ay, 0, flags | MOUSEEVENTF_MOVE, 0, 0)))
    down = _INPUT(INPUT_MOUSE, _U(mi=_MI(ax, ay, 0, flags | MOUSEEVENTF_LEFTDOWN, 0, 0)))
    up = _INPUT(INPUT_MOUSE, _U(mi=_MI(ax, ay, 0, flags | MOUSEEVENTF_LEFTUP, 0, 0)))
    with injecting(f'{what} (lab click at {x},{y})'):
        sent = _send(move)
        time.sleep(HOVER_S)
        at = cursor()
        f = u.WindowFromPoint
        f.restype, f.argtypes = ctypes.c_void_p, [wintypes.POINT]
        under = f(wintypes.POINT(*at)) or 0
        sent += _send(down)
        time.sleep(PRESS_S)
        sent += _send(up)
        time.sleep(0.15)
        u.SetCursorPos(*back)
    last_landed = (at, under)
    return sent


def key(vk, what):
    """A real key tap (down, up) through SendInput."""
    down = _INPUT(INPUT_KEYBOARD, _U(ki=_KI(vk, 0, 0, 0, 0)))
    up = _INPUT(INPUT_KEYBOARD, _U(ki=_KI(vk, 0, KEYEVENTF_KEYUP, 0, 0)))
    with injecting(f'{what} (lab key 0x{vk:02x})'):
        return _send(down, up)


def tray_icon_rect(hwnd, ids=range(0, 16)):
    """The tray icon's rectangle [l, t, r, b] for the icon window `hwnd` (tray-icon numbers its
    icons from a process counter, so the first id that answers), or None (the icon is in the
    hidden-icons flyout, or gone)."""
    wintypes, _, _, _ = _win()

    class NOTIFYICONIDENTIFIER(ctypes.Structure):
        _fields_ = [('cbSize', ctypes.c_ulong), ('hWnd', ctypes.c_void_p), ('uID', ctypes.c_uint),
                    ('guidItem', ctypes.c_byte * 16)]
    f = ctypes.windll.shell32.Shell_NotifyIconGetRect
    for i in ids:
        nid = NOTIFYICONIDENTIFIER(ctypes.sizeof(NOTIFYICONIDENTIFIER), hwnd, i)
        r = wintypes.RECT()
        if f(ctypes.byref(nid), ctypes.byref(r)) == 0 and r.right > r.left:
            return [r.left, r.top, r.right, r.bottom]
    return None


def inside(box, rects):
    """Whether `box` [l, t, r, b] lies wholly on one of `rects` (the monitors)."""
    return any(r[0] <= box[0] and box[2] <= r[2] and r[1] <= box[1] and box[3] <= r[3] for r in rects)


def move(x, y, what):
    """A real mouse move to (x, y) (SendInput); the mouse stays there (put it back with move)."""
    ax, ay = absolute(x, y, virtual_desktop())
    m = _INPUT(INPUT_MOUSE, _U(mi=_MI(ax, ay, 0, MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE, 0, 0)))
    with injecting(f'{what} (lab move to {x},{y})'):
        return _send(m)


def tray_icon_shown(hwnd, settle_s=1.5):
    """The tray icon's rectangle once it is on screen: an auto-hidden taskbar keeps it a few pixels
    past the monitor's edge, so the mouse goes to that edge first (as a person's would) and the
    rectangle is read again after the taskbar slid in. None when it never shows."""
    rect = tray_icon_rect(hwnd)
    if rect is None:
        return None
    screens = [r for _, r in monitors()]
    if inside(rect, screens):
        return rect
    cx, cy = (rect[0] + rect[2]) // 2, (rect[1] + rect[3]) // 2
    edge = next((r for r in screens if r[0] <= cx < r[2]), None)
    if edge is None:
        return None
    y = min(max(cy, edge[1]), edge[3] - 1)
    move(cx, y, 'reveal the auto-hidden taskbar')
    end = time.time() + settle_s
    while time.time() < end:
        time.sleep(0.1)
        rect = tray_icon_rect(hwnd)
        if rect and inside(rect, screens):
            return rect
    return None


def monitors():
    """[(device interface name, [l, t, r, b])] of the monitors, in Windows' order."""
    wintypes, u, _, _ = _win()

    class MONITORINFOEXW(ctypes.Structure):
        _fields_ = [('cbSize', ctypes.c_ulong), ('rcMonitor', wintypes.RECT), ('rcWork', wintypes.RECT),
                    ('dwFlags', ctypes.c_ulong), ('szDevice', ctypes.c_wchar * 32)]

    class DISPLAY_DEVICEW(ctypes.Structure):
        _fields_ = [('cb', ctypes.c_ulong), ('DeviceName', ctypes.c_wchar * 32), ('DeviceString', ctypes.c_wchar * 128),
                    ('StateFlags', ctypes.c_ulong), ('DeviceID', ctypes.c_wchar * 128), ('DeviceKey', ctypes.c_wchar * 128)]
    out = []
    proc = ctypes.WINFUNCTYPE(wintypes.BOOL, ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(wintypes.RECT), ctypes.c_void_p)

    def each(hmon, _hdc, _rect, _data):
        mi = MONITORINFOEXW()
        mi.cbSize = ctypes.sizeof(MONITORINFOEXW)
        u.GetMonitorInfoW(ctypes.c_void_p(hmon), ctypes.byref(mi))
        dd = DISPLAY_DEVICEW()
        dd.cb = ctypes.sizeof(DISPLAY_DEVICEW)
        u.EnumDisplayDevicesW(mi.szDevice, 0, ctypes.byref(dd), 1)  # EDD_GET_DEVICE_INTERFACE_NAME
        r = mi.rcMonitor
        out.append((dd.DeviceID or mi.szDevice, [r.left, r.top, r.right, r.bottom]))
        return True
    u.EnumDisplayMonitors(None, None, proc(each), 0)
    return out


def grab(rect, scale=8):
    """One capture of screen rect [l, t, r, b], shrunk `scale` times (HALFTONE): (w, h, BGRA)."""
    _, u, g, _ = _win()
    H, I = ctypes.c_void_p, ctypes.c_int
    u.GetDC.restype, u.GetDC.argtypes = H, [H]
    u.ReleaseDC.argtypes = [H, H]
    g.CreateCompatibleDC.restype, g.CreateCompatibleDC.argtypes = H, [H]
    g.CreateCompatibleBitmap.restype, g.CreateCompatibleBitmap.argtypes = H, [H, I, I]
    g.SelectObject.restype, g.SelectObject.argtypes = H, [H, H]
    g.SetStretchBltMode.argtypes = [H, I]
    g.StretchBlt.argtypes = [H, I, I, I, I, H, I, I, I, I, ctypes.c_ulong]
    g.GetDIBits.argtypes = [H, H, ctypes.c_uint, ctypes.c_uint, H, H, ctypes.c_uint]
    g.DeleteObject.argtypes = g.DeleteDC.argtypes = [H]
    l, t, r, b = rect
    w, h = max((r - l) // scale, 1), max((b - t) // scale, 1)
    screen = u.GetDC(None)
    mem = g.CreateCompatibleDC(screen)
    bmp = g.CreateCompatibleBitmap(screen, w, h)
    old = g.SelectObject(mem, bmp)
    try:
        g.SetStretchBltMode(mem, 4)  # HALFTONE
        g.StretchBlt(mem, 0, 0, w, h, screen, l, t, r - l, b - t, 0x00CC0020 | 0x40000000)  # SRCCOPY|CAPTUREBLT

        class BITMAPINFOHEADER(ctypes.Structure):
            _fields_ = [('biSize', ctypes.c_uint32), ('biWidth', ctypes.c_int32), ('biHeight', ctypes.c_int32),
                        ('biPlanes', ctypes.c_uint16), ('biBitCount', ctypes.c_uint16), ('biCompression', ctypes.c_uint32),
                        ('biSizeImage', ctypes.c_uint32), ('biXPelsPerMeter', ctypes.c_int32),
                        ('biYPelsPerMeter', ctypes.c_int32), ('biClrUsed', ctypes.c_uint32), ('biClrImportant', ctypes.c_uint32)]
        bi = BITMAPINFOHEADER(ctypes.sizeof(BITMAPINFOHEADER), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
        buf = ctypes.create_string_buffer(w * h * 4)
        g.SelectObject(mem, old)
        g.GetDIBits(mem, bmp, 0, h, ctypes.addressof(buf), ctypes.addressof(bi), 0)
        return w, h, buf.raw
    finally:
        g.DeleteObject(bmp)
        g.DeleteDC(mem)
        u.ReleaseDC(None, screen)


def save_png(path, w, h, bgra):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_bytes(png(w, h, bgra_to_rgb(bgra)))


class Probe:
    """A lab-owned window at `rect` on its own thread, recording what reaches it: left clicks (as
    WM_LBUTTONDOWN, client coordinates) and activation. `game=True` makes it a stand-in for a
    full-screen game (#586): borderless, topmost, black, covering `rect`, and it minimizes itself
    when it loses the focus, as an exclusive full-screen game does on the Win key. Use as a
    context manager; the window closes at the end of the block."""
    WM_DESTROY, WM_CLOSE, WM_LBUTTONDOWN, WM_ACTIVATE, WM_ERASEBKGND = 0x0002, 0x0010, 0x0201, 0x0006, 0x0014

    def __init__(self, rect, title='Foxglove lab probe', game=False):
        self.rect, self.title, self.game = rect, title, game
        self.clicks, self.activations = [], []
        self.hwnd = None
        self._ready = threading.Event()

    def __enter__(self):
        self._thread = threading.Thread(target=self._run, name='lab-probe', daemon=True)
        self._thread.start()
        if not self._ready.wait(10):
            raise RuntimeError('the probe window did not open')
        return self

    def __exit__(self, *exc):
        if self.hwnd:
            ctypes.windll.user32.PostMessageW(ctypes.c_void_p(self.hwnd), self.WM_CLOSE, 0, 0)
        self._thread.join(10)

    def _run(self):
        wintypes, u, g, k = _win()
        LRESULT = ctypes.c_ssize_t
        WNDPROC = ctypes.WINFUNCTYPE(LRESULT, ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t)
        u.DefWindowProcW.restype = LRESULT
        u.DefWindowProcW.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]

        def proc(hwnd, msg, wp, lp):
            if msg == self.WM_LBUTTONDOWN:
                self.clicks.append((time.time(), lp & 0xFFFF, (lp >> 16) & 0xFFFF))
            elif msg == self.WM_ACTIVATE:
                active = (wp & 0xFFFF) != 0
                self.activations.append((time.time(), active))
                if self.game and not active:
                    u.ShowWindow(ctypes.c_void_p(hwnd), 6)  # SW_MINIMIZE, as a full-screen game does on the Win key
            elif msg == self.WM_CLOSE:
                u.DestroyWindow(ctypes.c_void_p(hwnd))
                return 0
            elif msg == self.WM_DESTROY:
                u.PostQuitMessage(0)
                return 0
            return u.DefWindowProcW(hwnd, msg, wp, lp)
        self._proc = WNDPROC(proc)  # kept alive with the window

        class WNDCLASSW(ctypes.Structure):
            _fields_ = [('style', ctypes.c_uint), ('lpfnWndProc', WNDPROC), ('cbClsExtra', ctypes.c_int),
                        ('cbWndExtra', ctypes.c_int), ('hInstance', ctypes.c_void_p), ('hIcon', ctypes.c_void_p),
                        ('hCursor', ctypes.c_void_p), ('hbrBackground', ctypes.c_void_p),
                        ('lpszMenuName', ctypes.c_wchar_p), ('lpszClassName', ctypes.c_wchar_p)]
        name = f'FoxgloveLabProbe{id(self)}'
        g.GetStockObject.restype = ctypes.c_void_p
        brush = g.GetStockObject(4 if self.game else 0)  # BLACK_BRUSH / WHITE_BRUSH
        k.GetModuleHandleW.restype = ctypes.c_void_p
        wc = WNDCLASSW(0, self._proc, 0, 0, k.GetModuleHandleW(None), None, None, brush, None, name)
        u.RegisterClassW(ctypes.byref(wc))
        l, t, r, b = self.rect
        ex = 0x00000080 | (0x00000008 if self.game else 0)  # WS_EX_TOOLWINDOW (+ WS_EX_TOPMOST)
        u.CreateWindowExW.restype = ctypes.c_void_p
        u.CreateWindowExW.argtypes = [ctypes.c_ulong, ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_ulong, ctypes.c_int,
                                      ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p,
                                      ctypes.c_void_p, ctypes.c_void_p]
        self.hwnd = u.CreateWindowExW(ex, name, self.title, 0x80000000 | 0x10000000,  # WS_POPUP | WS_VISIBLE
                                      l, t, r - l, b - t, None, None, wc.hInstance, None)
        self._ready.set()
        msg = wintypes.MSG()
        while u.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            u.TranslateMessage(ctypes.byref(msg))
            u.DispatchMessageW(ctypes.byref(msg))
        u.UnregisterClassW(name, ctypes.c_void_p(wc.hInstance))

    def focus(self):
        """Brings the probe to the foreground as a user's click on it would: a real click on the
        probe itself (it is the lab's own window, so the click lands nowhere else). Whether it
        got the foreground (bounded wait)."""
        l, t, r, b = self.rect
        click(l + min(40, (r - l) // 2), t + min(40, (b - t) // 2), 'click on the lab probe to focus it')
        return self.foreground(3)

    def foreground(self, seconds):
        """Whether the probe is (or becomes, within `seconds`) the foreground window."""
        f = ctypes.windll.user32.GetForegroundWindow
        f.restype = ctypes.c_void_p
        end = time.time() + seconds
        while True:
            if f() == self.hwnd:
                return True
            if time.time() > end:
                return False
            time.sleep(0.05)

    def iconic(self):
        return bool(ctypes.windll.user32.IsIconic(ctypes.c_void_p(self.hwnd)))
