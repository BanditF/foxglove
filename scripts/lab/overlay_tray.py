"""App-level cases of the stability pass (#618) for the overlay, the tray panel, the agent controls
and the re-apply hooks, read back through the local API (API 1.6) and the test app's log:

- tray click (#612/#620), first: the overlay minimized by the lab hook (ShowWindow), then the tray
  icon's left-button-up posted to the icon's window (the shell's own callback message; the app
  reads the icon's real rect): the panel must open (tray.panel, drawn inside the overlay's rect).
  The "(tray click)" restore line is expected only when overlay.window said minimized just before
  the click. It runs before the display change on purpose: the app counts a minimize within
  SETTLE (10 s) of a display change as part of it and restores the overlay itself with a
  "(display change)" line (overlay_restore::minimized_by_display_change), so a tray click after
  one finds the overlay already shown and rightly logs nothing (#618 follow-up).
- display change (#612/#620): the overlay minimized again, then WM_DISPLAYCHANGE posted to the
  app's display watcher: overlay.window must say not minimized and on a live display, and the log
  has the "(display change)" restore line.
- agent controls (#618): tray.set_panel open/close and overlay.restore, one log line each; the
  tray's Next › and ‹ through art.select, read back with art.status, one "art … (api)" line each.
- re-apply after a display change (#601/#622): a built-in pinned through the API, another set
  selected behind the app's back (foxglove-lab art_select with direct = true: the D6 SELECT even
  while the app runs), then WM_DISPLAYCHANGE: the pinned set comes back with one "re-applied the
  right screen's art (display change)" line; a second display change while it shows adds no line.
- re-apply after relay-ready (#622): the split link dropped once by the dev build's guard hook
  (D7 0B, foxglove-lab split_guard_test); once the relay is back the pinned set still shows.

- click-through (#549, #405/#406; the sitting's batch I): the overlay's window style and Windows'
  own hit test (WindowFromPoint, which routes a click as the mouse does). The tray panel is drawn
  inside the overlay window (not a window of its own): open, the overlay takes the mouse and its
  window region is the panel. With Settings not focused (the test app opens it behind at start)
  the overlay is WS_EX_TRANSPARENT | WS_EX_LAYERED and a click on it reaches the window underneath;
  with the tray panel open the overlay drops WS_EX_TRANSPARENT and only the panel's region takes
  clicks (a click beside it goes underneath); each is waited for, bounded, since the passthrough
  lands after the frame's paint. With --focus (it takes the focus: the PC unlocked and the user
  away 2 min, else BLOCKED): a second launch focuses Settings, which arms the overlay over the
  keys (once its keymap session connected: the user's remembered keyboard is seeded into the temp
  store); minimizing Settings (ShowWindow, the user's #549 report) closes it, keeps the app
  running and makes the overlay click-through again; and the tray panel, opened by the tray
  click, closes when the focus goes to Settings (a second launch). A focus case is BLOCKED, not
  FAIL, when Windows' foreground lock keeps the focus where it was (a posted message or a launch
  from a background process carries no foreground right): then the sitting keeps its eyes step.

It launches the build under test on a temp FOXGLOVE_CONFIG_DIR (the user's Foxglove is stopped by
its exact name and started again by its path), so it runs only with FOXGLOVE_APP_OK=1 (On-call).
The keyboard cases need the keyboard (BLOCKED without it); the guard case a dev build. When the
test app ends mid-run the run stops with one FAIL saying how (exit code, its log's last lines).

    FOXGLOVE_APP_OK=1 python scripts/lab/overlay_tray.py --app <foxglove.exe> [--focus]  (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, json, subprocess, time
from labkit import (App, AppGone, tool_arg, Checks, REPO, TARGET, Windows, centre, click_through, connect, injecting,
                    log_lines, outside, require_app_ok, require_user_idle, same_box, temp_config, users_keyboard, wait)

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=tool_arg('--app'), help='the Foxglove build under test')
ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
ap.add_argument('--skip-guard', action='store_true', help='leave out the split-link drop (GA build)')
ap.add_argument('--focus', action='store_true',
                help='also the cases that move the focus (Settings shown by a second launch): PC unlocked, user away')
args = ap.parse_args()
require_app_ok()
if args.focus:
    require_user_idle(120)
check = Checks()
PERMISSIONS = ['devices.read', 'app.windows', 'art.write']


def call(api, method, params=None):
    """The result, or None with the error printed (a check then fails on it). A closed pipe (the
    test app ended) raises AppGone, which ends the run with the app's exit report."""
    try:
        return api.call(method, params or {})
    except AppGone:
        raise
    except RuntimeError as e:
        print(f'  {method}: {e}', flush=True)
        return None


