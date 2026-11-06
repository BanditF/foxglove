"""The sitting's batch P (#609, #604, PR #651) without the user: the PC asleep and awake, Foxglove quit,
and the keyboard going dark on its own, with no real display-off (on this PC that starts Modern
Standby, so the probes that did it are retired, #651).

The build under test runs on a temp FOXGLOVE_CONFIG_DIR with the away test hook
(FOXGLOVE_AWAY_OVERRIDE, app/src/foxglove/away.rs): a watched file stands in for Windows' display
notification, so everything after the edge is the app's real path (pc_state.rs telling the left
half, which tells the right). The keyboard is read with foxglove-lab: rgb-watch (each D7 83 event:
both halves' lit, wanted, idle) and pc-state (D7 11 read: held, reason).

- PA1: the app says "awake" when the keyboard connects; PC_STATE held, reason 0 (the lease).
  When both halves' underglow is off by the user's choice (wanted false) the run turns it on
  through the API for the run (dark against lit needs a lit keyboard) and puts it back at the end.
- old P1-P2 (PA2): the hook says "display off": the log has "PC state to the keyboard: away
  (display off)" and both halves report dark within --edge-s (D7 83 comes about 600 ms after a
  change).
- old P3 (PA2): "display on": "awake, wake (display on)" and both halves lit within --edge-s; the
  right's screen readback changes within 5 s (its art moves again).
- the lock (PA2): "locked" alone tells the keyboard nothing and both halves stay lit (#609
  decision 1); "unlocked" is #604's back: "awake, wake (session unlocked)".
- PA3: pc-state reads Awake, reason 0: the app's last word was awake before the quit.
- old P4 (PA4): the test app is stopped (by its own handle, as Quit ends it: nothing more is sent),
  no key is pressed, and the keyboard is read for up to --dark-within-s (700): rgb-watch in
  segments with a pc-state read between them (about every 2 minutes). The lease holds it lit
  first (reason 0), then the USB idle time (reason 1), then it idles (reason 4).
- old P5 (PA4): both halves report dark, the hold has ended.
- old P6 (PA4): the user's Foxglove starts again by its path, and runs.

--history also reads the user's own foxglove.log for the real display edges of normal nights
("PC state to the keyboard: away (display off)" then "awake, wake (display on)"): the Windows
notification the hook stands in for, measured, never caused.

Needs both halves on firmware with PC_STATE (HELLO bit 26) and the left on USB. It launches the
app (FOXGLOVE_APP_OK=1, On-call), and the keyboard must stay untouched about 13 minutes (the user
away 2 min first; a key pressed meanwhile makes the dark step BLOCKED, not FAIL).

    FOXGLOVE_APP_OK=1 python scripts/lab/pc_away.py --app <foxglove.exe> [--lab <foxglove-lab.exe>] [--history]
    (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse
import json
import subprocess
import threading
import time
from pathlib import Path

from labkit import (App, Checks, REAL_CONFIG, REPO, TARGET, connect, log_lines, require_app_ok, require_user_idle,
                    restore_lights, rgb, set_lights, temp_config, tool_arg, user_idle, users_keyboard, wait)

AWAY_LINE = 'PC state to the keyboard: away (display off)'
WAKE_LINE = 'PC state to the keyboard: awake, wake (display on)'
CONNECT_LINE = 'PC state to the keyboard: awake (keyboard connected)'


# ----------------------------------------------------------------------------- pure parts

def status_json(out):
    """`foxglove-lab status` output → {'RGB_SHOWN': {...}, 'PC_STATE': {...}} (the JSON lines)."""
    found = {}
    for line in out.splitlines():
        key, _, rest = line.partition(': ')
        if key in ('RGB_SHOWN', 'PC_STATE') and rest.startswith('{'):
            try:
                found[key] = json.loads(rest)
            except ValueError:
                pass
    return found


def both(event, field, value):
    """Whether an rgb-watch line's rgb_state (or baseline) has `field` == `value` on both halves."""
    s = event.get('rgb_state') or event.get('baseline') or {}
    return all(isinstance(s.get(h), dict) and s[h].get(field) == value for h in ('left', 'right'))


