"""Shared pieces for the app-level board checks in scripts/lab (stdlib only, Windows).

Every check runs a Foxglove build on a temp FOXGLOVE_CONFIG_DIR; the user's real settings are never
touched. Whatever Foxglove the user had running is stopped first and started again after, by path.
"""
import ctypes, json, os, re, shutil, subprocess, sys, tempfile, time
from contextlib import contextmanager
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
# Absolute even when CARGO_TARGET_DIR is relative: it is relative to the repository root, as cargo
# runs there (#618: a relative tool path crashed overlay_tray's lab cases).
TARGET = REPO / os.environ.get('CARGO_TARGET_DIR', 'target')
REAL_CONFIG = Path(os.environ['APPDATA']) / 'Foxglove' / 'config'
REAL_STORE = Path(os.environ['APPDATA']) / 'Foxglove' / 'foxglove.toml'


def tool(path, flag='--lab', kind='file'):
    """`path` (a build, a tool or a folder the lab runs) as an absolute path: relative paths are
    taken from the repository root, whatever the caller's working directory (#618: release-test.ps1
    passed `--lab target/release/foxglove-lab.exe`, and starting it from a child with another cwd
    raised FileNotFoundError mid-run, so the later cases never ran). Raises ValueError naming the
    flag when it isn't there."""
    p = Path(path)
    if not p.is_absolute():
        p = REPO / p
    p = Path(os.path.normpath(p))
    ok = p.is_dir() if kind == 'dir' else p.is_file()
    if not ok:
        what = 'folder' if kind == 'dir' else 'file'
        raise ValueError(f'{flag} {path}: no such {what} ({p}; relative paths are from the repo root {REPO})')
    return p


def tool_arg(flag, kind='file'):
    """An argparse `type=` for `flag` through tool(): a missing path stops the script before
    anything runs, with argparse's exit 2 (BLOCKED) and the error naming the flag."""
    import argparse

    def parse(value):
        try:
            return tool(value, flag, kind)
        except ValueError as e:
            raise argparse.ArgumentTypeError(str(e)) from None
    parse.__name__ = flag.lstrip('-')
    return parse


def utf8(stream):
    """`stream` writing UTF-8 (unencodable text replaced, never raised): a piped or redirected
    console stream on Windows is cp1252, and a "→" in a log line crashed the checks (#618)."""
    try:
        stream.reconfigure(encoding='utf-8', errors='replace')
    except (AttributeError, ValueError):  # not a TextIOWrapper, or already closed
        pass
    return stream


utf8(sys.stdout)
utf8(sys.stderr)


def ps(script):
    """A PowerShell snippet's output, decoded as the UTF-8 it is told to write."""
    script = '[Console]::OutputEncoding = [Text.Encoding]::UTF8; ' + script
    return subprocess.run(['powershell', '-NoProfile', '-Command', script], capture_output=True, text=True,
                          encoding='utf-8', errors='replace').stdout


def locked():
    """A locked PC sends keys to the secure desktop: no global hotkey fires."""
    return not ctypes.windll.user32.OpenInputDesktop(0, False, 0x0100)


# The harness's own input (#618). Some lab steps make Windows see input: foxglove-lab's key presses
# through the keyboard (D7 05 positions, INVOKE, the typing sink) and these scripts' tray clicks and
# window posts. GetLastInputInfo can't tell them from the user's, so the "the user is working" gate
# (PC idle >= 2 min before a soak or a typing step) stopped the lab's own later steps. Every
# injection is recorded here, by foxglove-lab and labkit alike, one line each:
#     <start> <end> <the user's last input before it, or -> <what>     (Unix seconds)
# The rule: input from the injection's start (less 0.1 s) to 2 s after its end is the harness's.
# When the PC's last input falls in such a window, the user's last input is the one recorded when
# that injection began. Any other input, before or after, is the user's and still blocks.
INJECTIONS = Path(os.environ.get('FOXGLOVE_LAB_INJECTIONS') or
                  Path(os.environ.get('LOCALAPPDATA', tempfile.gettempdir())) / 'Foxglove' / 'lab-injections.log')
HARNESS_AFTER = 2.0
HARNESS_BEFORE = 0.1
KEEP = 3600  # records older than an hour are dropped when the file is rewritten


