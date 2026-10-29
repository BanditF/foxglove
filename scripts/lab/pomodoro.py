"""Board check for the Pomodoro plugin (#209/#255) on the real keyboard.

Foxglove runs on a temp config with the focus timer auto-installed (test hook), `pause` bound to F13
and `reset` to F14. Checks:
- facts: focus.state "focus", focus.remaining counting down;
- left panel: the countdown frame changes every second in FOXGLOVE_DUMP_PANEL, and the frame the
  app sent is on the screen (D7 06 readback, tests/hil panel_dump);
- underglow: the focus lease shows red on both halves;
- F13 from the keyboard (D7 02 &kp F13, tests/hil/app-actions.toml) pauses: focus.paused, the
  lease is given back (lights as before), the panel stops ticking; F13 again resumes;
- F14 starts a new focus period (focus.remaining back near 25:00);
- a crash (focus-timer.exe killed): Foxglove restarts it, the countdown carries on from its
  deadline, and the lease is taken again. (A lease ending with its connection is leases.py's case.)
The lights are put back through the app's API at the end (a hard-stopped app can't give a lease back).
The key steps need an unlocked PC (BLOCKED otherwise; the rest still runs).

    python scripts/lab/pomodoro.py --app <foxglove.exe>      (exit 0 PASS, 1 FAIL, 2 BLOCKED)
Build first: cargo build --release -p plugin-focus-timer -p foxglove-cli -p foxglove-lab.
"""
import argparse, re, shutil, subprocess, time
from pathlib import Path
from labkit import App, tool_arg, Checks, REPO, TARGET, hue_is, kill, locked, ps, restore_lights, rgb, same, temp_config, wait

PLUGIN = 'org.foxglove.focus-timer'
ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=tool_arg('--app'))
ap.add_argument('--target', default=str(TARGET), type=tool_arg('--target', 'dir'), help='where focus-timer.exe, foxglove-cli and foxglove-lab are built')
args = ap.parse_args()
release = Path(args.target) / 'release'
lab, cli = release / 'foxglove-lab.exe', release / 'foxglove-cli.exe'

cfg = temp_config('foxglove-pomodoro-', f'''
[plugins.bindings.F13]
plugin = "{PLUGIN}"
action = "pause"

[plugins.bindings.F14]
plugin = "{PLUGIN}"
action = "reset"
''')
stage = cfg.parent / 'plugin'
stage.mkdir()
shutil.copy(REPO / 'crates/plugin-focus-timer/plugin.toml', stage)
shutil.copy(release / 'focus-timer.exe', stage)
dump = cfg / 'panel.pbm'
check = Checks()
RIGHT_LAG = 10
original = rgb(lab, {**__import__('os').environ})
print('lights before:', original, flush=True)

with App(args.app, cfg, {'FOXGLOVE_DEV_UNSIGNED_PLUGINS': '1', 'FOXGLOVE_PLUGINS_AUTOINSTALL': str(stage),
                         'FOXGLOVE_DUMP_PANEL': str(dump)}, plugins=['focus-timer']) as app:
    def facts():
        out = subprocess.run([str(cli), 'facts', 'focus.*'], capture_output=True, text=True, env=app.env).stdout
        return dict(re.findall(r'^(focus\.\w+) = (.*)$', out, re.M))

    def remaining(f=None):
        v = (f or facts()).get('focus.remaining', '')
        return int(v) if v.isdigit() else None

    def frames(seconds):
        """Distinct panel frames the app sent over `seconds`."""
        seen, end = set(), time.time() + seconds
        while time.time() < end:
            try:
                seen.add(dump.read_bytes())
            except OSError:
                pass
            time.sleep(0.1)
        return len(seen)

    def on_screen():
        suite = cfg / 'readback.toml'
        suite.write_text('schema = 1\n[[cases]]\nname = "readback"\ndescription = "dump on screen"\n'
                         f'steps = [{{ panel_dump = {{ path = "{dump.as_posix()}", within_ms = 4000 }} }}]\n')
        r = subprocess.run([str(lab), '--allow-app', 'run', '--suite', str(suite), '--non-interactive', '--out',
                            str(cfg / f'rb-{time.time_ns()}')], capture_output=True, text=True, env=app.env, cwd=REPO)
        return 'PASS readback' in r.stdout

    def tap(key_usage):
        suite = cfg / 'tap.toml'
        suite.write_text('schema = 1\n[[cases]]\nname = "tap"\ndescription = "tap an F13-F24 key"\n'
                         f'steps = [{{ invoke = {{ behavior = "kp", param1 = {key_usage}, hold_ms = 30 }} }}]\n')
        r = subprocess.run([str(lab), '--allow-app', 'run', '--suite', str(suite), '--non-interactive', '--out',
                            str(cfg / f'tap-{time.time_ns()}')], capture_output=True, text=True, env=app.env, cwd=REPO)
        return 'PASS tap' in r.stdout

    read = lambda: rgb(lab, app.env)
    ok, f = wait(facts, lambda f: remaining(f) is not None, 30)
    check('Pomodoro installed and publishing', ok and f.get('focus.state') == 'focus', f)
    a = remaining(); time.sleep(3); b = remaining()
    check('focus.remaining counts down', a is not None and b is not None and b < a, (a, b))
    n = frames(4)
    check('left panel ticks every second', n >= 3, f'{n} distinct frames in 4 s')
    check('the countdown frame is on the left screen (D7 06)', on_screen(), 'panel_dump readback')
    check('focus lease shows red on both halves', *wait(read, hue_is(0), RIGHT_LAG))

    if locked():
        check('keys (F13 pause, F14 reset)', False, 'the PC is locked, so Windows delivers no hotkeys', blocked=True)
    else:
        check('keyboard taps F13', tap(0x70068), 'D7 02 &kp F13')
        ok, f = wait(facts, lambda f: f.get('focus.paused') == 'true', 3)
        check('F13 pauses', ok, f)
        check('paused: the lease is given back (lights as before the plugin)',
              *wait(read, lambda s: same(s, original), RIGHT_LAG))
        a = remaining(); n = frames(3); b = remaining()
        check('paused: time and panel hold still', a == b and n <= 1, f'remaining {a} -> {b}, {n} frames in 3 s')
        tap(0x70068)
        ok, f = wait(facts, lambda f: f.get('focus.paused') == 'false', 3)
        check('F13 again resumes, red again', ok and wait(read, hue_is(0), RIGHT_LAG)[0], f)
        time.sleep(3)
        check('keyboard taps F14', tap(0x70069), 'D7 02 &kp F14')
        ok, f = wait(facts, lambda f: (remaining(f) or 0) >= 25 * 60 - 3, 3)
        check('F14 starts a new focus period', ok, f)

    pids = lambda: ps('(Get-Process focus-timer -ErrorAction SilentlyContinue).Id').split()
    crashed, left = pids(), remaining()
    kill('focus-timer')
    ok, now = wait(pids, lambda p: p and p != crashed, 15)
    check('crash: Foxglove restarts the plugin', ok, f'{crashed} -> {now}')
    ok, f = wait(facts, lambda f: remaining(f) is not None and remaining(f) < (left or 0), 15)
    check('crash: the countdown carries on from its deadline (no reset)', ok, f'{left} before -> {f}')
    check('crash: the restarted plugin leases red again', *wait(read, hue_is(0), RIGHT_LAG))

restore_lights(args.app, lab, original)
check.finish()
