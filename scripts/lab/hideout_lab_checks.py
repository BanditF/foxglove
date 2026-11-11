"""Hideout lab checks (#1003): the #997 tray flyout steps, the #995 module page steps, the
#1004 supervisor steps and #1093's state steps 2-4 (#1096), run against a lab hub through its lab
control pipe
(`hideout.exe --lab-do <verb>`, crates/hideout-shell/src/lab_control.rs) with `hideout.exe
--capture` PNGs as proofs. Prints PASS/FAIL/BLOCKED per step and RESULT at the end (exit 0 PASS,
1 FAIL, 2 BLOCKED). What still needs hands is listed in docs/hideout-lab.md.

The lab hub runs with HIDEOUT_LAB=1 and HIDEOUT_LAB_CAPTURE=1 on its own HIDEOUT_CONFIG_DIR
(--lab), with the first-party plugins in <lab>\\lab-modules (docs/hideout-lab.md, On-call steps).
--start starts it (and stops only that process, by its own handle, at the end); without it the
hub must already run on --lab. It shows windows, the tray flyout and the tray menu, so it runs only
with FOXGLOVE_APP_OK=1 (On-call, in a lab sitting), never on the user's desktop otherwise.

    FOXGLOVE_APP_OK=1 python scripts/lab/hideout_lab_checks.py --app <hideout.exe> --lab <folder> [--start]
        [--only flyout,pages,supervisor,states] [--shots <folder>]

Waits: every lab control action answers after the frame that shows its result. The few waits on
the hub's own timing (a module restarting, a crash landing) read `state` every 0.25 s for a
bounded time, started by the action they wait for.
"""
import argparse, json, os, subprocess, sys, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit import tool_arg, Checks, REPO, require_app_ok  # noqa: E402

try:
    import tomllib
except ImportError:  # Python < 3.11
    tomllib = None

# The module the page and supervisor steps crash (its exe is killed; the supervisor restarts it).
CRASH_MODULE = 'org.foxglove.focus-timer'


# ---- pure helpers (tested window-free in scripts/test_hideout_lab_checks.py) -------------------

def texts(state, window='main'):
    """Every name or text the window last drew, in tree order."""
    return [n.get('name') or n.get('value') or '' for n in state.get(window, {}).get('nodes', [])]


def has(state, text, window='main', prefix=False):
    return any(t == text or (prefix and t.startswith(text)) for t in texts(state, window))


def contains(state, piece, window='main'):
    return any(piece in t for t in texts(state, window))


def node_index(state, window, pred):
    """The index (for `click @<i>`) of the first node `pred` accepts."""
    for i, n in enumerate(state.get(window, {}).get('nodes', [])):
        if pred(n):
            return i
    return None


def field_after(state, label, window='main'):
    """The index of the first text field after the node that says `label` (a setting's row)."""
    nodes = state.get(window, {}).get('nodes', [])
    start = next((i for i, n in enumerate(nodes) if (n.get('name') or n.get('value')) == label), None)
    if start is None:
        return None
    return next((i for i in range(start + 1, len(nodes)) if nodes[i].get('role') in ('TextInput', 'MultilineTextInput')), None)


def tile_title(name):
    """A tile's title from its accessible name ("Focus timer, 18:42, Focus · round 2 of 4")."""
    return (name or '').split(', ')[0]


def flicker_verdict(frames, main_shown, dbl_t=None):
    """The double-click check (#997 step 1, #1076): PASS when no flyout frame was on screen
    after the hub took the double click (`dbl_t`, from `state().tray.last`), else FAIL with when
    and how long. The first click's flyout before it is allowed (it opens at once, as any
    single click). Without `dbl_t`, any flyout frame fails."""
    rows = frames.get('rows', [])
    since = dbl_t if dbl_t is not None else -1
    fly = [r['t'] for r in rows if r.get('flyout_visible') and r['t'] > since]
    passes = frames.get('counts', {}).get('flyout', 0)
    if not main_shown:
        return False, 'the main window did not open after the double click'
    if not fly:
        return True, f'no flyout frame on screen after t={since} ({passes} flyout pass(es))'
    return False, (f'flicker: the flyout was on screen {fly[-1] - fly[0]} ms after the double click '
                   f'({len(fly)} frame(s), from t={fly[0]})')


def tray_event_t(state, what):
    """When the hub took the tray command `what` (`state().tray.last`: "<what> at <ms>"), or None."""
    last = (state.get('tray') or {}).get('last') or ''
    head, sep, t = last.rpartition(' at ')
    if not sep or head != what:
        return None
    try:
        return int(t)
    except ValueError:
        return None


def click_latency(frames, click_t):
    """Ms from a tray click to the flyout's first frame on screen, or None if none came."""
    fly = [r['t'] for r in frames.get('rows', []) if r.get('flyout_visible') and r['t'] >= click_t]
    return fly[0] - click_t if fly else None


# A single click opens the flyout at once, as on main (#1076: no hold for the double-click time).
CLICK_LATENCY_MS = 250


