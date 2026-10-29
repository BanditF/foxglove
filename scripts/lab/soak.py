"""The 30-minute soak (#618 exit criteria, before the H/I sitting resumes): the build under test runs
on a temp FOXGLOVE_CONFIG_DIR for 30 minutes while the keyboard goes through

  1. one idle -> wake (HIL resilience/right-redraw-after-wake: the left idles, a left tap wakes it,
     the right's whole-screen rewrite count must grow; HELLO bit 24),
  2. one right re-link (the dev build's split-guard hook, D7 0B: the left drops the split link
     once and the relay comes back; BLOCKED on a GA build), and
  3. one display change (the lab hook: the overlay minimized, WM_DISPLAYCHANGE posted to the app's
     display watcher; overlay.window must be back on screen),

then idles to the 30-minute mark. At the end: the app still runs, foxglove.log has no error lines
(panics, "error", "failed", "can't"; --allow takes a regex for a known-harmless line), the right's
screen readback works (a capture of the right), and its redraw count grew since the start.

Bounded: a watchdog ends everything at --cap-min (35) whatever runs; each lab step has its own
timeout. Cleanup: the test app is stopped by its own PID and the user's Foxglove (stopped by its
exact name at the start) is started again by its path. It launches the app, so it runs only with
FOXGLOVE_APP_OK=1 (On-call). The PC must stay unlocked and the keyboard untouched while it runs.

    FOXGLOVE_APP_OK=1 python scripts/lab/soak.py --app <foxglove.exe>   (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, json, os, re, subprocess, threading, time
from labkit import App, Checks, REPO, TARGET, Windows, connect, require_app_ok, temp_config, wait

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, help='the Foxglove build under test')
ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'))
ap.add_argument('--minutes', type=float, default=30, help='how long the soak runs')
ap.add_argument('--cap-min', type=float, default=35, help='hard stop, whatever runs')
ap.add_argument('--allow', action='append', default=[], help='regex of a log line that is not an error')
args = ap.parse_args()
require_app_ok()
if args.minutes >= args.cap_min:
    raise SystemExit('--minutes must be under --cap-min')
check = Checks()
start = time.time()
out = REPO / 'lab-reports' / time.strftime('soak-%Y%m%dT%H%M%SZ', time.gmtime())
out.mkdir(parents=True, exist_ok=True)
ERROR = re.compile(r"panick|\berror\b|\bfailed\b|can't|cannot", re.I)


def left():
    return max(0.0, start + args.cap_min * 60 - time.time())


def lab(*a, env=None, timeout=180):
    """One foxglove-lab command next to the test app, within the cap."""
    return subprocess.run([args.lab, '--allow-app', *a], capture_output=True, text=True, env=env, cwd=REPO,
                          timeout=min(timeout, max(1, left() - 30)))


def suite(name, steps, env):
    path = out / f'{name}.toml'
    path.write_text(f'schema = 1\n[[cases]]\nname = "{name}"\ndescription = "{name}"\nsteps = [\n  ' +
                    ',\n  '.join(steps) + ',\n]\n')
    try:
        lab('run', '--suite', str(path), '--non-interactive', '--out', str(out / name), env=env)
        case = json.loads((out / name / 'report.json').read_text())['cases'][0]
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError, IndexError) as e:
        return 'FAIL', [f'{name}: {e}']
    return case['status'], [s['actual'] for s in case['steps']]


def redraws(env):
    """The right's whole-screen rewrites as `foxglove-lab status` reads them, or None."""
    try:
        text = lab('status', env=env, timeout=60).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    m = re.search(r'^RIGHT_REDRAWS: (\d+)', text, re.M)
    return int(m.group(1)) if m else None


cfg = temp_config('foxglove-soak-')
with App(args.app, cfg) as app:
    # The hard stop: the test app by its own PID, then out (App's cleanup can't run after this,
    # so the user's Foxglove is started here too).
    def hard_stop():
        print(f'SOAK: hard stop at {args.cap_min} min', flush=True)
        app.proc.kill()
        for p in app.previous:
            subprocess.Popen([p])
        print('RESULT FAIL', flush=True)
        os._exit(1)
    watchdog = threading.Timer(args.cap_min * 60, hard_stop)
    watchdog.daemon = True
    watchdog.start()
    try:
        api = connect('test-lab-soak', ['devices.read', 'app.windows'])
        r0 = redraws(app.env)
        check('start: the right reports its redraw count', r0 is not None, r0, blocked=r0 is None)

        status, detail = 'BLOCKED', []
        try:
            r = lab('run', '--suite', 'tests/hil/resilience.toml', '--only', 'right-redraw-after-wake',
                    '--non-interactive', '--out', str(out / 'idle-wake'), env=app.env, timeout=240)
            case = next(c for c in json.loads((out / 'idle-wake' / 'report.json').read_text())['cases']
                        if c['name'] == 'right-redraw-after-wake')
            status, detail = case['status'], [s['actual'] for s in case['steps']][-2:]
        except (OSError, StopIteration, subprocess.TimeoutExpired, json.JSONDecodeError) as e:
            detail = [str(e)]
        check('1. idle -> wake: the right rewrote its panel', status == 'PASS', detail, blocked=status == 'BLOCKED')

        status, detail = suite('relink', ['{ split_guard_test = { within_ms = 60000 } }'], app.env)
        check('2. right re-link (guard hook) and the relay back', status == 'PASS', detail,
              blocked=status == 'BLOCKED')

        win = Windows(app.proc.pid)
        o = None
        try:
            o = api.call('overlay.window', {})
            hwnd = win.overlay(o['rect'])
            win.minimize(hwnd)
            win.display_change()
            ok, o = wait(lambda: api.call('overlay.window', {}), lambda o: not o['iconic'] and o['on_screen'], 10)
        except RuntimeError as e:
            ok, o = False, str(e)
        check('3. display change (lab hook): the overlay is back on screen', ok, o)

        remaining = start + args.minutes * 60 - time.time()
        print(f'idling {remaining / 60:.1f} more min to the {args.minutes:.0f}-minute mark', flush=True)
        time.sleep(max(0, remaining))  # the soak itself: one bounded wait, nothing polled

        check('end: the app still runs', app.proc.poll() is None, f'exit {app.proc.poll()}')
        log = cfg / 'logs' / 'foxglove.log'
        lines = log.read_text(encoding='utf-8', errors='replace').splitlines() if log.exists() else []
        allowed = [re.compile(a) for a in args.allow]
        errors = [l for l in lines if ERROR.search(l) and not any(a.search(l) for a in allowed)]
        (out / 'error-lines.txt').write_text('\n'.join(errors), encoding='utf-8')
        check('end: no error lines in foxglove.log', bool(lines) and not errors,
              f'{len(errors)} of {len(lines)} lines (see {out / "error-lines.txt"})' + ''.join('\n    ' + l for l in errors[:10]))
        status, detail = suite('readback', ['{ capture_screen = { half = "right" } }'], app.env)
        check("end: the right's screen readback works", status == 'PASS', detail, blocked=status == 'BLOCKED')
        r1 = redraws(app.env)
        check("end: the right's redraw count grew (the wake and the re-link rewrote it)",
              r0 is not None and r1 is not None and r1 > r0, f'{r0} -> {r1}', blocked=r0 is None or r1 is None)
        api.close()
    finally:
        watchdog.cancel()
        (out / 'summary.json').write_text(json.dumps({'results': check.results, 'minutes': (time.time() - start) / 60}),
                                          encoding='utf-8')
print(f'report folder: {out}', flush=True)
check.finish()
