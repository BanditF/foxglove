"""Board check for the Mic mute plugin (#210/#264) on this PC's default capture device.

The microphone's mute and volume are recorded first (straight from Core Audio, coreaudio.ps1) and put
back exactly in a `finally`, whatever happens: nothing stays muted. Foxglove runs on a temp config with
the plugin auto-installed (test hook) and `toggle`/`mute`/`unmute` bound to F15/F16/F17.
- Mute from outside (Core Audio): mic.state "muted", red high-priority underglow on both halves,
  the bright MUTED screen on the left panel (and on the screen, D7 06 readback).
- Unmute: mic.state "unmuted", the lights go back, LIVE shows and then the panel returns.
- Keys from the keyboard (D7 02 &kp F15-F17): toggle mutes and unmutes, mute and unmute do just
  that; checked in Core Audio itself. Needs an unlocked PC (BLOCKED otherwise).

    python scripts/lab/mic_mute.py --app <foxglove.exe>      (exit 0 PASS, 1 FAIL, 2 BLOCKED)
Build first: cargo build --release -p plugin-mic-mute -p foxglove-cli -p foxglove-lab.
"""
import argparse, json, re, shutil, subprocess, time
from pathlib import Path
from labkit import App, tool_arg, Checks, REPO, TARGET, hue_is, locked, restore_lights, rgb, same, temp_config, wait

PLUGIN = 'org.foxglove.mic-mute'
F15, F16, F17 = 0x7006A, 0x7006B, 0x7006C
ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=tool_arg('--app'))
ap.add_argument('--target', default=str(TARGET), type=tool_arg('--target', 'dir'), help='where mic-mute.exe, foxglove-cli and foxglove-lab are built')
args = ap.parse_args()
release = Path(args.target) / 'release'
lab, cli = release / 'foxglove-lab.exe', release / 'foxglove-cli.exe'
COREAUDIO = Path(__file__).with_name('coreaudio.ps1')


def audio(*a):
    r = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(COREAUDIO), *a],
                       capture_output=True, text=True)
    if r.returncode:
        raise RuntimeError(f'coreaudio {a}: {r.stderr.strip()}')
    return json.loads(r.stdout) if a[0] == 'get' else None


def comms():
    return next(e for e in audio('get') if e['role'] == 'communications')


def set_mute(muted):
    e = comms()
    audio('set', 'communications', '1' if muted else '0', str(e['volume']))


check = Checks()
original_audio = audio('get')
print('microphone before:', original_audio, flush=True)
if not comms().get('id'):
    check('microphone', False, 'no default capture device on this PC: needs a human', blocked=True)
    check.finish()
original_lights = rgb(lab, {})
print('lights before:', original_lights, flush=True)

cfg = temp_config('foxglove-micmute-', ''.join(f'''
[plugins.bindings.{key}]
plugin = "{PLUGIN}"
action = "{action}"
''' for key, action in (('F15', 'toggle'), ('F16', 'mute'), ('F17', 'unmute'))))
stage = cfg.parent / 'plugin'
stage.mkdir()
shutil.copy(REPO / 'crates/plugin-mic-mute/plugin.toml', stage)
shutil.copy(release / 'mic-mute.exe', stage)
dump = cfg / 'panel.pbm'
RIGHT_LAG = 10


def ink():
    """The share of lit pixels in the frame the app last sent to the left panel."""
    raw = dump.read_bytes()
    bits = raw[raw.index(b'\n', raw.index(b'\n') + 1) + 1:]
    return sum(bin(b).count('1') for b in bits) / (len(bits) * 8)


try:
    with App(args.app, cfg, {'FOXGLOVE_DEV_UNSIGNED_PLUGINS': '1', 'FOXGLOVE_PLUGINS_AUTOINSTALL': str(stage),
                             'FOXGLOVE_DUMP_PANEL': str(dump)}, plugins=['mic-mute']) as app:
        def facts():
            out = subprocess.run([str(cli), 'facts', 'mic.*'], capture_output=True, text=True, env=app.env).stdout
            return dict(re.findall(r'^(mic\.\w+) = (.*)$', out, re.M))

        def lab_run(name, step):
            suite = cfg / f'{name}.toml'
            suite.write_text(f'schema = 1\n[[cases]]\nname = "{name}"\ndescription = "{name}"\nsteps = [{step}]\n')
            r = subprocess.run([str(lab), '--allow-app', 'run', '--suite', str(suite), '--non-interactive', '--out',
                                str(cfg / f'{name}-{time.time_ns()}')], capture_output=True, text=True, env=app.env, cwd=REPO)
            return f'PASS {name}' in r.stdout

        on_screen = lambda: lab_run('readback', f'{{ panel_dump = {{ path = "{dump.as_posix()}", within_ms = 4000 }} }}')
        tap = lambda usage: lab_run('tap', f'{{ invoke = {{ behavior = "kp", param1 = {usage}, hold_ms = 30 }} }}')
        read = lambda: rgb(lab, app.env)
        muted_in_windows = lambda want: wait(comms, lambda e: e['muted'] == want, 5)

        ok, f = wait(facts, lambda f: f.get('mic.state') == 'unmuted', 30)
        check('mic.state unmuted at start', ok, f)
        time.sleep(6)  # LIVE from the start has gone
        base = ink()

        set_mute(True)
        check('Core Audio mute: mic.state muted', *wait(facts, lambda f: f.get('mic.state') == 'muted', 5))
        check('muted: red high-priority underglow on both halves', *wait(read, hue_is(0), RIGHT_LAG))
        ok, level = wait(ink, lambda v: v > 0.5, 5)
        check('muted: the bright MUTED screen on the left panel', ok, f'{level:.0%} lit (before {base:.0%})')
        check('muted: the MUTED frame is on the screen (D7 06)', on_screen(), 'panel_dump readback')

        set_mute(False)
        check('Core Audio unmute: mic.state unmuted', *wait(facts, lambda f: f.get('mic.state') == 'unmuted', 5))
        ok, level = wait(ink, lambda v: v < 0.5, 3, poll=0.1)
        check('unmuted: MUTED leaves the panel (LIVE)', ok, f'{level:.0%} lit')
        check('unmuted: the lights go back', *wait(read, lambda s: same(s, original_lights), RIGHT_LAG))
        time.sleep(5)
        check('LIVE shows for a few seconds, then the panel is back', abs(ink() - base) < 0.1, f'{ink():.0%} lit, {base:.0%} before')

        if locked():
            check('keys (toggle, mute, unmute)', False, 'the PC is locked, so Windows delivers no hotkeys', blocked=True)
        else:
            for name, usage, want in (('toggle mutes', F15, True), ('toggle unmutes', F15, False),
                                      ('mute key mutes', F16, True), ('unmute key unmutes', F17, False)):
                tapped = tap(usage)
                ok, e = muted_in_windows(want)
                check(f'{name} (Core Audio)', tapped and ok, e)
                time.sleep(1)
finally:
    for e in original_audio:  # exactly as it was, even after a failure
        if e.get('id'):
            audio('set', e['role'], '1' if e['muted'] else '0', str(e['volume']))
    now = audio('get')
    back = [(e['muted'], e['volume']) for e in now] == [(e['muted'], e['volume']) for e in original_audio]
    print('microphone restored:' if back else 'MICROPHONE NOT RESTORED:', now, flush=True)
    restore_lights(args.app, lab, original_lights)
check('microphone left exactly as it was', back, now)
check.finish()