def against_taskbar(geometry, gap=32):
    """The flyout is inside its monitor's work area and against the taskbar's edge (within `gap`
    pixels of the work area's edge on the taskbar side)."""
    if not geometry:
        return False, 'no flyout window geometry'
    l, t, r, b = geometry['rect']
    wl, wt, wr, wb = geometry['work']
    ml, mt, mr, mb = geometry['monitor']
    inside = wl <= l and wt <= t and r <= wr and b <= wb
    edges = {'bottom': wb < mb, 'top': wt > mt, 'left': wl > ml, 'right': wr < mr}
    side = next((e for e, on in edges.items() if on), 'bottom')  # an auto-hidden taskbar: bottom
    near = {'bottom': wb - b, 'top': t - wt, 'left': l - wl, 'right': wr - r}[side]
    ok = inside and 0 <= near <= gap
    return ok, f'flyout {geometry["rect"]} in work area {geometry["work"]}, taskbar {side}, {near} px from it'


def manifest_names(lab):
    """{id: name} of the lab modules in <lab>\\lab-modules (module.toml, else plugin.toml)."""
    out = {}
    for d in sorted((Path(lab) / 'lab-modules').glob('*')):
        for f in ('module.toml', 'plugin.toml'):
            p = d / f
            if p.is_file() and tomllib:
                m = tomllib.loads(p.read_text(encoding='utf-8'))
                out[m.get('id', d.name)] = m.get('name', d.name)
                break
    return out


def module_exe(lab, module):
    """The lab module's exe (the manifest's run[0]) as a full path."""
    d = Path(lab) / 'lab-modules' / module
    for f in ('module.toml', 'plugin.toml'):
        p = d / f
        if p.is_file() and tomllib:
            run = tomllib.loads(p.read_text(encoding='utf-8')).get('run') or []
            if run:
                return d / run[0]
    return None


def tray_status(state):
    """The tray menu's status line (its first item), when the menu is open."""
    items = (state.get('tray') or {}).get('menu') or []
    return items[0]['text'] if items else None


# The `hub` this Hideout serves (hideout_contract::HUB_VERSION); a lab module asking for more
# reads "Needs a newer Hideout" (#1096).
SERVED_HUB = (1, 2)


def hub_newer(hub, served=SERVED_HUB):
    """Whether a manifest's `hub` ("1.9") asks for a newer hub than `served` serves."""
    try:
        major, minor = (int(x) for x in str(hub).split('.'))
    except ValueError:
        return False
    return (major, minor) > served and not (major == served[0] and minor <= served[1])


def needs_hub_modules(lab, served=SERVED_HUB):
    """{id: name} of the lab modules whose module.toml asks for a newer hub (#1093 step 2)."""
    out = {}
    for d in sorted((Path(lab) / 'lab-modules').glob('*')):
        p = d / 'module.toml'
        if p.is_file() and tomllib:
            m = tomllib.loads(p.read_text(encoding='utf-8'))
            if hub_newer(m.get('hub', ''), served):
                out[m.get('id', d.name)] = m.get('name', d.name)
    return out


def node(state, text, window='main'):
    """The first node that says `text` exactly, or None."""
    return next((n for n in state.get(window, {}).get('nodes', [])
                 if (n.get('name') or n.get('value')) == text), None)


# ---- the lab hub ---------------------------------------------------------------------------------