def lab_case(name, steps, env):
    """One foxglove-lab case next to the running test app; returns (status, the steps' results)."""
    out = REPO / 'lab-reports' / f'overlay-tray-{name}-{time.time_ns()}'
    path = out.with_suffix('.toml')
    out.parent.mkdir(exist_ok=True)
    path.write_text(f'schema = 1\n[[cases]]\nname = "{name}"\ndescription = "{name}"\nsteps = [\n  ' +
                    ',\n  '.join(steps) + ',\n]\n', encoding='utf-8')
    subprocess.run([args.lab, '--allow-app', 'run', '--suite', str(path), '--non-interactive', '--out', str(out)],
                   capture_output=True, text=True, encoding='utf-8', errors='replace', env=env, cwd=REPO, timeout=180)
    case = json.loads((out / 'report.json').read_text(encoding='utf-8'))['cases'][0]
    return case['status'], [s['actual'] for s in case['steps']]


def overlay_cases(api, win, cfg):
    first = wait(lambda: call(api, 'overlay.window'), lambda o: o is not None, 30)[1]
    if not check('the overlay probe answers', first is not None, first):
        return
    hwnd = win.overlay(first['rect'])
    if not check('the lab finds the overlay window', hwnd is not None, f'rect {first["rect"]}'):
        return

    # Tray click (#620): the full #612 path. First, before any display change: a minimize within
    # SETTLE of one is restored by the app as part of that change (see the module docs).
    win.minimize(hwnd)
    minimized, o = wait(lambda: call(api, 'overlay.window'), lambda o: o and o['iconic'], 5)
    check('lab hook: the overlay is minimized before the tray click', minimized, o)
    before = len(log_lines(cfg, '(tray click)'))
    check('tray click posted to the icon window', win.tray_click(), 'tray_icon_app')
    ok, p = wait(lambda: call(api, 'tray.panel'), lambda p: p and p['open'] and p['rect'], 10)
    check('tray click: the panel opens and is drawn', ok, p)
    o = call(api, 'overlay.window')
    inside = bool(ok and o and o['on_screen'] and o['rect'][0] <= p['rect'][0] and p['rect'][2] <= o['rect'][2]
                  and o['rect'][1] <= p['rect'][1] and p['rect'][3] <= o['rect'][3])
    check('tray click: the panel is inside the overlay, on screen', inside, {'panel': p, 'overlay': o})
    if minimized:  # only a minimized overlay is restored, so only then is there a line
        lines = log_lines(cfg, '(tray click)')[before:]
        check('tray click: the restore line', any('restored to' in l for l in lines), lines[-1:])
    call(api, 'tray.set_panel', {'open': False})

    # Display change (#620).
    win.minimize(hwnd)
    ok, o = wait(lambda: call(api, 'overlay.window'), lambda o: o and o['iconic'], 5)
    check('lab hook: the overlay is minimized', ok, o)
    before = len(log_lines(cfg, '(display change)'))
    check('WM_DISPLAYCHANGE posted to the display watcher', win.display_change(), 'FoxgloveOverlayDisplays')
    ok, o = wait(lambda: call(api, 'overlay.window'), lambda o: o and not o['iconic'] and o['on_screen'], 10)
    check('display change: the overlay is back, on a live display', ok, o)
    lines = log_lines(cfg, '(display change)')[before:]
    check('display change: one restore line', any('restored to' in l for l in lines), lines[-2:])

    # Agent controls (#618).
    p = call(api, 'tray.set_panel', {'open': False})
    check('tray.set_panel close: closed', p == {'open': False, 'rect': None}, p)
    p = call(api, 'tray.set_panel', {'open': True})
    check('tray.set_panel open: open and drawn', bool(p and p['open'] and p['rect']), p)
    p = call(api, 'tray.set_panel', {'open': False})
    check('tray.set_panel close again', p == {'open': False, 'rect': None}, p)
    check('one log line per panel write', len(log_lines(cfg, '(api asked ')) >= 3, log_lines(cfg, '(api asked ')[-3:])
    win.minimize(hwnd)
    o = call(api, 'overlay.restore')
    check('overlay.restore: back on screen', bool(o and o['on_screen'] and not o['iconic']), o)
    check('overlay.restore: its log line', bool(log_lines(cfg, 'restored to') and log_lines(cfg, '(api)')),
          log_lines(cfg, '(api)')[-1:])


