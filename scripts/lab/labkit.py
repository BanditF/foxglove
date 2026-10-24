"""Shared pieces for the app-level board checks in scripts/lab (stdlib only, Windows).

Every check runs a Foxglove build on a temp FOXGLOVE_CONFIG_DIR; the user's real settings are never
touched. Whatever Foxglove the user had running is stopped first and started again after, by path.
"""
import ctypes, json, os, re, shutil, subprocess, sys, tempfile, time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', REPO / 'target'))
REAL_CONFIG = Path(os.environ['APPDATA']) / 'Foxglove' / 'config'


def ps(script):
    return subprocess.run(['powershell', '-NoProfile', '-Command', script], capture_output=True, text=True).stdout


def locked():
    """A locked PC sends keys to the secure desktop: no global hotkey fires."""
    return not ctypes.windll.user32.OpenInputDesktop(0, False, 0x0100)


def temp_config(prefix, store=''):
    """A temp config folder with the user's keymap metadata (so behaviors resolve) and `store` as
    its foxglove.toml."""
    cfg = Path(tempfile.mkdtemp(prefix=prefix)) / 'config'
    cfg.mkdir()
    for k in REAL_CONFIG.glob('keymap-*.studio'):
        shutil.copy(k, cfg)
    (cfg / 'foxglove.toml').write_text('schema = 1\n' + store, encoding='utf-8')
    return cfg


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
        self.exe, self.cfg, self.plugins = exe, cfg, plugins
        self.env = {**os.environ, 'FOXGLOVE_CONFIG_DIR': str(cfg), **(env or {})}

    def __enter__(self):
        # Exactly the app's process name: test binaries such as foxglove_plugin_host-*.exe are other
        # sessions' work and must never be stopped.
        self.previous = ps("Get-Process -Name foxglove -ErrorAction SilentlyContinue | "
                           "% { $_.Path; Stop-Process -Id $_.Id -Confirm:$false }").split()
        time.sleep(2)
        for p in self.plugins:
            kill(p)
        self.proc = subprocess.Popen([str(self.exe)], env=self.env)
        return self

    def __exit__(self, *exc):
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


def rgb(lab, env):
    """Both halves' underglow as RGB_STATE reports it: [{on, hue, sat, brt, effect}] * 2."""
    out = subprocess.run([str(lab), '--allow-app', 'status'], capture_output=True, text=True, env=env, cwd=REPO).stdout
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


class Api:
    """A client of Foxglove's local API over its named pipe (docs/local-api.md), as an outside tool."""

    def __init__(self, name, permissions, api='1.2'):
        self.pipe = open('//./pipe/foxglove-' + re.sub(r'[^A-Za-z0-9_-]', '', os.environ['USERNAME']), 'r+b', buffering=0)
        self.next = 0
        self.granted = self.call('initialize', {'api': api, 'client': {'name': name, 'version': '1'},
                                                'permissions': permissions})['granted']

    def call(self, method, params):
        """The call's result; raises RuntimeError with the API error."""
        self.next += 1
        self.pipe.write((json.dumps({'jsonrpc': '2.0', 'id': self.next, 'method': method, 'params': params}) + '\n').encode())
        buf = b''
        while True:
            c = self.pipe.read(1)
            if not c:
                raise RuntimeError('Foxglove closed the connection')
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
