"""Keyboard-first rows that need the app or a fresh boot (docs/resilience.md, #25); the rest are
tests/hil/resilience.toml.

  --row 2a   A test instance of Foxglove (temp config) drives the left panel; it is killed by its own
             PID (the user's Foxglove is stopped first and started again after, never killed mid-test).
             The last frame it sent must stay on the screen 10 s and 5 minutes later, and keys work.
  --row 1    Right after a flash or power cycle, before any Foxglove has run: the right link is up,
             the right shows a built-in set (an art select is allowed), and keys work on both halves.

Typing through the sink needs an unlocked PC (BLOCKED otherwise); the layer-key check runs either way.

    python scripts/lab/resilience.py --row 2a --app <foxglove.exe>      (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, json, subprocess, time
from labkit import App, tool_arg, Checks, REPO, TARGET, locked, temp_config

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--row', required=True, choices=['1', '2a'])
ap.add_argument('--app', type=tool_arg('--app'), help='the Foxglove build for row 2a')
ap.add_argument('--hold-s', type=int, default=300, help='row 2a: how long the frame must stay')
ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
args = ap.parse_args()
check = Checks()


def suite(name, steps, env=None):
    """Runs one case through foxglove-lab; returns (status, the steps' results)."""
    out = REPO / 'lab-reports' / f'resilience-{name}-{time.time_ns()}'
    path = out.with_suffix('.toml')
    out.parent.mkdir(exist_ok=True)
    path.write_text(f'schema = 1\n[[cases]]\nname = "{name}"\ndescription = "{name}"\nsteps = [\n  ' +
                    ',\n  '.join(steps) + ',\n]\n')
    subprocess.run([args.lab, '--allow-app', 'run', '--suite', str(path), '--non-interactive', '--out', str(out)],
                   capture_output=True, text=True, env=env, cwd=REPO)
    case = json.loads((out / 'report.json').read_text(encoding='utf-8'))['cases'][0]
    return case['status'], [s['actual'] for s in case['steps']]


LAYER_KEYS = ['{ press_position = { pos = 43, state = "down" } }', '{ expect_layer = { active_contains = [1] } }',
              '{ press_position = { pos = 43, state = "up" } }', '{ expect_layer = { top = 0 } }',
              '{ press_position = { pos = 46, state = "down" } }', '{ expect_layer = { active_contains = [2] } }',
              '{ press_position = { pos = 46, state = "up" } }', '{ expect_layer = { top = 0 } }']
# Q and F on the left, P and J on the right (over the split link), as positions.toml types them.
TYPING = ['{ type_into_sink = { text = "qpfj", expect = "qpfj", via = [' + ', '.join(
    f'{{ press_position = {{ pos = {p}, state = "tap" }} }}' for p in (1, 11, 17, 23)) + '] } }']


def typing():
    if locked():
        return check('typing on both halves', False, 'the PC is locked: the typing sink needs it unlocked', blocked=True)
    status, detail = suite('typing', TYPING)
    check('typing on both halves', status == 'PASS', detail)


if args.row == '2a':
    cfg = temp_config('foxglove-resilience-')
    dump = cfg / 'panel.pbm'
    with App(args.app, cfg, {'FOXGLOVE_DUMP_PANEL': str(dump)}) as app:
        time.sleep(15)  # the app takes the panel and streams it
        status, detail = suite('app-drives-panel', [f'{{ panel_dump = {{ path = "{dump.as_posix()}" }} }}'], app.env)
        check('the app drives the left panel', status == 'PASS', detail)
        app.proc.kill()  # its own PID only: "End task", mid-stream
        app.proc.wait(10)
        last = dump.read_bytes()
        frozen = cfg / 'last.pbm'
        frozen.write_bytes(last)
        time.sleep(10)
        status, detail = suite('kept-10s', [f'{{ panel_dump = {{ path = "{frozen.as_posix()}" }} }}'])
        check('app killed: the last frame stays (10 s)', status == 'PASS', detail)
        status, detail = suite('keys', LAYER_KEYS)
        check('app killed: keys work on both halves', status == 'PASS', detail[-1:])
        time.sleep(max(0, args.hold_s - 30))
        status, detail = suite('kept-long', [f'{{ panel_dump = {{ path = "{frozen.as_posix()}" }} }}'])
        check(f'app killed: the last frame stays ({args.hold_s // 60} min), no WPM', status == 'PASS', detail)
        typing()
        # Leave the screen as the firmware would have it: give the panel back.
        suite('release', ['{ panel_release = {} }'])
else:
    # art_select refuses while pushed art shows, so a select that works means a built-in set.
    status, detail = suite('fresh-boot', ['{ art_select = { set = 0, restore = true } }'] + LAYER_KEYS)
    check('fresh boot: the right shows a built-in set (nothing pushed), keys work on both halves',
          status == 'PASS', detail)
    typing()
check.finish()