def first_both(events, field, value, after=0.0):
    """The wall time of the first event at or after `after` with `field` == `value` on both halves."""
    return next((t for t, e in events if t >= after and both(e, field, value)), None)


def history(lines):
    """The real display edges in a foxglove.log: [(away line time, wake line time or None)]."""
    pairs, away = [], None
    for l in lines:
        if AWAY_LINE in l:
            away = l[:20]
        elif WAKE_LINE in l and away:
            pairs.append((away, l[:20]))
            away = None
    if away:
        pairs.append((away, None))
    return pairs


# ----------------------------------------------------------------------------- the run

class Watch:
    """One `foxglove-lab rgb-watch` run in the background: its lines, each with the wall time it
    arrived (Rust's stdout flushes each line). Ends at its own deadline or on stop()."""

    def __init__(self, lab, env, seconds):
        self.events, self.base = [], threading.Event()
        self.proc = subprocess.Popen([str(lab), '--allow-app', 'rgb-watch', '--seconds', str(int(seconds))],
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8',
                                     errors='replace', env=env, cwd=REPO)
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stdout:
            try:
                e = json.loads(line)
            except ValueError:
                continue
            self.events.append((time.time(), e))
            if 'baseline' in e:
                self.base.set()

    def started(self, seconds=20):
        return self.base.wait(seconds)

    def stop(self):
        if self.proc.poll() is None:
            self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        return self.proc.stderr.read() if self.proc.stderr else ''


def lab_out(lab, env, *argv, timeout=60):
    r = subprocess.run([str(lab), '--allow-app', *argv], capture_output=True, text=True, encoding='utf-8',
                       errors='replace', env=env, cwd=REPO, timeout=timeout)
    return r.returncode, r.stdout, r.stderr


