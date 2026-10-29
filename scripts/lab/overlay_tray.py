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

It launches the build under test on a temp FOXGLOVE_CONFIG_DIR (the user's Foxglove is stopped by
its exact name and started again by its path), so it runs only with FOXGLOVE_APP_OK=1 (On-call).
The keyboard cases need the keyboard (BLOCKED without it); the guard case a dev build.

    FOXGLOVE_APP_OK=1 python scripts/lab/overlay_tray.py --app <foxglove.exe>  (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, json, subprocess, time
from labkit import App, tool_arg, Checks, REPO, TARGET, Windows, connect, log_lines, require_app_ok, temp_config, wait

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=tool_arg('--app'), help='the Foxglove build under test')
ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
ap.add_argument('--skip-guard', action='store_true', help='leave out the split-link drop (GA build)')
args = ap.parse_args()
require_app_ok()
check = Checks()
PERMISSIONS = ['devices.read', 'app.windows', 'art.write']


def call(api, method, params=None):
    """The result, or None with the error printed (a check then fails on it)."""
    try:
        return api.call(method, params or {})
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


cfg = temp_config('foxglove-overlay-tray-')
with App(args.app, cfg) as app:
    api = connect('test-lab-overlay-tray', PERMISSIONS)
    granted = set(api.granted)
    if check('API 1.6 grants the window and art writes', {'app.windows', 'art.write'} <= granted, sorted(granted)):
        overlay_cases(api, Windows(app.proc.pid), cfg)
        keyboard_cases(api, cfg, app.env)
    api.close()
check.finish()