def click_cases(api, win):
    """#549 and #405/#406 by the window style and Windows' own hit test (no mouse, no clicks)."""
    call(api, 'tray.set_panel', {'open': False})
    o = call(api, 'overlay.restore')
    hwnd = win.overlay(o['rect']) if o else None
    if not check('click-through: the lab finds the overlay', hwnd is not None, o):
        return None
    rect = o['rect']

    # Settings not focused: the overlay passes every click on. The test app opens Settings at
    # every start, but a launch from a background process gets no foreground, so Settings shows
    # behind, unfocused: the same rule as closed (overlay_grab), and closing it comes later
    # (Settings minimized, --focus).
    ok, style = wait(lambda: win.exstyle(hwnd), lambda s: click_through(s, win.visible(hwnd)), 5)
    check('Settings not focused: the overlay is click-through (WS_EX_TRANSPARENT | WS_EX_LAYERED)', ok, hex(style))
    x, y = centre(rect)
    under = win.at(x, y)
    check('Settings not focused: a click on the overlay reaches the window underneath (WindowFromPoint)',
          under is not None and under != hwnd, f'({x}, {y}) -> {under} (overlay {hwnd})')

    # The tray panel open: only the panel takes clicks (#405), the rest goes underneath (#406).
    # tray.set_panel answers from the frame that drew the panel (tray_panel::show); that frame
    # then sets the window region at once (SetWindowRgn), while the mouse passthrough is an egui
    # viewport command winit applies only after the frame's paint. Each is waited for (bounded),
    # never read once: the 10/29 pre-sitting run read the style a moment after the region and
    # saw the overlay still WS_EX_TRANSPARENT.
    p = call(api, 'tray.set_panel', {'open': True})
    if check('panel open for the hit test', bool(p and p['open'] and p['rect']), p):
        t0 = time.time()
        ok, box = wait(lambda: win.region_box(hwnd), lambda b: same_box(b, p['rect']), 5, poll=0.05)
        check("panel open: the overlay's window region is the panel", ok,
              {'region': box, 'panel': p['rect'], 'after_s': round(time.time() - t0, 2)})
        ok, style = wait(lambda: win.exstyle(hwnd), lambda s: not click_through(s, win.visible(hwnd)), 5, poll=0.05)
        check('panel open: the overlay takes the mouse (not transparent)', ok,
              f'{hex(style)} after {time.time() - t0:.2f} s')
        px, py = centre(p['rect'])
        ok, at = wait(lambda: win.at(px, py), lambda h: h == hwnd, 2, poll=0.05)
        check('panel open: a click on the panel reaches the panel', ok, f'({px}, {py}) -> {at} (overlay {hwnd})')
        pt = outside(p['rect'], rect)
        if pt is None:
            check('panel open: a click beside the panel', False, 'no point of the overlay away from the panel', blocked=True)
        else:
            under = win.at(*pt)
            check('panel open: a click beside the panel reaches the window underneath',
                  under is not None and under != hwnd, f'{pt} -> {under} (overlay {hwnd})')
    call(api, 'tray.set_panel', {'open': False})
    return hwnd


def show_settings(win, hwnd):
    """A second launch shows the running test app's Settings (#345) and gives it the focus when
    Windows lets it: (the Settings window or None, whether it got the foreground)."""
    with injecting('second launch shows Settings (lab hook)'):
        subprocess.run([str(app.exe)], env=app.env, timeout=30, capture_output=True)
    ok, shown = wait(lambda: win.others(hwnd), lambda s: bool(s), 10)
    if not ok:
        return None, False
    settings = shown[0]
    focused, _ = wait(lambda: win.foreground(), lambda f: f == settings, 5)
    return settings, focused