def last_input():
    """When this PC last had keyboard or mouse input (Unix seconds; GetLastInputInfo), or None."""
    if not hasattr(ctypes, 'windll'):
        return None

    class LASTINPUTINFO(ctypes.Structure):
        _fields_ = [('cbSize', ctypes.c_uint), ('dwTime', ctypes.c_uint)]
    info = LASTINPUTINFO(ctypes.sizeof(LASTINPUTINFO), 0)
    if not ctypes.windll.user32.GetLastInputInfo(ctypes.byref(info)):
        return None
    ago = ((ctypes.windll.kernel32.GetTickCount() - info.dwTime) & 0xFFFFFFFF) / 1000.0
    return time.time() - ago


def _line(start, end, before, what):
    return f"{start:.3f} {end:.3f} {'-' if before is None else f'{before:.3f}'} {what.replace(chr(10), ' ')}\n"


def injections(path=None):
    """The recorded injections, oldest first: [(start, end, user_last or None, what)]."""
    try:
        text = Path(path or INJECTIONS).read_text(encoding='utf-8', errors='replace')
    except OSError:
        return []
    out = []
    for line in text.splitlines():
        parts = line.split(' ', 3)
        try:
            out.append((float(parts[0]), float(parts[1]), None if parts[2] == '-' else float(parts[2]),
                        parts[3] if len(parts) > 3 else ''))
        except (IndexError, ValueError):
            continue
    return out


def user_last_input(raw, records):
    """The user's last input given the PC's (`raw`) and the harness's `records`, by the rule
    above: (when or None, the injection it was discounted for or None)."""
    if raw is None:
        return None, None
    for r in reversed(records):  # newest first
        start, end, before, _ = r
        if start - HARNESS_BEFORE <= raw <= end + HARNESS_AFTER:
            return before, r
    return raw, None


def user_idle(now=None, raw=None, path=None):
    """Seconds since the user's own last input (the harness's left out) and the injection
    discounted (or None). (None, None) when Windows can't say."""
    now = time.time() if now is None else now
    raw = last_input() if raw is None else raw
    when, r = user_last_input(raw, injections(path))
    if when is None:
        # Nothing known before that injection: idle since it began, the cautious reading.
        return (None, None) if r is None else (now - r[0], r)
    return now - when, r


def record_injection(start, end, before, what, path=None):
    """Appends one injection record (and drops those older than KEEP once the file passes 64 KiB)."""
    path = Path(path or INJECTIONS)
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists() and path.stat().st_size > 64 * 1024:
            keep = [r for r in injections(path) if r[1] >= time.time() - KEEP]
            path.write_text(''.join(_line(*r) for r in keep), encoding='utf-8')
        with open(path, 'a', encoding='utf-8') as f:
            f.write(_line(start, end, before, what))
    except OSError as e:
        print(f'  (could not record the injection in {path}: {e})', flush=True)


@contextmanager
def injecting(what, path=None):
    """Records the input made inside the block as the harness's (the rule above)."""
    before, _ = user_last_input(last_input(), injections(path))
    start = time.time()
    try:
        yield
    finally:
        record_injection(start, time.time(), before, what, path)


def require_user_idle(seconds):
    """Exit 2 (BLOCKED) when the user had input within `seconds` (the harness's own left out)."""
    idle, r = user_idle()
    if idle is not None and idle < seconds:
        print(f'BLOCKED: the PC had user input {idle:.0f} s ago (needs {seconds:.0f} s idle)', flush=True)
        print('RESULT BLOCKED', flush=True)
        sys.exit(2)
    note = f" (later input was the harness's: {r[3]})" if r else ''
    print(f'user idle: {"unknown" if idle is None else f"{idle:.0f} s"}{note}', flush=True)


def remembered_keyboard(store_text):
    """The `[connection.last]` table of a settings store (the keyboard the overlay connects to by
    itself on start, last_device.rs) as TOML text for a temp store, or '' when there is none.
    Without it a test app's overlay never connects: it draws no keys, so with Settings focused
    it has nothing to take the mouse over (#549), and before #618's fix closing Settings quit it."""
    import tomllib
    try:
        last = tomllib.loads(store_text).get('connection', {}).get('last')
    except tomllib.TOMLDecodeError:
        return ''
    if not isinstance(last, dict) or not {'vid', 'pid', 'transport', 'id'} <= set(last):
        return ''
    return (f"\n[connection.last]\nid = {json.dumps(str(last['id']))}\npid = {int(last['pid'])}\n"
            f"transport = {json.dumps(str(last['transport']))}\nvid = {int(last['vid'])}\n")


