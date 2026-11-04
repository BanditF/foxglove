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
  has the "(display change)" restore line. BLOCKED, not FAIL (#678), when Windows' own display
  change restored the overlay first (the app does that within SETTLE, as designed), or when the PC
  is away (locked or display off, `foxglove-lab away`): the app then holds display changes until
  it is back (#683).
- agent controls (#618): tray.set_panel open/close and overlay.restore, one log line each; the
  tray's Next › and ‹ through art.select, read back with art.status, one "art … (api)" line each.
  The firmware has one built-in since #671, so the run first shows a bundled piece from this PC
  (fury.bin): Next › then shows the built-in, and ‹ from it shows the last kept piece, or with
  none kept the app says there is nothing else to show and the built-in stays.
- re-apply after a display change (#601/#622): the built-in pinned through the API, another
  piece sent behind the app's back (foxglove-lab art_push of a bundled blob: the D6 push even
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
  lands after the frame's paint. A hit test that lands on a window above the overlay in the
  z-order (the lock screen, a shell notification window) is BLOCKED, naming it (#678). With --focus (it takes the focus: the PC unlocked and the user
  away 2 min, else BLOCKED): a second launch focuses Settings, which arms the overlay over the
  keys (once its keymap session connected: the user's remembered keyboard is seeded into the temp
  store); minimizing Settings (ShowWindow, the user's #549 report) closes it, keeps the app
  running and makes the overlay click-through again; and the tray panel, opened by the tray
  click, closes when the focus goes to Settings (a second launch). A focus case is BLOCKED, not
  FAIL, when Windows' foreground lock keeps the focus where it was (a posted message or a launch
  from a background process carries no foreground right): --real-clicks (IA3) then covers it.

It launches the build under test on a temp FOXGLOVE_CONFIG_DIR (the user's Foxglove is stopped by
its exact name and started again by its path), so it runs only with FOXGLOVE_APP_OK=1 (On-call).
The keyboard cases need the keyboard (BLOCKED without it); the guard case a dev build. When the
test app ends mid-run the run stops with one FAIL saying how (exit code, its log's last lines).

- real clicks (--real-clicks; the sitting's old I1-I4, now IA3): real mouse input through
  SendInput (scripts/lab/desktop.py), routed by Windows as a person's click: Settings' minimize
  button (found by WM_NCHITTEST), a click on the faded keyboard reaching a lab probe window under
  the overlay, a click on the tray icon (Shell_NotifyIconGetRect) opening the panel, and a click
  on another probe window ("any browser tab") moving the focus and closing it (#406). Each click
  puts the mouse back. BLOCKED, not FAIL, when the icon is in the hidden-icons flyout or Settings
  draws its own title bar (the posted-message cases above then stand).

    FOXGLOVE_APP_OK=1 python scripts/lab/overlay_tray.py --app <foxglove.exe> [--focus] [--real-clicks]  (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, ctypes, json, subprocess, time
from labkit import (App, AppGone, tool_arg, Checks, REPO, TARGET, Windows, art_reapply_lines, away, centre,
                    click_through, connect, covering, injecting, log_lines, outside, require_app_ok, require_user_idle, same_box, temp_config,
                    users_keyboard, wait)

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=tool_arg('--app'), help='the Foxglove build under test')
ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
ap.add_argument('--skip-guard', action='store_true', help='leave out the split-link drop (GA build)')
ap.add_argument('--focus', action='store_true',
                help='also the cases that move the focus (Settings shown by a second launch): PC unlocked, user away')
ap.add_argument('--real-clicks', action='store_true',
                help="the sitting's I1-I4 with real mouse clicks (SendInput) on lab probe windows: PC unlocked, user away")
args = ap.parse_args()
require_app_ok()
if args.focus or args.real_clicks:
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

    # Display change (#620). Two things outside the app undo this case (#678), so each is named
    # BLOCKED, not FAIL: a display change Windows sends itself (a monitor waking or sleeping) lets
    # the app restore the lab's minimize within SETTLE, as designed; and away (locked or display
    # off: Windows sent one about every 9.5 s at night on 10/30), the app holds display changes
    # until the PC is back (#683), so the posted one restores nothing yet.
    own = len(log_lines(cfg, '(display change)'))
    win.minimize(hwnd)
    minimized, o = wait(lambda: call(api, 'overlay.window'), lambda o: o and o['iconic'], 5)
    windows_own = [l for l in log_lines(cfg, '(display change)')[own:] if 'restored to' in l]
    if not minimized and windows_own:
        check('lab hook: the overlay is minimized', False,
              f'Windows sent a display change of its own and the app restored the overlay, as designed: {windows_own[-1:]}',
              blocked=True)
    else:
        check('lab hook: the overlay is minimized', minimized, o)
    gone = away(args.lab)
    before = len(log_lines(cfg, '(display change)'))
    check('WM_DISPLAYCHANGE posted to the display watcher', win.display_change(), 'FoxgloveOverlayDisplays')
    if gone:
        why = f'{gone}: the app holds display changes until the PC is back (#683); rerun with it unlocked, display on'
        check('display change: the overlay is back, on a live display', False, why, blocked=True)
        check('display change: one restore line', False, why, blocked=True)
    else:
        ok, o = wait(lambda: call(api, 'overlay.window'), lambda o: o and not o['iconic'] and o['on_screen'], 10)
        check('display change: the overlay is back, on a live display', ok, o)
        lines = log_lines(cfg, '(display change)')[before:]
        if minimized:
            check('display change: one restore line', any('restored to' in l for l in lines), lines[-2:])
        else:  # only a minimized overlay is restored, so only then is there a line
            check('display change: one restore line', False, 'the overlay was not minimized, so nothing to restore',
                  blocked=True)

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


def hit_check(name, win, hwnd, at, ok, detail):
    """A WindowFromPoint check, BLOCKED when the point is under a window above the overlay in the
    z-order (#678: the lock screen and a shell "New notification" window covered the whole screen
    in the 10/30 gate runs, so every point, the panel's too, reached them): the hit test then
    proves nothing about the overlay's style or region."""
    cover = covering(hwnd, at, win.above)
    if cover:
        return check(name, False, f'{detail}: {win.describe(cover)} is above the overlay there, so it takes '
                                  'any click; rerun with the PC unlocked and nothing over the screen', blocked=True)
    return check(name, ok, f'{detail}; {hit_detail(win, hwnd, at)}')


def hit_detail(win, hwnd, at):
    """What the hit test reached (class, exe, title) and the overlay's enabled and visible state:
    the 11/04 batch saw one window, 6557032, at every point, the panel's too, and its report
    couldn't say what that window was or whether the overlay was disabled (WindowFromPoint skips
    disabled and hidden windows)."""
    reached = win.describe(at) if at else 'nothing'
    return f'reached {reached}; overlay enabled {win.enabled(hwnd)}, visible {win.visible(hwnd)}'


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
    hit_check('Settings not focused: a click on the overlay reaches the window underneath (WindowFromPoint)',
              win, hwnd, under, under is not None and under != hwnd, f'({x}, {y}) -> {under} (overlay {hwnd})')

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
        hit_check('panel open: a click on the panel reaches the panel', win, hwnd, at, ok,
                  f'({px}, {py}) -> {at} (overlay {hwnd})')
        pt = outside(p['rect'], rect)
        if pt is None:
            check('panel open: a click beside the panel', False, 'no point of the overlay away from the panel', blocked=True)
        else:
            under = win.at(*pt)
            hit_check('panel open: a click beside the panel reaches the window underneath', win, hwnd, under,
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
              "Settings didn't get the foreground (Windows' foreground lock); IA3's real clicks cover it", blocked=True)
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
                     'the overlay never got the foreground from a posted tray click; IA3\'s real clicks cover it',
                     blocked=True)
    settings, focused = show_settings(win, hwnd)
    if not focused:
        call(api, 'tray.set_panel', {'open': False})
        return check('focus loss closes the panel', False,
                     "Settings didn't get the foreground; IA3's real clicks cover it", blocked=True)
    ok, p = wait(lambda: call(api, 'tray.panel'), lambda p: p and not p['open'], 5)
    check('focus to another window: the panel closes (#406)', ok, p)
    with injecting('Settings minimize (lab hook)'):
        win.u.ShowWindow(settings, win.SW_MINIMIZE)


HTMINBUTTON = 8


def minimize_button(win, hwnd):
    """A screen point on `hwnd`'s minimize button, by Windows' own hit test (WM_NCHITTEST along the
    top band, right to left), or None (a window without one, or one that draws its own title bar
    and answers HTCLIENT)."""
    l, t, r, _ = win.rect(hwnd)
    send = win.u.SendMessageW
    send.restype = ctypes.c_ssize_t
    send.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]
    for y in range(t + 4, t + 48, 4):
        for x in range(r - 4, max(l, r - 400), -4):
            if send(hwnd, 0x0084, 0, ((y & 0xFFFF) << 16) | (x & 0xFFFF)) == HTMINBUTTON:
                return x, y
    return None


def real_click_cases(api, win, hwnd):
    """The sitting's I1-I4 with real input (SendInput, desktop.py): Windows routes each click as
    the mouse does, so what these see is what a person's click does. A lab probe window stands in
    for the desktop under the overlay (I2) and for "any browser tab" (I4); it records the clicks
    that reach it."""
    import desktop
    call(api, 'tray.set_panel', {'open': False})
    # #678's rules hold for real clicks too: away (locked, display off) no click reaches the desktop.
    gone = away(args.lab)
    if gone:
        return check('IA3: real clicks', False, f'{gone}: rerun with the PC unlocked and the display on', blocked=True)

    # I1: the minimize button of a focused Settings window, clicked (#549's report).
    settings, focused = show_settings(win, hwnd)
    if not check('I1: Settings shows for the minimize click', settings is not None, settings):
        return
    point = minimize_button(win, settings)
    if point is None:
        check('I1: a real click on Settings\' minimize button', False,
              "no point answers HTMINBUTTON (a drawn title bar?); IA2's ShowWindow minimize stands", blocked=True)
        with injecting('Settings minimize (lab hook)'):
            win.u.ShowWindow(settings, win.SW_MINIMIZE)
    else:
        sent = desktop.click(*point, 'I1 Settings minimize button')
        ok, _ = wait(lambda: bool(win.u.IsIconic(settings)) or not win.visible(settings), lambda v: v, 5)
        check('I1: a real click on the minimize button minimizes Settings', ok, f'{point}, {sent} events in')
    try:
        app.proc.wait(3)  # bounded: an app that quits ends within a few seconds
    except subprocess.TimeoutExpired:
        pass
    if not check('I1: the test app keeps running (in the tray)', app.exited() is None,
                 'running' if app.exited() is None else app.exit_report()):
        raise AppGone('the test app ended when Settings was minimized by its button')
    ok, style = wait(lambda: win.exstyle(hwnd), lambda s: click_through(s, win.visible(hwnd)), 5)
    check('I1: the overlay is click-through again', ok, hex(style))

    # I2: a real click on the faded keyboard picture reaches what is under it.
    o = call(api, 'overlay.window')
    if not check('I2: the overlay is on screen', bool(o and o['on_screen']), o):
        return
    x, y = centre(o['rect'])
    with desktop.Probe([x - 120, y - 60, x + 120, y + 60], 'Foxglove lab: under the overlay') as under:
        time.sleep(0.5)  # the probe's first paint
        # A window above the overlay there (the lock screen, a shell notification) would take the
        # click whatever the overlay does: BLOCKED, naming it (#678), not a click on it.
        cover = covering(hwnd, win.at(x, y), win.above)
        if cover:
            return check('I2: a real click on the overlay reaches the window under it (#549)', False,
                         f'{win.describe(cover)} is above the overlay at ({x}, {y}); rerun with nothing over the screen',
                         blocked=True)
        before = len(under.clicks)
        sent = desktop.click(x, y, 'I2 click on the overlay')
        ok, n = wait(lambda: len(under.clicks), lambda n: n > before, 2, poll=0.05)
        check('I2: a real click on the overlay reaches the window under it (#549)', ok,
              f'({x}, {y}): {n - before} click(s) in the probe, {sent} events in')

    # I3: a real click on the fox tray icon opens the panel.
    icons = win.find('tray_icon_app')
    back = desktop.cursor()
    rect = desktop.tray_icon_shown(icons[0]) if icons else None  # an auto-hidden taskbar slides in first
    if not check('I3: the tray icon is on the taskbar', rect is not None,
                 rect or "not found (in the hidden-icons flyout?): IA1's posted tray click stands", blocked=rect is None):
        desktop.move(*back, 'mouse back')
        return
    with desktop.Probe([x - 120, y - 300, x + 120, y - 180], 'Foxglove lab: another window') as other:
        time.sleep(0.5)
        sent = desktop.click(*centre(rect), 'I3 tray icon click')
        ok, p = wait(lambda: call(api, 'tray.panel'), lambda p: p and p['open'], 5)
        if not check('I3: a real tray click opens the panel', ok, f'{p}, {sent} events in'):
            return
        # I4: a real click on another window moves the focus and closes the panel (#406).
        px, py = centre(other.rect)
        sent = desktop.click(px, py, 'I4 click on another window')
        got = other.foreground(3)
        ok, p = wait(lambda: call(api, 'tray.panel'), lambda p: p and not p['open'], 5)
        check('I4: the click reached the other window', bool(other.clicks) and got,
              f'{len(other.clicks)} click(s), foreground {got}, {sent} events in')
        check('I4: the panel closed (#406)', ok, p)
    desktop.move(*back, 'mouse back where it was')
    call(api, 'tray.set_panel', {'open': False})


def keyboard_cases(api, cfg, env):
    devices = (call(api, 'devices.list') or {}).get('devices', [])
    if not devices:
        return check('keyboard cases', False, 'no keyboard with the host link', blocked=True)
    device = devices[0]['id']
    ok, start = wait(lambda: call(api, 'art.status', {'device': device}), lambda s: s is not None, 20)
    if not check('art.status reads the right screen', ok, start):
        return

    # The tray's Next › and ‹ (#618) with the firmware's one built-in (#671): from a piece sent
    # from this PC, Next › shows the keyboard's first piece, its built-in; ‹ from there wraps to
    # the last kept piece, or, with none kept, there is nothing else to show.
    fury = str(REPO / 'app' / 'resources' / 'lighting' / 'art' / 'fury.bin')
    start = call(api, 'art.select', {'device': device, 'target': {'file': fury}})
    if not check('art: a piece from this PC shows first', bool(start and start['pushed']), start):
        return
    nxt = call(api, 'art.select', {'device': device, 'target': 'next'})
    check("art Next ›: the keyboard's built-in shows", bool(nxt and nxt['built_in']), f'{start["name"]} -> {nxt}')
    back = call(api, 'art.select', {'device': device, 'target': 'previous'})
    if back:
        check('art ‹: the last kept piece shows', back['slot'] is not None, back)
    else:
        now = call(api, 'art.status', {'device': device})
        check('art ‹ with no kept piece: nothing else to show, the built-in stays',
              bool(now and nxt and now['built_in'] == nxt['built_in']), now)
    check('one log line per art write', len(log_lines(cfg, ' (api): ')) >= 2, log_lines(cfg, ' (api): ')[-2:])

    # Re-apply after a display change (#622): pin the built-in, lose it behind the app's back.
    names = [n for n in [(nxt or {}).get('built_in')] if n]
    if not names:
        return check('re-apply cases', False, 'needs the built-in showing (Next showed none)', blocked=True)
    pinned = call(api, 'art.select', {'device': device, 'target': {'built_in': names[0]}})
    check('pin a built-in through the API', bool(pinned and pinned['built_in'] == names[0]), pinned)
    status, detail = lab_case('lose-art', ['{ art_push = { blob = "app/resources/lighting/art/hearth.bin", '
                                           'expect_left_state = 3, expect_right_state = 2, within_ms = 60000 } }'], env)
    check('lab: another piece sent behind the app', status == 'PASS', detail)
    # art.status answers from the app's last read, which still says the pinned built-in: the
    # push went behind the app's back. Since #673 the display change reads the right afresh and
    # puts the pin back over the push, with one line; wait for that line (bounded), then read.
    reapplied = lambda: len(art_reapply_lines(cfg, 'display change'))  # labkit.ART_REAPPLY (#695's wording)
    before = reapplied()
    Windows(app.proc.pid).display_change()
    ok, after = wait(reapplied, lambda n: n > before, 20)
    check('display change: one re-applied line', ok and after == before + 1, after - before)
    ok, s = wait(lambda: call(api, 'art.status', {'device': device}), lambda s: s and s['built_in'] == names[0], 20)
    check('display change: the pinned set is re-applied', ok, s)
    Windows(app.proc.pid).display_change()
    time.sleep(5)  # bounded: a re-apply would have logged within the art worker's status read
    check('display change while it shows: nothing re-applied',
          reapplied() == after,
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
            if args.real_clicks and hwnd:
                real_click_cases(api, Windows(app.proc.pid), hwnd)
            keyboard_cases(api, cfg, app.env)
    except AppGone as e:
        # The test app ended mid-run: say how (its exit code and log), and stop; nothing after
        # this point ran.
        check('the test app runs to the end of the cases', False, f'{e}; {app.exit_report()}')
        check('the cases after it', False, 'not run: the test app had ended', blocked=True)
    api.close()
check.finish()
