"""#672's three keyboard steps, window-free (#674): the Lights page follows what the keyboard shows,
read back through the local API's `lights.page` (API 1.7), the page's own view-model, next to
RGB_STATE (foxglove-lab status). No click and no glance at a window.

1. The effect is changed on the keyboard's side: `&rgb_ug RGB_EFF` (ZMK command 11) through
   foxglove-lab's INVOKE, as the key does, until RGB_STATE says Swirl (at most 4 steps). Then
   lights.page says Swirl, the rainbow under "Looks like", no swatches, Custom, hue or saturation,
   no accent switch, brightness and speed. The app must not put the old look back (#672 B): the
   keyboard still reports Swirl 5 s later.
2. RGB_EFF again to Solid: the swatches, Custom, hue, saturation and the accent switch come back,
   and speed is hidden (Solid stands still).
3. The split link is dropped once by the dev build's guard hook (D7 0B, foxglove-lab
   split_guard_test) and comes back: lights.page still matches RGB_STATE (the effect name for the
   reported number, and its controls). --skip-guard leaves it out (a GA build has no hook).

The lights are put back as they were at the end (labkit.restore_lights). It launches the build
under test on a temp FOXGLOVE_CONFIG_DIR, so it runs only with FOXGLOVE_APP_OK=1 (On-call), and
needs the keyboard (BLOCKED without it).

    FOXGLOVE_APP_OK=1 python scripts/lab/lights_page.py --app <foxglove.exe> [--lab <foxglove-lab.exe>] [--skip-guard]
    (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, json, subprocess, time
from labkit import (App, AppGone, Checks, REPO, TARGET, connect, require_app_ok, restore_lights, rgb, temp_config,
                    tool_arg, wait)

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=tool_arg('--app'), help='the Foxglove build under test')
ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
ap.add_argument('--skip-guard', action='store_true', help='leave out the split-link drop (GA build)')
args = ap.parse_args()
require_app_ok()
check = Checks()

RGB_EFF = 11  # ZMK's &rgb_ug RGB_EFF: the next effect, as the key
EFFECTS = ['Solid', 'Breathe', 'Spectrum', 'Swirl']  # ZMK's numbers (effect_controls.rs UNDERGLOW)
SWIRL, SOLID = 3, 0
RAINBOW_CONTROLS = dict(swatches=False, custom=False, hue=False, sat=False, brightness=True, speed=True, accent=False)
SOLID_CONTROLS = dict(swatches=True, custom=True, hue=True, sat=True, brightness=True, speed=False, accent=True)
STEP_S = 10  # an RGB_EFF reaches RGB_STATE well inside this; the right follows over the relay


def lab_case(name, steps, env):
    """One foxglove-lab case next to the running test app; returns (status, the steps' results)."""
    out = REPO / 'lab-reports' / f'lights-page-{name}-{time.time_ns()}'
    path = out.with_suffix('.toml')
    out.parent.mkdir(exist_ok=True)
    path.write_text(f'schema = 1\n[[cases]]\nname = "{name}"\ndescription = "{name}"\nsteps = [\n  ' +
                    ',\n  '.join(steps) + ',\n]\n', encoding='utf-8')
    # --keep-lights: the run's own save/restore put the old effect back right after each RGB_EFF
    # (#677); this script restores the user's lights itself at the end.
    subprocess.run([args.lab, '--allow-app', 'run', '--suite', str(path), '--non-interactive', '--keep-lights',
                    '--out', str(out)],
                   capture_output=True, text=True, encoding='utf-8', errors='replace', env=env, cwd=REPO, timeout=180)
    case = json.loads((out / 'report.json').read_text(encoding='utf-8'))['cases'][0]
    return case['status'], [s['actual'] for s in case['steps']]


def effect_eff(env):
    """RGB_EFF on the keyboard's side (INVOKE, as the key does)."""
    return lab_case('rgb-eff', [f'{{ invoke = {{ behavior = "rgb_ug", param1 = {RGB_EFF} }} }}'], env)


def left_effect(state):
    return state[0]['effect'] if len(state) == 2 else None


def step_to(effect, env):
    """RGB_EFF until RGB_STATE reports `effect` (at most 4 steps); (ok, last reading)."""
    s = rgb(args.lab, env)
    for _ in range(len(EFFECTS)):
        if left_effect(s) == effect:
            return True, s
        status, detail = effect_eff(env)
        if status != 'PASS':
            return False, f'RGB_EFF: {status} {detail}'
        _, s = wait(lambda: rgb(args.lab, env), lambda r: left_effect(r) != left_effect(s), STEP_S)
    return left_effect(s) == effect, s


def page(api, device):
    try:
        return api.call('lights.page', {'device': device})
    except AppGone:
        raise
    except RuntimeError as e:
        print(f'  lights.page: {e}', flush=True)
        return None


def matches(p, state):
    """lights.page agrees with RGB_STATE: the reported effect by name, and its controls."""
    e = left_effect(state)
    if p is None or e is None or e >= len(EFFECTS):
        return False
    want = RAINBOW_CONTROLS if e in (2, 3) else dict(SOLID_CONTROLS, speed=e == 1)
    return p['effect'] == EFFECTS[e] and p['rainbow'] == (e in (2, 3)) and p['controls'] == want


cfg = temp_config('foxglove-lights-page-')
original = []
with App(args.app, cfg) as app:
    api = connect('test-lab-lights-page', ['devices.read'])
    device = next((d['id'] for d in api.call('devices.list', {})['devices'] if 'lighting' in d['caps']), None)
    original = rgb(args.lab, app.env)
    print('before:', original, flush=True)
    if not device or len(original) != 2:
        check('keyboard', False, f'no lit keyboard (device {device}, RGB_STATE {original})', blocked=True)
    else:
        # 1. Swirl from the keyboard's side: the rainbow, no swatches, no accent switch.
        ok, s = step_to(SWIRL, app.env)
        if check('keyboard side: RGB_EFF reaches Swirl', ok, s):
            p = page(api, device)
            check('Swirl: lights.page shows the rainbow under "Looks like"',
                  bool(p) and p['effect'] == 'Swirl' and p['rainbow'] and p['colour_label'] == 'Looks like', p)
            check('Swirl: no swatches, Custom, hue, sat or accent switch; brightness and speed',
                  bool(p) and p['controls'] == RAINBOW_CONTROLS, p and p['controls'])
            time.sleep(5)
            s = rgb(args.lab, app.env)
            check('Swirl stays: the app puts no old look back (#672 B)', left_effect(s) == SWIRL, s)
        # 2. Solid: the swatches and the accent switch come back, speed is hidden.
        ok, s = step_to(SOLID, app.env)
        if check('keyboard side: RGB_EFF reaches Solid', ok, s):
            p = page(api, device)
            check('Solid: lights.page shows the colour row', bool(p) and p['effect'] == 'Solid'
                  and not p['rainbow'] and p['colour_label'] == 'Colour', p)
            check('Solid: swatches, Custom, hue, sat and the accent switch; no speed',
                  bool(p) and p['controls'] == SOLID_CONTROLS, p and p['controls'])
        # 3. A re-link: the page still matches what the keyboard reports.
        if args.skip_guard:
            check('re-link', False, 'left out (--skip-guard)', blocked=True)
        else:
            status, detail = lab_case('relink', ['{ split_guard_test = { within_ms = 60000 } }'], app.env)
            if status == 'BLOCKED':
                check('re-link', False, detail, blocked=True)
            elif check('lab: the right re-linked (guard hook)', status == 'PASS', detail):
                ok, s = wait(lambda: rgb(args.lab, app.env), lambda r: len(r) == 2 and r[0]['effect'] == r[1]['effect'], 15)
                p = page(api, device)
                check('after the re-link: lights.page matches RGB_STATE', ok and matches(p, s), {'page': p, 'rgb': s})
    api.close()
restore_lights(args.app, args.lab, original)
check.finish()