def focus_cases(api, win, hwnd):
    """#549 with Settings really focused, then minimized; #406 the panel closing on focus loss."""
    settings, focused = show_settings(win, hwnd)
    if not check('second launch: Settings shows', settings is not None, settings):
        return
    if not focused:
        check('Settings focused: the overlay arms over the keys', False,
              "Settings didn't get the foreground (Windows' foreground lock); the sitting keeps I1-I2", blocked=True)
    else:
        # The overlay takes the mouse over its keys only once it drew them: its keymap session is
        # connected (the remembered keyboard, seeded from the user's store) and the keymap read.
        armed = lambda: (win.exstyle(hwnd), win.region_box(hwnd))
        ok, (style, box) = wait(armed, lambda a: not click_through(a[0], win.visible(hwnd)) and a[1] is not None,
                                KEYS_S)
        connected = log_lines(cfg, CONNECTED)
        if ok:
            check('Settings focused: the overlay takes the mouse', True, hex(style))
            check('Settings focused: only over the keys (a window region)', True, box)
        elif connected or not KEYBOARD:
            # Connected and still nothing over the keys: the app's bug. No remembered keyboard: the
            # test app can't draw keys, which is a setup gap (BLOCKED), named as such.
            why = (f'{hex(style)}, region {box}; the overlay connected ({connected[-1]})' if connected else
                   "no remembered keyboard in the user's store ([connection.last]): the test app's overlay draws no keys")
            check('Settings focused: the overlay takes the mouse over its keys', False, why, blocked=not connected)
        else:
            check('Settings focused: the overlay takes the mouse over its keys', False,
                  f"{hex(style)}, region {box}: no keys within {KEYS_S} s and no '{CONNECTED}' line (the overlay's "
                  "keymap session didn't connect; builds before #618's fix don't log it)", blocked=True)
        with injecting('Settings minimize (lab hook)'):
            win.u.ShowWindow(settings, win.SW_MINIMIZE)
        ok, style = wait(lambda: win.exstyle(hwnd), lambda s: click_through(s, win.visible(hwnd)), 5)
        # Minimized is closed (#549). Before #618's fix, closing Settings in an app whose overlay
        # never connected quit the whole app (KeyPeek's rule), as on 10/29.
        try:  # bounded: an app that quits ends within a few seconds of the frame that saw it
            app.proc.wait(3)
        except subprocess.TimeoutExpired:
            pass
        alive = app.exited() is None
        check('Settings minimized: the test app keeps running (in the tray)', alive,
              'running' if alive else app.exit_report())
        if not alive:
            raise AppGone('the test app ended when Settings was minimized')
        check('Settings minimized: the overlay is click-through again (#549)', ok, hex(style))
        o = call(api, 'overlay.window')
        if o:
            x, y = centre(o['rect'])
            under = win.at(x, y)
            check('Settings minimized: a click on the overlay reaches the window underneath',
                  under is not None and under != hwnd, f'({x}, {y}) -> {under}')

    # #406: the panel closes when the overlay loses the focus.
    win.tray_click()
    ok, p = wait(lambda: call(api, 'tray.panel'), lambda p: p and p['open'], 10)
    if not check('tray click: the panel opens', ok, p):
        return
    got, _ = wait(lambda: win.foreground(), lambda f: f == hwnd, 3)
    if not got:
        call(api, 'tray.set_panel', {'open': False})
        return check('focus loss closes the panel', False,
                     'the overlay never got the foreground from a posted tray click; the sitting keeps I3-I4',
                     blocked=True)
    settings, focused = show_settings(win, hwnd)
    if not focused:
        call(api, 'tray.set_panel', {'open': False})
        return check('focus loss closes the panel', False,
                     "Settings didn't get the foreground; the sitting keeps I3-I4", blocked=True)
    ok, p = wait(lambda: call(api, 'tray.panel'), lambda p: p and not p['open'], 5)
    check('focus to another window: the panel closes (#406)', ok, p)
    with injecting('Settings minimize (lab hook)'):
        win.u.ShowWindow(settings, win.SW_MINIMIZE)