def users_keyboard():
    """remembered_keyboard() of the user's own store (read only)."""
    try:
        return remembered_keyboard(REAL_STORE.read_text(encoding='utf-8', errors='replace'))
    except OSError:
        return ''


def temp_config(prefix, store=''):
    """A temp config folder with the user's keymap metadata (so behaviors resolve) and `store` as
    its foxglove.toml."""
    cfg = Path(tempfile.mkdtemp(prefix=prefix)) / 'config'
    cfg.mkdir()
    for k in REAL_CONFIG.glob('keymap-*.studio'):
        shutil.copy(k, cfg)
    (cfg / 'foxglove.toml').write_text('schema = 1\n' + store, encoding='utf-8')
    return cfg


def require_app_ok():
    """Lab cases that launch Foxglove run only with On-call's OK (FOXGLOVE_APP_OK=1): exit 2
    (BLOCKED) otherwise, before anything starts."""
    if os.environ.get('FOXGLOVE_APP_OK') != '1':
        print('BLOCKED: this case launches Foxglove; On-call runs it with FOXGLOVE_APP_OK=1', flush=True)
        print('RESULT BLOCKED', flush=True)
        sys.exit(2)


def log_lines(cfg, piece):
    """The lines of the test app's foxglove.log (in its temp config) containing `piece`."""
    log = Path(cfg) / 'logs' / 'foxglove.log'
    return [l for l in log.read_text(encoding='utf-8', errors='replace').splitlines() if piece in l] if log.exists() else []


class Windows:
    """The test app's own windows, found by class among the top-level windows of its PID (the
    lab hooks of #612/#618: no mouse, no focus taken from the user's windows by these posts)."""
    WM_DISPLAYCHANGE, WM_LBUTTONUP, WM_USER_TRAYICON, SW_MINIMIZE = 0x007E, 0x0202, 6002, 6
    GWL_EXSTYLE, GA_ROOT = -20, 2

    def __init__(self, pid):
        self.pid, self.u = pid, ctypes.windll.user32
        # Physical pixels, as the per-monitor-aware app reports its rects (#549 probes): an unaware
        # lab process would get scaled coordinates on a 125/150 % display. Fails harmlessly when
        # the awareness is already set.
        try:
            self.u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
        except (AttributeError, OSError):
            pass

    def find(self, cls):
        """The pid's top-level windows of class `cls` (shown or not)."""
        from ctypes import wintypes
        found = []
        proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

        def each(hwnd, _):
            pid = wintypes.DWORD()
            self.u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            name = ctypes.create_unicode_buffer(256)
            self.u.GetClassNameW(hwnd, name, 256)
            if pid.value == self.pid and name.value == cls:
                found.append(hwnd)
            return True
        self.u.EnumWindows(proc(each), 0)
        return found

    def rect(self, hwnd):
        from ctypes import wintypes
        r = wintypes.RECT()
        self.u.GetWindowRect(hwnd, ctypes.byref(r))
        return [r.left, r.top, r.right, r.bottom]

    def exstyle(self, hwnd):
        """The window's extended style (GWL_EXSTYLE): WS_EX_TRANSPARENT | WS_EX_LAYERED is how
        winit makes a window click-through (set_cursor_hittest(false))."""
        f = self.u.GetWindowLongPtrW
        f.restype, f.argtypes = ctypes.c_ssize_t, [ctypes.c_void_p, ctypes.c_int]
        return f(hwnd, self.GWL_EXSTYLE) & 0xFFFFFFFF

    def visible(self, hwnd):
        return bool(self.u.IsWindowVisible(hwnd))

    def region_box(self, hwnd):
        """The window region's bounding box in screen pixels (SetWindowRgn, #405), or None when
        the window has no region (the whole window hit-tests)."""
        from ctypes import wintypes
        gdi = ctypes.windll.gdi32
        rgn = gdi.CreateRectRgn(0, 0, 0, 0)
        try:
            if self.u.GetWindowRgn(hwnd, rgn) in (0, 1):  # ERROR (no region) or NULLREGION
                return None
            r = wintypes.RECT()
            gdi.GetRgnBox(rgn, ctypes.byref(r))
        finally:
            gdi.DeleteObject(rgn)
        left, top = self.rect(hwnd)[:2]
        return [r.left + left, r.top + top, r.right + left, r.bottom + top]

    def at(self, x, y):
        """The top-level window a click at (x, y) would reach: WindowFromPoint, which hit-tests as
        the mouse does (it skips WS_EX_TRANSPARENT layered windows and honours window regions)."""
        from ctypes import wintypes
        f = self.u.WindowFromPoint
        f.restype, f.argtypes = ctypes.c_void_p, [wintypes.POINT]
        hwnd = f(wintypes.POINT(int(x), int(y)))
        if not hwnd:
            return None
        g = self.u.GetAncestor
        g.restype, g.argtypes = ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_uint]
        return g(hwnd, self.GA_ROOT)

    def foreground(self):
        f = self.u.GetForegroundWindow
        f.restype = ctypes.c_void_p
        return f()

    def others(self, overlay):
        """The pid's other shown winit windows (Settings, the editor): not the overlay."""
        return [h for h in self.find('Window Class') if h != overlay and self.visible(h)]

    def overlay(self, rect):
        """The overlay: the winit window whose rect is the one overlay.window reports."""
        return next((h for h in self.find('Window Class') if self.rect(h) == rect), None)

    def minimize(self, hwnd):
        with injecting('overlay minimize (lab hook)'):
            return self.u.ShowWindow(hwnd, self.SW_MINIMIZE)

    def display_change(self):
        """WM_DISPLAYCHANGE to the app's display watcher, as Windows sends it on a layout change."""
        hwnds = self.find('FoxgloveOverlayDisplays')
        with injecting('WM_DISPLAYCHANGE (lab hook)'):
            for h in hwnds:
                self.u.PostMessageW(h, self.WM_DISPLAYCHANGE, 32, 0)
        return bool(hwnds)

    def tray_click(self):
        """The tray icon's left-button-up, as the shell posts it to the icon's window (tray-icon
        0.24's callback message); the app reads the icon's real rect and the cursor itself."""
        hwnds = self.find('tray_icon_app')
        with injecting('tray click (lab hook)'):
            for h in hwnds:
                self.u.PostMessageW(h, self.WM_USER_TRAYICON, 0, self.WM_LBUTTONUP)
        return bool(hwnds)