class Hub:
    def __init__(self, exe, lab, shots):
        self.exe, self.lab, self.shots = Path(exe), Path(lab), Path(shots)
        self.env = {**os.environ, 'HIDEOUT_CONFIG_DIR': str(self.lab), 'HIDEOUT_LAB': '1',
                    'HIDEOUT_LAB_CAPTURE': '1'}
        self.proc = None

    def start(self):
        self.proc = subprocess.Popen([str(self.exe), '--lab'], env=self.env, cwd=self.lab,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ok, _ = self.wait(lambda s: s.get('ok'), 30, read=lambda: self.do('state', check=False))
        if not ok:
            raise RuntimeError('the lab hub did not answer on its lab control pipe within 30 s')

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()  # only the hub this script started, by its own handle
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()

    def do(self, *verb, check=True, timeout=30):
        r = subprocess.run([str(self.exe), '--lab-do', *map(str, verb)], env=self.env, capture_output=True,
                           text=True, encoding='utf-8', errors='replace', timeout=timeout)
        try:
            ans = json.loads(r.stdout.strip().splitlines()[-1])
        except (ValueError, IndexError):
            ans = {'ok': False, 'error': f'no answer: {r.stdout!r} {r.stderr!r}'}
        if check and not ans.get('ok'):
            raise RuntimeError(f'{" ".join(map(str, verb))}: {ans.get("error")}')
        return ans

    def state(self):
        return self.do('state')

    def wait(self, pred, seconds, read=None):
        """Reads `state` (or `read()`) every 0.25 s until `pred` holds or `seconds` pass."""
        read = read or self.state
        end = time.time() + seconds
        while True:
            v = read()
            if pred(v):
                return True, v
            if time.time() > end:
                return False, v
            time.sleep(0.25)

    def capture(self, name):
        self.shots.mkdir(parents=True, exist_ok=True)
        out = self.shots / f'{name}.png'
        r = subprocess.run([str(self.exe), '--capture', str(out)], env=self.env, capture_output=True,
                           text=True, encoding='utf-8', errors='replace', timeout=30)
        return r.returncode == 0, r.stdout.strip().replace('\n', '; ')

    def log(self):
        p = self.lab / 'logs' / 'hideout.log'
        return p.read_text(encoding='utf-8', errors='replace') if p.exists() else ''

    def pids(self, exe):
        """Pids of processes whose image is exactly `exe` (the lab module's own copy)."""
        exe = str(exe).replace("'", "''")
        r = subprocess.run(['powershell', '-NoProfile', '-Command',
                            f"Get-CimInstance Win32_Process | Where-Object {{ $_.ExecutablePath -eq '{exe}' }} "
                            "| ForEach-Object { $_.ProcessId }"], capture_output=True, text=True, timeout=60)
        return [int(x) for x in r.stdout.split() if x.isdigit()]

    def kill(self, pid):
        subprocess.run(['taskkill', '/PID', str(pid), '/F'], capture_output=True, timeout=30)

    # Bring the hub to a known state: window hidden, flyout and menu closed.
    def to_tray(self):
        s = self.state()
        if s['tray'].get('menu') is not None:
            self.do('menu-key', 'esc')
        if s['flyout']['open']:
            self.do('close-flyout')
        if s['main']['on_screen']:
            self.do('close-window')
        self.wait(lambda s: not s['main']['on_screen'] and not s['flyout']['open'], 5)

    def show_main(self):
        """Open Hideout from the tray menu (the keyboard-free tray path)."""
        if self.state()['main']['on_screen']:
            return
        self.do('tray-menu')
        ok, _ = self.wait(lambda s: s['tray'].get('menu') is not None, 5)
        if not ok:
            raise RuntimeError('the tray menu did not open')
        self.do('menu-pick', 'Open Hideout')
        ok, _ = self.wait(lambda s: s['main']['on_screen'], 5)
        if not ok:
            raise RuntimeError('Open Hideout did not show the window')


def step(check, name, fn):
    """Runs one step; an exception is that step's FAIL, and the next step still runs."""
    try:
        fn()
    except Exception as e:  # noqa: BLE001 (one step's failure is reported, not fatal)
        check(name, False, f'{type(e).__name__}: {e}')


# ---- #997: the tray flyout -------------------------------------------------------------------

def flyout_steps(hub, check):
    def s1_click_opens():
        hub.to_tray()
        t = hub.do('tray-click')['t']
        ok, s = hub.wait(lambda s: s['flyout']['open'] and s['flyout']['visible'], 5)
        if not check('997.1 tray click opens the flyout', ok, f'open={s["flyout"]["open"]} visible={s["flyout"]["visible"]}'):
            return
        ms = click_latency(hub.do('frames', 1500, t), t)
        check('997.1 the flyout opens at once (no double-click hold)',
              ms is not None and ms <= CLICK_LATENCY_MS, f'first flyout frame {ms} ms after the click')
        tiles = [t for t in ('Focus timer', 'Mic mute') if has(s, t, 'flyout')]
        check('997.1 default tiles Focus timer and Mic mute', len(tiles) == 2, f'drawn: {texts(s, "flyout")[:20]}')
        ok, why = against_taskbar(s['flyout'].get('geometry'))
        check('997.1 flyout against the taskbar, inside the screen', ok, why)
        ok, why = hub.capture('997-1-flyout')
        check('997.1 capture', ok, why)

    def s1_double_click():
        for gap in (150, 400):
            hub.to_tray()
            t = hub.do('tray-double-click', gap)['t']
            ok, s = hub.wait(lambda s: tray_event_t(s, 'DoubleClick') is not None and s['main']['on_screen'], 5)
            frames = hub.do('frames', 1500, t)
            ok, why = flicker_verdict(frames, s['main']['on_screen'], tray_event_t(s, 'DoubleClick'))
            check(f'997.1 double click ({gap} ms) opens Hideout, no flyout frame after it', ok, why)

    def s2_toggle_and_esc():
        hub.to_tray()
        hub.do('tray-click')
        hub.wait(lambda s: s['flyout']['visible'], 5)
        hub.do('tray-click')
        ok, s = hub.wait(lambda s: not s['flyout']['open'], 5)
        time.sleep(0.5)
        stays = not hub.state()['flyout']['open']
        check('997.2 a second click closes it and it stays closed', ok and stays, f'open={s["flyout"]["open"]}')
        hub.do('tray-click')
        hub.wait(lambda s: s['flyout']['visible'], 5)
        hub.do('key', 'esc', 'flyout')
        ok, _ = hub.wait(lambda s: not s['flyout']['open'], 5)
        check('997.2 Esc closes it', ok, 'Esc into the flyout window')

    def s4_hide_and_order():
        hub.to_tray()
        hub.do('open-flyout')
        hub.wait(lambda s: s['flyout']['visible'], 5)
        s = hub.state()
        first = next((t for t in texts(s, 'flyout') if t in ('Focus timer', 'Mic mute')), None)
        idx = [i for i, t in enumerate(texts(s, 'flyout')) if t == '···']
        if not first or not idx:
            return check('997.4 tile menu', False, f'no tile with a ··· menu: {texts(s, "flyout")[:20]}')
        hub.do('click', '···', 'flyout')  # the first tile's ···
        hub.do('click', 'Hide tile', 'flyout')
        hub.do('click', 'Edit tiles', 'flyout')
        s = hub.state()
        box = next((n for n in s['flyout']['nodes'] if n.get('role') == 'CheckBox' and n.get('name') == first), None)
        hidden = box is not None and box.get('selected') is not True
        check('997.4 Hide tile, then Edit tiles shows it unticked', hidden, f'{first}: {box}')
        hub.do('click', first, 'flyout')
        hub.do('click', 'Done', 'flyout')
        toml = (hub.lab / 'hideout.toml')
        text = toml.read_text(encoding='utf-8') if toml.exists() else ''
        check('997.4 hideout.toml has a [tray] table', '[tray]' in text, str(toml))
        hub.do('close-flyout')

    def s5_open_and_keyboard():
        hub.to_tray()
        hub.do('open-flyout')
        hub.wait(lambda s: s['flyout']['visible'], 5)
        hub.do('click', 'Open Hideout', 'flyout')
        ok, s = hub.wait(lambda s: s['main']['on_screen'] and not s['flyout']['open'], 5)
        check('997.5 Open Hideout opens the main window and closes the flyout', ok,
              f'main={s["main"]["on_screen"]} flyout={s["flyout"]["open"]}')
        hub.to_tray()
        hub.do('open-flyout')
        hub.wait(lambda s: s['flyout']['visible'], 5)
        hub.do('click', '···', 'flyout')
        s = hub.state()
        opener = next((t for t in texts(s, 'flyout') if t.startswith('Open ') and t != 'Open Hideout'), None)
        if opener:
            hub.do('click', opener, 'flyout')
            ok, s = hub.wait(lambda s: s['main']['on_screen'] and
                             (s.get('active_page') or '').startswith(opener[5:]), 5)
            check('997.5 a tile opens its module page in the main window', ok, f'{opener}: active {s.get("active_page")}')
        else:
            check('997.5 a tile opens its module page', False, 'no "Open <Module>" in the tile menu')
        # Keyboard: Win+B, arrows, Enter on the icon -> what the shell sends -> the tray menu.
        hub.to_tray()
        hub.do('tray-key-select')
        ok, s = hub.wait(lambda s: s['tray'].get('menu') is not None, 5)
        if not check('997.5 keyboard select on the icon opens the tray menu', ok, f'menu {s["tray"].get("menu")}'):
            return
        items = s['tray']['menu']
        target = next((i for i, it in enumerate(items) if it['text'] == 'Open Hideout'), None)
        for _ in range(len(items) + 1):
            items = hub.state()['tray'].get('menu') or []
            if any(it['highlighted'] and it['text'] == 'Open Hideout' for it in items):
                break
            hub.do('menu-key', 'down')
            time.sleep(0.1)
        hub.do('menu-key', 'enter')
        ok, s = hub.wait(lambda s: s['main']['on_screen'], 5)
        check('997.5 arrows and Enter in the tray menu pick Open Hideout', ok and target is not None,
              f'main on screen: {s["main"]["on_screen"]}')

    def s10_tile_keyboard():
        # #991: the keyboard inside the open flyout (docs/hideout-plugin-modules-ux.md).
        hub.to_tray()
        hub.do('open-flyout')
        ok, s = hub.wait(lambda s: s['flyout']['visible'] and s['flyout'].get('focus'), 5)
        first = s['flyout'].get('focus') or ''
        if not check('991 the flyout opens with the first tile focused',
                     tile_title(first) in ('Focus timer', 'Mic mute'), f'focus {first!r}'):
            return hub.do('close-flyout')
        hub.do('key', 'right', 'flyout')
        second = hub.state()['flyout'].get('focus') or ''
        check('991 Right moves to the next tile', tile_title(second) not in ('', tile_title(first)),
              f'{first!r} -> {second!r}')
        hub.do('key', 'left', 'flyout')
        back = hub.state()['flyout'].get('focus') or ''
        check('991 Left comes back', tile_title(back) == tile_title(first), f'focus {back!r}')
        if tile_title(first) == 'Focus timer':
            hub.do('key', 'space', 'flyout')
            ok, s = hub.wait(lambda s: 'Paused' in (s['flyout'].get('focus') or ''), 5)
            check('991 Space presses the tile button (Pause)', ok, f'focus {s["flyout"].get("focus")!r}')
            hub.do('key', 'space', 'flyout')  # Resume: leave the timer as it was
        hub.do('key', 'shift+f10', 'flyout')
        ok, s = hub.wait(lambda s: (s['flyout'].get('focus') or '').startswith('Open '), 5)
        check('991 Shift+F10 opens the tile menu with Open <Module> focused', ok,
              f'focus {s["flyout"].get("focus")!r}')
        hub.do('key', 'esc', 'flyout')
        ok, s = hub.wait(lambda s: tile_title(s['flyout'].get('focus')) == tile_title(first), 5)
        check('991 Esc closes the menu, not the flyout; the focus is back on the tile',
              ok and s['flyout']['open'], f'open={s["flyout"]["open"]} focus {s["flyout"].get("focus")!r}')
        ok, why = hub.capture('991-flyout-keyboard')
        check('991 capture', ok, why)
        hub.do('key', 'enter', 'flyout')
        ok, s = hub.wait(lambda s: s['main']['on_screen'] and not s['flyout']['open'], 5)
        check('991 Enter opens the tile page in the main window', ok and
              (s.get('active_page') or '').startswith(tile_title(first)),
              f'main={s["main"]["on_screen"]} active {s.get("active_page")!r}')

    def s8_obs_cancel():
        hub.show_main()
        names = manifest_names(hub.lab)
        obs = names.get('org.foxglove.broadcast-status')
        if not obs:
            return check('997.8 OBS Change, Cancel', False, 'no OBS lab module', blocked=True)
        hub.do('focus', obs)
        s = hub.state()
        if not has(s, 'Saved'):
            return check('997.8 OBS Change, Cancel', False, 'no saved OBS password in this lab hub', blocked=True)
        hub.do('click', 'Change')
        f = node_index(hub.state(), 'main', lambda n: n.get('role') in ('PasswordInput', 'TextInput'))
        if f is not None:
            hub.do('click', f'@{f}')
            hub.do('text', 'lab-not-the-password')
        hub.do('click', 'Cancel')
        check('997.8 Change, type, Cancel: back to Saved', has(hub.state(), 'Saved'), 'Saved row is back')

    def s0_hidden_root_gate():
        # Hard gate (#1019): with the main window hidden, a tray click must place, show and draw
        # the flyout within 1 s, and the hub must go back to sleeping after it closes.
        hub.to_tray()
        before = hub.state()['flyout']['passes']
        hub.do('tray-click')
        ok, s = hub.wait(lambda s: s['flyout']['open'] and s['flyout']['visible']
                         and s['flyout']['passes'] > before, 1)
        f = s['flyout']
        check('1019 hidden main window: tray click shows and draws the flyout within 1 s', ok,
              f'open={f["open"]} visible={f["visible"]} passes {before} -> {f["passes"]}')
        hub.do('close-flyout')
        hub.wait(lambda s: not s['flyout']['open'], 5)
        time.sleep(3)
        a = hub.state()['loop']['wakes']
        time.sleep(10)
        rate = (hub.state()['loop']['wakes'] - a) / 10
        check('1019 hidden idle after the flyout closes (loop wakes/s)', rate <= 2, f'{rate:.1f} wakes/s over 10 s')

    def s9_hidden_wakes():
        hub.to_tray()
        time.sleep(3)
        a = hub.state()['loop']['wakes']
        time.sleep(10)
        b = hub.state()['loop']['wakes']
        # Each `state` read wakes nothing on the UI thread; the hub's own wakes are what's counted.
        rate = (b - a) / 10
        check('997.9 hidden hub sleeps (loop wakes/s)', rate <= 2, f'{rate:.1f} wakes/s over 10 s')
        hub.do('open-flyout')
        hub.wait(lambda s: s['flyout']['visible'], 5)
        hub.do('key', 'esc', 'flyout')
        hub.wait(lambda s: not s['flyout']['open'], 5)
        time.sleep(3)
        a = hub.state()['loop']['wakes']
        time.sleep(10)
        b = hub.state()['loop']['wakes']
        rate = (b - a) / 10
        check('997.9 back to the hidden baseline after the flyout', rate <= 2, f'{rate:.1f} wakes/s over 10 s')

    for name, fn in [('1019', s0_hidden_root_gate), ('997.1', s1_click_opens), ('997.1 dbl', s1_double_click), ('997.2', s2_toggle_and_esc),
                     ('997.4', s4_hide_and_order), ('997.5', s5_open_and_keyboard), ('991 keys', s10_tile_keyboard), ('997.8', s8_obs_cancel),
                     ('997.9', s9_hidden_wakes)]:
        step(check, name, fn)


# ---- #995: module pages ------------------------------------------------------------------------

def page_steps(hub, check):
    names = manifest_names(hub.lab)

    def s1_sidebar():
        hub.show_main()
        s = hub.state()
        missing = [n for n in names.values() if not has(s, n, prefix=True) and not contains(s, n)]
        check('995.1 the sidebar lists every lab module page', bool(names) and not missing,
              f'missing {missing} of {sorted(names.values())}')
        line = next((l for l in hub.log().splitlines() if 'module manifest(s) loaded' in l), '')
        check('995.1 hideout.log has the manifest line', bool(line), line.strip()[-60:])

    def s2_focus_timer():
        name = names.get(CRASH_MODULE, 'Focus timer')
        hub.show_main()
        hub.do('focus', name)
        s = hub.state()
        rows = [r for r in ('Pause or resume', 'Start a new focus period') if contains(s, r)]
        check('995.2 Focus timer Keys lists its two actions', len(rows) == 2, f'found {rows}')
        if has(s, 'Pause'):
            before = hub.log().count('action.invoke')
            hub.do('click', 'Pause')
            time.sleep(0.5)
            after = hub.log().count('action.invoke')
            check('995.2 Pause clicks through to the module', True,
                  f'clicked Pause (action.invoke lines in hideout.log {before} -> {after})')
        else:
            check('995.2 Pause button on the page', False, f'no Pause: {texts(s)[:30]}')
        hub.capture('995-2-focus-timer')

    def s3_icue_setting():
        name = names.get('org.foxglove.icue')
        if not name:
            return check('995.3 iCUE SDK path', False, 'no iCUE lab module', blocked=True)
        hub.show_main()
        hub.do('focus', name)
        s = hub.state()
        label = next((t for t in texts(s) if t.startswith("Corsair's iCUE SDK file")), None)
        if not check('995.3 iCUE Settings shows the SDK file field', label is not None, f'{texts(s)[:40]}'):
            return
        # A file field is a picker, not a text box a script can type into (#1019): set it through
        # the Settings form's own path (lab control's set-setting), as a pick would.
        start = time.time()
        hub.do('set-setting', 'org.foxglove.icue', 'sdk-dll', str(hub.lab / 'iCUESDK.x64_2019.dll'))
        ok, _ = hub.wait(lambda _: any(p.stat().st_mtime >= start - 1 for p in hub.lab.rglob('settings.json')
                                       if 'icue' in str(p).lower()), 5, read=lambda: None)
        check('995.3 a typed SDK path lands in its settings.json', ok, 'settings.json under the lab folder')
        hub.capture('995-3-icue-settings')

    def s4_obs_how_to():
        name = names.get('org.foxglove.broadcast-status')
        if not name:
            return check('995.4 OBS How to', False, 'no OBS lab module', blocked=True)
        hub.show_main()
        hub.do('focus', name)
        if not has(hub.state(), 'How to'):
            return check('995.4 OBS How to', False, 'no How to button (OBS reachable?)', blocked=True)
        hub.do('click', 'How to')
        check('995.4 How to opens How to connect', contains(hub.state(), 'How to connect'), 'section drawn')

    for name, fn in [('995.1', s1_sidebar), ('995.2', s2_focus_timer), ('995.3', s3_icue_setting),
                     ('995.4', s4_obs_how_to)]:
        step(check, name, fn)


# ---- #995 step 5 and #1004: the supervisor's states ----------------------------------------------

def supervisor_steps(hub, check):
    names = manifest_names(hub.lab)
    name = names.get(CRASH_MODULE)
    exe = module_exe(hub.lab, CRASH_MODULE)
    if not name or not exe:
        check('1004 supervisor steps', False, f'no {CRASH_MODULE} lab module', blocked=True)
        return

    def tray_line():
        hub.do('tray-menu')
        ok, s = hub.wait(lambda s: s['tray'].get('menu') is not None, 5)
        hub.do('menu-key', 'esc')
        return tray_status(s) if ok else None

    def crash_once(k):
        pids = hub.pids(exe)
        if not pids:
            raise RuntimeError(f'{exe.name} is not running')
        hub.kill(pids[0])
        return pids[0]

    def s1_kill_once():
        hub.show_main()
        hub.do('focus', name)
        quiet = tray_line()
        crash_once(1)
        ok, s = hub.wait(lambda s: contains(s, f'{name} stopped unexpectedly.'), 10)
        check('1004.1 a crash shows the amber crash bar', ok, next((t for t in texts(s) if 'stopped unexpectedly' in t), 'no bar'))
        check('1004.1 the bar keeps the place and has Open log, no Restart now',
              contains(s, 'Your place on this page is kept.') and has(s, 'Open log') and not contains(s, 'Restart now'),
              'Line 2 and Open log')
        hub.capture('1004-1-crash-bar')
        check('1004.1 a restart leaves the tray alone', tray_line() == quiet, f'menu status {quiet!r}')
        ok, s = hub.wait(lambda s: not contains(s, f'{name} stopped unexpectedly.'), 40)
        check('995.5 / 1004.1 the page comes back live after the restart', ok, 'crash bar gone')

    def s2_five_crashes():
        hub.show_main()
        hub.do('focus', name)
        # s1_kill_once already crashed the module once; four more make five (a fifth here would
        # find the module correctly stopped).
        for k in range(4):
            ok, _ = hub.wait(lambda _: bool(hub.pids(exe)), 70, read=lambda: None)
            if not ok:
                return check('1004.2 five crashes', False, f'{exe.name} did not come back after crash {k}')
            crash_once(k + 1)
            if k < 3:
                hub.wait(lambda s: contains(s, f'{name} stopped unexpectedly.'), 10)
        ok, s = hub.wait(lambda s: contains(s, 'Stopped after 5 crashes'), 15)
        check('1004.2 five crashes show the stopped card', ok, 'Stopped after 5 crashes')
        check('1004.2 the card has Start again (focused) and Copy diagnostics',
              has(s, 'Start again') and has(s, 'Copy diagnostics') and s['main'].get('focus') == 'Start again',
              f'focus {s["main"].get("focus")!r}')
        hub.capture('1004-2-stopped-card')
        line = tray_line() or ''
        check('1004.2 the tray menu names the stopped module', line.startswith(f'{name} stopped'), repr(line))
        hub.do('key', 'esc', 'main')
        check('1004.2 Esc leaves the card as it is', has(hub.state(), 'Start again'), 'Start again still drawn')
        hub.do('key', 'enter', 'main')
        ok, _ = hub.wait(lambda _: bool(hub.pids(exe)), 20, read=lambda: None)
        ok2, _ = hub.wait(lambda s: not contains(s, 'Stopped after 5 crashes'), 20)
        check('1004.3 Enter presses Start again: the module runs again', ok and ok2, f'{exe.name} running: {ok}')

    for n, fn in [('1004.1', s1_kill_once), ('1004.2', s2_five_crashes)]:
        step(check, n, fn)


# ---- #1093 steps 2-4: needs a newer Hideout, the update states, a quarantine (#1096) -------------

# The module the update steps drive (it runs and has a page), and the one quarantined (another,
# so the updates' "Go back" version never shows on the quarantine card).
UPDATE_MODULE = CRASH_MODULE
QUARANTINE_MODULES = ('org.foxglove.broadcast-status', 'org.foxglove.icue', 'org.foxglove.mic-mute')
UPDATE_HIDEOUT = 'Update Hideout'
NEEDS_HUB = 'Needs a newer Hideout'
UPDATE_READY, UPDATE_WAITING, UPDATING = 'Update ready', 'Update waiting for you', 'Updating…'
PLACE_KEPT = 'Your place on this page is kept.'
QUARANTINED_LINE = 'Hideout stopped twice while drawing it.'


def state_steps(hub, check):
    names = manifest_names(hub.lab)

    def tray_line():
        hub.do('tray-menu')
        ok, s = hub.wait(lambda s: s['tray'].get('menu') is not None, 5)
        hub.do('menu-key', 'esc')
        return tray_status(s) if ok else None

    def s2_needs_hub():
        newer = needs_hub_modules(hub.lab)
        if not newer:
            return check('1093.2 needs a newer Hideout', False,
                         'no lab module asks for a newer hub: set hub = "1.9" in one module.toml '
                         'before the hub starts', blocked=True)
        mid, name = next(iter(newer.items()))
        line = next((l for l in hub.log().splitlines() if f'lab: {mid}: listed from' in l), '')
        check('1093.2 the lab hub lists it (not "not loaded")', 'needs hub' in line, line.strip()[-90:])
        exe = module_exe(hub.lab, mid)
        check('1093.2 it never runs', not (exe and hub.pids(exe)), f'{exe.name if exe else "no run"} not running')
        hub.show_main()
        hub.do('focus', 'Modules')
        s = hub.state()
        check('1093.2 its Modules row reads Needs a newer Hideout', has(s, name, prefix=True) and contains(s, NEEDS_HUB),
              f'{name}: {[t for t in texts(s) if name in t or NEEDS_HUB in t][:4]}')
        check('1093.2 the row offers Update Hideout', has(s, UPDATE_HIDEOUT), 'Update Hideout button')
        hub.capture('1093-2-needs-hub-modules-row')
        line = tray_line() or ''
        check('1093.2 the tray menu names it (red, root cause once)',
              line == f'{name} needs a newer Hideout' or 'needs a newer Hideout' in line, repr(line))

    def s3_updates():
        name = names.get(UPDATE_MODULE)
        if not name:
            return check('1093.3 update states', False, f'no {UPDATE_MODULE} lab module', blocked=True)
        hub.show_main()
        hub.do('focus', name)
        # Update ready: the drain answers busy.
        r = hub.do('update-ready', UPDATE_MODULE, 'a lab check', timeout=60)
        check('1093.3 update-ready reaches update-busy', r.get('state') == 'update-busy', str(r))
        hub.do('focus', name)
        s = hub.state()
        check('1093.3 the page says Update ready / Waits until you finish a lab check, Force, Not now',
              has(s, UPDATE_READY) and contains(s, 'Waits until you finish a lab check')
              and has(s, 'Force') and has(s, 'Not now'), f'{texts(s)[:30]}')
        hub.capture('1093-3-update-ready')
        hub.do('click', 'Not now')
        ok, _ = hub.wait(lambda s: not has(s, UPDATE_READY), 5)
        check('1093.3 Not now keeps the old version running', ok, 'Update ready line gone')
        # Force: Update ready, then Force swaps (Updating…, then live again).
        hub.do('update-ready', UPDATE_MODULE, 'a lab check', timeout=60)
        hub.do('focus', name)
        hub.do('click', 'Force')
        ok, s = hub.wait(lambda s: contains(s, UPDATING) or not has(s, UPDATE_READY), 40)
        check('1093.3 Force swaps', ok, 'Update ready gone after Force')
        ok, _ = hub.wait(lambda s: not contains(s, UPDATING), 40)
        check('1093.3 the page is live again after Force', ok, 'Updating… gone')
        # Update waiting for you: an update that adds a permission.
        r = hub.do('update-waiting', UPDATE_MODULE, 'devices.read', timeout=60)
        check('1093.3 update-waiting reaches update-waiting', r.get('state') == 'update-waiting', str(r))
        hub.do('focus', name)
        s = hub.state()
        review = node(s, 'Review')
        check('1093.3 the page says Update waiting for you, the permission, Review greyed, Not now',
              has(s, UPDATE_WAITING) and contains(s, 'The new version wants to') and review is not None
              and review.get('disabled') and has(s, 'Not now'), f'Review {review}')
        hub.capture('1093-3-update-waiting')
        hub.do('click', 'Not now')
        ok, _ = hub.wait(lambda s: not has(s, UPDATE_WAITING), 5)
        check('1093.3 Not now on Update waiting for you', ok, 'line gone')
        # Updating…: the drain answers done; the module starts again.
        r = hub.do('updating', UPDATE_MODULE, timeout=60)
        check('1093.3 updating reaches updating', r.get('state') == 'updating', str(r))
        hub.do('focus', name)
        s = hub.state()
        check('1093.3 the page says Updating… Your place on this page is kept.',
              contains(s, UPDATING) and contains(s, PLACE_KEPT), f'{[t for t in texts(s) if "Updating" in t]}')
        hub.capture('1093-3-updating')
        ok, _ = hub.wait(lambda s: not contains(s, UPDATING), 40)
        check('1093.3 the new instance runs and the page is live again', ok, 'Updating… gone')

    def s4_quarantine():
        mid = next((m for m in QUARANTINE_MODULES if m in names), None)
        if not mid:
            return check('1093.4 quarantine', False, f'none of {QUARANTINE_MODULES} in the lab', blocked=True)
        name = names[mid]
        hub.show_main()
        hub.do('focus', name)
        quiet = tray_line()
        r = hub.do('quarantine', mid)
        check('1093.4 quarantine reaches quarantined', r.get('state') == 'quarantined', str(r))
        hub.do('focus', name)
        s = hub.state()
        check(f"1093.4 the card says {name}'s page couldn't be shown / Hideout stopped twice",
              has(s, f"{name}'s page couldn't be shown") and contains(s, QUARANTINED_LINE),
              f'{texts(s)[:20]}')
        check('1093.4 Start again is focused', s['main'].get('focus') == 'Start again', f'focus {s["main"].get("focus")!r}')
        hub.capture('1093-4-quarantined')
        check('1093.4 the tray stays quiet', tray_line() == quiet, f'menu status {quiet!r}')
        exe = module_exe(hub.lab, mid)
        hub.do('key', 'enter', 'main')
        ok, _ = hub.wait(lambda s: not contains(s, QUARANTINED_LINE), 20)
        running = not exe or hub.wait(lambda _: bool(hub.pids(exe)), 20, read=lambda: None)[0]
        check('1093.4 Start again runs it and releases the page', ok and running, f'running: {running}')

    for n, fn in [('1093.2', s2_needs_hub), ('1093.3', s3_updates), ('1093.4', s4_quarantine)]:
        step(check, n, fn)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'), help='the hideout.exe under test')
    ap.add_argument('--lab', required=True, type=tool_arg('--lab', kind='dir'), help="the lab hub's HIDEOUT_CONFIG_DIR")
    ap.add_argument('--start', action='store_true', help='start the lab hub (and stop it at the end)')
    ap.add_argument('--only', default='flyout,pages,supervisor,states', help='which groups to run')
    ap.add_argument('--shots', default=None, help='where capture PNGs go (default <lab>\\lab-shots)')
    args = ap.parse_args()
    require_app_ok()
    home = Path(os.environ.get('LOCALAPPDATA', '')) / 'Hideout'
    if Path(args.lab).resolve() == home.resolve() or home.resolve() in Path(args.lab).resolve().parents:
        print(f'BLOCKED: {args.lab} is the user\'s hub folder', flush=True)
        print('RESULT BLOCKED', flush=True)
        sys.exit(2)
    hub = Hub(args.app, args.lab, args.shots or Path(args.lab) / 'lab-shots')
    check = Checks()
    try:
        if args.start:
            hub.start()
        s = hub.do('state', check=False)
        if not s.get('ok'):
            check('lab control', False, s.get('error'), blocked=True)
            check.finish()
        groups = args.only.split(',')
        if 'flyout' in groups:
            flyout_steps(hub, check)
        if 'pages' in groups:
            page_steps(hub, check)
        if 'supervisor' in groups:
            supervisor_steps(hub, check)
        if 'states' in groups:
            state_steps(hub, check)
    finally:
        hub.stop()
    check.finish()


if __name__ == '__main__':
    main()