def keyboard_cases(api, cfg, env):
    devices = (call(api, 'devices.list') or {}).get('devices', [])
    if not devices:
        return check('keyboard cases', False, 'no keyboard with the host link', blocked=True)
    device = devices[0]['id']
    ok, start = wait(lambda: call(api, 'art.status', {'device': device}), lambda s: s is not None, 20)
    if not check('art.status reads the right screen', ok, start):
        return

    # The tray's Next › and ‹ (#618).
    nxt = call(api, 'art.select', {'device': device, 'target': 'next'})
    check('art Next ›: another piece shows', bool(nxt and nxt['name'] != start['name']), f'{start["name"]} -> {nxt}')
    back = call(api, 'art.select', {'device': device, 'target': 'previous'})
    check('art ‹: the piece before comes back', bool(back and back['name'] == start['name']), back)
    check('one log line per art write', len(log_lines(cfg, ' (api): ')) >= 2, log_lines(cfg, ' (api): ')[-2:])

    # Re-apply after a display change (#622): pin a built-in, lose it behind the app's back.
    names = [n for n in [start.get('built_in'), (nxt or {}).get('built_in')] if n]
    if len(names) < 2:
        return check('re-apply cases', False, 'needs two built-in sets showing (Next showed no built-in)', blocked=True)
    pinned = call(api, 'art.select', {'device': device, 'target': {'built_in': names[0]}})
    check('pin a built-in through the API', bool(pinned and pinned['built_in'] == names[0]), pinned)
    status, detail = lab_case('lose-art', ['{ art_select = { set = 1, restore = false, direct = true } }'], env)
    lost = call(api, 'art.status', {'device': device})
    if lost and lost['built_in'] == names[0]:  # set 1 was the pinned one: lose it to set 0
        status, detail = lab_case('lose-art', ['{ art_select = { set = 0, restore = false, direct = true } }'], env)
    check('lab: another set selected behind the app', status == 'PASS', detail)
    before = len(log_lines(cfg, "re-applied the right screen's art (display change)"))
    Windows(app.proc.pid).display_change()
    ok, s = wait(lambda: call(api, 'art.status', {'device': device}), lambda s: s and s['built_in'] == names[0], 20)
    check('display change: the pinned set is re-applied', ok, s)
    after = len(log_lines(cfg, "re-applied the right screen's art (display change)"))
    check('display change: one re-applied line', after == before + 1, after - before)
    Windows(app.proc.pid).display_change()
    time.sleep(5)  # bounded: a re-apply would have logged within the art worker's status read
    check('display change while it shows: nothing re-applied',
          len(log_lines(cfg, "re-applied the right screen's art (display change)")) == after,
          'no SELECT, no line')

    # Re-apply after relay-ready (#622): drop the split link once (dev build's guard hook).
    if args.skip_guard:
        return check('relay-ready re-apply', False, '--skip-guard', blocked=True)
    status, detail = lab_case('relink', ['{ split_guard_test = { within_ms = 60000 } }'], env)
    if status == 'BLOCKED':
        return check('relay-ready re-apply', False, detail, blocked=True)
    check('lab: the right re-linked (guard hook)', status == 'PASS', detail)
    ok, s = wait(lambda: call(api, 'art.status', {'device': device}), lambda s: s and s['built_in'] == names[0], 30)
    check('relay ready: the pinned set shows', ok, s)
    print('  re-apply lines:', log_lines(cfg, '(right relay ready)')[-3:], flush=True)


# The keyboard the user's Foxglove reconnects to, so the test app's overlay connects and draws
# its keys (the Settings-focused case needs them); '' when the user's store has none.
KEYBOARD = users_keyboard()
CONNECTED = "the overlay connected to the keyboard's keymap"
KEYS_S = 30  # the overlay's connect plus a fresh keymap read over USB
print('remembered keyboard for the test app:', KEYBOARD.strip().replace('\n', '; ') or 'none', flush=True)
cfg = temp_config('foxglove-overlay-tray-', KEYBOARD)
with App(args.app, cfg) as app:
    api = connect('test-lab-overlay-tray', PERMISSIONS)
    granted = set(api.granted)
    try:
        if check('API 1.6 grants the window and art writes', {'app.windows', 'art.write'} <= granted, sorted(granted)):
            overlay_cases(api, Windows(app.proc.pid), cfg)
            hwnd = click_cases(api, Windows(app.proc.pid))
            if args.focus and hwnd:
                focus_cases(api, Windows(app.proc.pid), hwnd)
            keyboard_cases(api, cfg, app.env)
    except AppGone as e:
        # The test app ended mid-run: say how (its exit code and log), and stop; nothing after
        # this point ran.
        check('the test app runs to the end of the cases', False, f'{e}; {app.exit_report()}')
        check('the cases after it', False, 'not run: the test app had ended', blocked=True)
    api.close()
check.finish()