WS_EX_TRANSPARENT, WS_EX_LAYERED = 0x20, 0x80000


def click_through(exstyle, visible=True):
    """A window clicks pass through (#549): hidden, or transparent and layered (winit's
    set_cursor_hittest(false))."""
    both = WS_EX_TRANSPARENT | WS_EX_LAYERED
    return not visible or exstyle & both == both


def same_box(a, b, tolerance=2):
    """Two [left, top, right, bottom] boxes equal within `tolerance` pixels (the panel's region is
    its rect rounded outward, tray_panel::region)."""
    return a is not None and b is not None and all(abs(x - y) <= tolerance for x, y in zip(a, b))


def centre(box):
    return ((box[0] + box[2]) // 2, (box[1] + box[3]) // 2)


def outside(box, rect, margin=40):
    """A point inside `rect` (the overlay) but at least `margin` px outside `box` (the panel), or
    None: the corner of the overlay farthest from the panel."""
    corners = [(rect[0] + margin, rect[1] + margin), (rect[2] - margin, rect[1] + margin),
               (rect[0] + margin, rect[3] - margin), (rect[2] - margin, rect[3] - margin)]

    def gap(p):
        dx = max(box[0] - p[0], 0, p[0] - box[2])
        dy = max(box[1] - p[1], 0, p[1] - box[3])
        return max(dx, dy)
    best = max(corners, key=gap)
    return best if gap(best) >= margin else None


def kill(name):
    ps(f'Get-Process {name} -ErrorAction SilentlyContinue | Stop-Process -Force')


class Checks:
    def __init__(self):
        self.results = []

    def __call__(self, name, ok, detail, blocked=False):
        status = 'BLOCKED' if blocked else 'PASS' if ok else 'FAIL'
        self.results.append(status)
        print(f'{status} {name}: {detail}', flush=True)
        return ok

    def finish(self):
        """Prints the result; exit code 0 PASS, 1 FAIL, 2 BLOCKED (nothing failed, something blocked)."""
        r = 'FAIL' if 'FAIL' in self.results or not self.results else 'BLOCKED' if 'BLOCKED' in self.results else 'PASS'
        print('RESULT', r, flush=True)
        sys.exit({'PASS': 0, 'FAIL': 1, 'BLOCKED': 2}[r])


class App:
    """Runs the build under test with `env`; stops the user's Foxglove for the duration."""

    def __init__(self, exe, cfg, env=None, plugins=()):
        self.exe, self.cfg, self.plugins = tool(exe, '--app'), cfg, plugins
        self.env = {**os.environ, 'FOXGLOVE_CONFIG_DIR': str(cfg), **(env or {})}

    def __enter__(self):
        require_app_ok()  # every app-level case launches Foxglove: On-call's OK only
        # Exactly the app's process name: test binaries such as foxglove_plugin_host-*.exe are other
        # sessions' work and must never be stopped.
        self.previous = ps("Get-Process -Name foxglove -ErrorAction SilentlyContinue | "
                           "% { $_.Path; Stop-Process -Id $_.Id -Confirm:$false }").split()
        time.sleep(2)
        for p in self.plugins:
            kill(p)
        self.proc = subprocess.Popen([str(self.exe)], env=self.env)
        return self

    def exited(self):
        """The test app's exit code once it has ended, else None."""
        return self.proc.poll()

    def exit_report(self):
        """Why the test app is gone, from its exit code and its log (in the temp config). Waits
        up to 10 s for a closing app to end (its pipe can break a moment before it exits)."""
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            pass
        return exit_report(self.proc.poll(), log_lines(self.cfg, 'Foxglove'))

    def __exit__(self, *exc):
        if self.proc.poll() is not None:
            print('the test app had already ended:', self.exit_report(), flush=True)
        self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        time.sleep(1)
        for p in self.plugins:  # a hard-stopped app can leave its plugin until the plugin notices
            kill(p)
        for p in self.previous:
            subprocess.Popen([p])
        time.sleep(5)
        alive = [p for p in self.previous if p.lower() in ps("(Get-Process -Name foxglove -ErrorAction SilentlyContinue).Path").lower()]
        print('restored:', alive if alive == self.previous else f'{alive} of {self.previous} (check!)', flush=True)


# Windows' NTSTATUS codes a crashed process exits with, as unsigned 32-bit.
CRASH_CODES = {0xC0000005: 'an access violation', 0xC0000409: 'a stack buffer overrun (abort)',
               0xC00000FD: 'a stack overflow', 0xC0000374: 'heap corruption'}


def exit_report(code, lines):
    """One line on how the test app ended: its exit code read as a crash, a Rust panic or a quit,
    and the last lines of its log. A panic's own log line ("Foxglove: panic in …") wins."""
    if code is None:
        return 'still running'
    tail = [l for l in lines if 'Foxglove' in l][-3:]
    panic = [l for l in lines if 'Foxglove: panic in ' in l]
    quits = [l for l in lines if 'the overlay window closed; quitting' in l or 'Quit from the tray menu' in l]
    c = code & 0xFFFFFFFF
    if panic:
        why = f'a panic: {panic[-1]}'
    elif c in CRASH_CODES:
        why = f'a crash ({CRASH_CODES[c]}, 0x{c:08X})'
    elif c == 101:
        why = 'a Rust panic (exit code 101)'
    elif c == 0 and quits:
        why = f'it quit on its own (exit code 0), not a crash: {quits[-1]}'
    elif c == 0:
        why = ('it quit on its own (exit code 0), not a crash; builds before #618\'s fix log no line '
               'for a window-loop exit')
    elif c == 1:
        why = 'it was stopped, or failed to start (exit code 1)'
    else:
        why = f'exit code {code} (killed or crashed)'
    return f'{why}; its log ends: {tail}'


def rgb(lab, env):
    """Both halves' underglow as RGB_STATE reports it: [{on, hue, sat, brt, effect}] * 2."""
    out = subprocess.run([str(tool(lab)), '--allow-app', 'status'], capture_output=True, text=True, encoding='utf-8',
                         errors='replace', env=env, cwd=REPO).stdout
    line = next((l for l in out.splitlines() if l.startswith('RGB_STATE')), '')
    return [dict(on=h[0] == 'true', hue=int(h[1]), sat=int(h[2]), brt=int(h[3]), effect=int(h[4]))
            for h in re.findall(r'on: (\w+), hue: (\d+), sat: (\d+), brightness: (\d+), effect: (\d+)', line)]


def same(a, b):
    key = lambda s: [(h['on'], h['hue'], h['sat'], h['effect']) for h in s]
    return len(a) == len(b) == 2 and key(a) == key(b)


def hue_is(hue, tolerance=5):
    return lambda s: len(s) == 2 and all(h['on'] and min(abs(h['hue'] - hue), 360 - abs(h['hue'] - hue)) <= tolerance for h in s)


def wait(read, pred, seconds, poll=0.5):
    """Polls `read()` until `pred` holds or `seconds` pass; returns (ok, last value)."""
    end = time.time() + seconds
    while True:
        v = read()
        if pred(v):
            return True, v
        if time.time() > end:
            return False, v
        time.sleep(poll)


class AppGone(RuntimeError):
    """The app's end of the API pipe is gone: the app ended (see App.exit_report)."""


class Api:
    """A client of Foxglove's local API over its named pipe (docs/local-api.md), as an outside tool."""

    def __init__(self, name, permissions, api='1.2'):
        self.pipe = open('//./pipe/foxglove-' + re.sub(r'[^A-Za-z0-9_-]', '', os.environ['USERNAME']), 'r+b', buffering=0)
        self.next = 0
        self.granted = self.call('initialize', {'api': api, 'client': {'name': name, 'version': '1'},
                                                'permissions': permissions})['granted']

    def call(self, method, params):
        """The call's result; raises RuntimeError with the API error, and AppGone when the pipe
        broke (the app ended: on Windows a write to a closed pipe is OSError 22, EINVAL)."""
        self.next += 1
        try:
            self.pipe.write((json.dumps({'jsonrpc': '2.0', 'id': self.next, 'method': method, 'params': params}) + '\n').encode())
        except OSError as e:
            raise AppGone(f'{method}: the API pipe is closed ({e})') from e
        buf = b''
        while True:
            try:
                c = self.pipe.read(1)
            except OSError as e:
                raise AppGone(f'{method}: the API pipe is closed ({e})') from e
            if not c:
                raise AppGone(f'{method}: Foxglove closed the connection')
            buf += c
            if c == b'\n':
                msg, buf = json.loads(buf), b''
                if msg.get('id') == self.next:
                    if 'error' in msg:
                        raise RuntimeError(f"{method}: {msg['error']}")
                    return msg['result']

    def close(self):
        self.pipe.close()


def connect(name, permissions, seconds=30):
    """Waits for the app's pipe (it comes up a few seconds after start)."""
    end = time.time() + seconds
    while True:
        try:
            return Api(name, permissions)
        except OSError:
            if time.time() > end:
                raise
            time.sleep(0.5)


def restore_lights(exe, lab, original):
    """Puts the underglow back to `original` (an rgb() reading) through a short run of the app with no
    plugins: a hard-stopped app can't give back a plugin's lease, so the lights would stay leased."""
    if len(original) != 2:
        return print('restore_lights: no reading to restore', flush=True)
    cfg = temp_config('foxglove-restore-')
    with App(exe, cfg) as app:
        api = connect('test-lab-restore', ['devices.read', 'lighting.write'])
        h = original[0]
        for d in api.call('devices.list', {})['devices']:
            if 'lighting' in d['caps']:
                api.call('lighting.set', {'device': d['id'], 'on': h['on'], 'effect': str(h['effect']),
                                          'color': {'hue': h['hue'], 'saturation': h['sat'], 'brightness': h['brt']}})
        ok, s = wait(lambda: rgb(lab, app.env), lambda s: same(s, original), 10)
        api.close()
    print('lights restored' if ok else f'lights NOT restored: {s} (wanted {original})', flush=True)


if __name__ == '__main__':
    # The "user is working" gate On-call runs before a soak or a typing step, with the harness's
    # own input left out:  python scripts/lab/labkit.py idle [--min 120]   (exit 0, or 2 BLOCKED)
    import argparse
    ap = argparse.ArgumentParser(description='labkit tools')
    ap.add_argument('what', choices=['idle'])
    ap.add_argument('--min', type=float, default=120, help='seconds of user idle needed')
    require_user_idle(ap.parse_args().min)