def pc_state(lab, env):
    code, out, err = lab_out(lab, env, 'pc-state')
    try:
        return code, json.loads(out.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return code, {'error': (err or out).strip()[-200:]}


def edge(check, lab, env, hook, text, field, value, line, cfg, name, edge_s):
    """Writes the hook, then waits for the log line and both halves' `field` == `value`."""
    w = Watch(lab, env, edge_s + 8)
    if not w.started():
        check(f'{name}: rgb-watch starts', False, w.stop(), blocked=True)
        return False
    before = len(log_lines(cfg, line))
    t0 = time.time()
    Path(hook).write_text(text + '\n', encoding='utf-8')
    ok_line, _ = wait(lambda: len(log_lines(cfg, line)), lambda n: n > before, 5, poll=0.1)
    seen, at = wait(lambda: first_both(w.events, field, value, t0), lambda t: t is not None, edge_s + 2, poll=0.05)
    w.stop()
    check(f'{name}: the app tells the keyboard ("{line}")', ok_line, log_lines(cfg, line)[-1:])
    lat = (at - t0) if at else None
    check(f'{name}: both halves {"lit" if value else "dark"} within {edge_s} s', bool(seen and lat <= edge_s),
          f'{lat:.2f} s' if lat is not None else f'no event: {[e for _, e in w.events][-2:]}')
    return bool(seen)


def lock_edges(check, lab, env, hook, cfg):
    """A lock alone changes nothing (#609 decision 1): no PC-state line, both halves stay lit; the
    unlock is #604's "back": "awake, wake (session unlocked)"."""
    w = Watch(lab, env, 8)
    w.started()
    before = len(log_lines(cfg, 'PC state to the keyboard:'))
    t0 = time.time()
    Path(hook).write_text('display on\nlocked\n', encoding='utf-8')
    time.sleep(4)  # bounded: an edge would have been told and reported within it
    w.stop()
    told = log_lines(cfg, 'PC state to the keyboard:')[before:]
    dark = first_both(w.events, 'lit', False, t0)
    check('lock alone: nothing told, both halves stay lit (#609 decision 1)', not told and dark is None,
          {'told': told, 'dark_at': dark})
    line = 'PC state to the keyboard: awake, wake (session unlocked)'
    before = len(log_lines(cfg, line))
    Path(hook).write_text('display on\nunlocked\n', encoding='utf-8')
    ok, _ = wait(lambda: len(log_lines(cfg, line)), lambda n: n > before, 5, poll=0.1)
    check(f'unlock: "{line}" (#604 back)', ok, log_lines(cfg, 'PC state to the keyboard:')[-1:])


def right_moves(check, lab, env):
    path = REPO / 'lab-reports' / f'pc-away-right-{time.time_ns()}.toml'
    path.parent.mkdir(exist_ok=True)
    path.write_text('schema = 1\n[[cases]]\nname = "right-moves"\ndescription = "the right\'s art moves after the wake"\n'
                    'steps = [\n  { capture_screen = { half = "right" } },\n'
                    '  { expect_screen_change = { half = "right", region = { x = 0, y = 20, width = 68, height = 140 }, '
                    'within_ms = 5000 } },\n]\n', encoding='utf-8')
    out = path.with_suffix('')
    code, _, err = lab_out(lab, env, 'run', '--suite', str(path), '--non-interactive', '--keep-lights', '--out', str(out),
                           timeout=120)
    try:
        case = json.loads((out / 'report.json').read_text(encoding='utf-8'))['cases'][0]
    except (OSError, ValueError, IndexError, KeyError):
        return check('P3: the right\'s picture moves after the wake', False, err[-200:], blocked=True)
    check("P3: the right's picture moves after the wake (readback changes, strip masked)",
          case['status'] == 'PASS', case['message'] or case['status'], blocked=case['status'] == 'BLOCKED')


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'))
    ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
    ap.add_argument('--edge-s', type=float, default=2.0, help='dark or lit within this after the edge')
    ap.add_argument('--dark-within-s', type=float, default=700, help='after the quit, dark within this')
    ap.add_argument('--segment-s', type=float, default=120, help='rgb-watch segments between pc-state reads')
    ap.add_argument('--history', action='store_true', help="also read the user's foxglove.log for real display edges")
    args = ap.parse_args()
    require_app_ok()
    require_user_idle(120)
    check = Checks()
    if args.history:
        log = REAL_CONFIG.parent / 'logs' / 'foxglove.log'
        lines = log.read_text(encoding='utf-8-sig', errors='replace').splitlines() if log.exists() else []
        pairs = history(lines)
        for away, back in pairs[-5:]:
            print(f'  real display edge: away {away}, back {back}', flush=True)
        check("history: the user's own nights have display-off and display-on edges", bool(pairs),
              f'{len(pairs)} in {log}', blocked=not pairs)
    cfg = temp_config('foxglove-pc-away-', users_keyboard())
    hook = cfg.parent / 'away.txt'
    hook.write_text('display on\n', encoding='utf-8')
    app = App(args.app, cfg, env={'FOXGLOVE_AWAY_OVERRIDE': str(hook), 'FOXGLOVE_APP_OK': '1'})  # the hook arms only with the lab marker
    app.__enter__()
    had = list(app.previous)
    lights = {'original': None}
    start = time.time()
    try:
        run(args, check, app, cfg, hook, lights)
    finally:
        app.stop()
        if lights['original']:  # first, while no Foxglove runs: its own short test app run
            restore_lights(args.app, args.lab, lights['original'])
        alive = app.restore()
        check("P6: the user's Foxglove runs again (started by its path)", alive == had or not had,
              alive or 'none was running before the run')
        print(f'  ran {time.time() - start:.0f} s', flush=True)
    check.finish()


def run(args, check, app, cfg, hook, lights):
    env = app.env
    ok, _ = wait(lambda: log_lines(cfg, 'away test hook: following'), bool, 15)
    check('the app follows the away test hook', ok, str(hook))
    ok, _ = wait(lambda: log_lines(cfg, CONNECT_LINE), bool, 40)
    if not check(f'PA1: on connect the app says awake ("{CONNECT_LINE}")', ok, log_lines(cfg, 'PC state')[-2:],
                 blocked=not ok):
        return
    code, out, err = lab_out(args.lab, env, 'status')
    st = status_json(out)
    pcs, shown = st.get('PC_STATE', {}), st.get('RGB_SHOWN', {})
    check('PA1: PC_STATE held by the lease (reason 0)', pcs.get('held') is True and pcs.get('reason') == 0, pcs)
    wanted = [((shown.get(h) or {}).get('wanted')) for h in ('left', 'right')]
    if wanted != [True, True]:
        original = rgb(args.lab, env)
        api = connect('test-lab-pc-away', ['devices.read', 'lighting.write'])
        for d in api.call('devices.list', {})['devices']:
            if 'lighting' in d['caps']:
                set_lights(api, {'device': d['id'], 'on': True})
        api.close()
        lights['original'] = original
        print(f'  the underglow was off by choice ({wanted}); on for the run, put back at the end', flush=True)
        time.sleep(2)
    # PA2: display off, then on.
    edge(check, args.lab, env, hook, 'display off', 'lit', False, AWAY_LINE, cfg, 'P2 (display off)', args.edge_s)
    edge(check, args.lab, env, hook, 'display on', 'lit', True, WAKE_LINE, cfg, 'P3 (display on)', args.edge_s)
    right_moves(check, args.lab, env)
    lock_edges(check, args.lab, env, hook, cfg)
    # PA3: the last word is awake.
    code, s = pc_state(args.lab, env)
    check('PA3: pc-state Awake, reason 0 before the quit', s.get('state') == 'Awake' and s.get('reason') == 0, s)
    last = log_lines(cfg, 'PC state to the keyboard:')[-1:]
    check("PA3: the app's last PC state line is awake", bool(last) and ': awake' in last[0], last)
    # PA4 (old P4): quit, no keys, the keyboard goes dark on its own.
    app.stop()
    quit_at = time.time()
    reads, dark_at = [], None
    while time.time() - quit_at < args.dark_within_s and dark_at is None:
        seg = min(args.segment_s, args.dark_within_s - (time.time() - quit_at))
        w = Watch(args.lab, env, max(seg, 5))
        w.started()
        while w.proc.poll() is None:
            dark_at = first_both(w.events, 'lit', False, quit_at)
            if dark_at:
                break
            time.sleep(0.5)
        w.stop()
        dark_at = dark_at or first_both(w.events, 'lit', False, quit_at)
        code, s = pc_state(args.lab, env)
        reads.append((round(time.time() - quit_at), s.get('held'), s.get('reason'), s.get('reason_text')))
        print(f'  +{reads[-1][0]} s after the quit: held {s.get("held")}, reason {s.get("reason")} '
              f'({s.get("reason_text")})', flush=True)
    check('P4: the lease holds the keyboard lit after the quit (first read reason 0)',
          bool(reads) and reads[0][1] is True and reads[0][2] == 0, reads[:1])
    idle, _ = user_idle()
    touched = idle is not None and idle < time.time() - quit_at
    if dark_at is None:
        check(f'P5: the keyboard goes dark within {args.dark_within_s:.0f} s of the quit', False,
              'a key or the mouse was used meanwhile' if touched else reads, blocked=touched)
    else:
        check(f'P5: both halves dark {dark_at - quit_at:.0f} s after the quit, no key pressed', True, reads[-1])
        check('P5: the hold has ended (not held, reason 4: idle)', reads[-1][1] is False or reads[-1][2] == 4,
              reads[-1])


if __name__ == '__main__':
    main()
