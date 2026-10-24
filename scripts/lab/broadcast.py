"""Board check for Broadcast Status (#211/#276) against a stand-in for OBS's WebSocket server.

No OBS is needed (and nothing can go live): obs_mock.py speaks obs-websocket v5 on 127.0.0.1:4455 with
a password generated for this run. The password goes into the plugin's secret (Credential Manager,
plugin_secret.ps1) and is deleted at the end. Foxglove runs on a temp config with the plugin
auto-installed (test hook). Checks, on the real keyboard:
- connects with the password: broadcast.connected, READY briefly, then Foxglove's panel again;
- streaming: red on both halves, a held LIVE screen (also on the screen, D7 06); the scene shows and
  follows a switch; recording too: LIVE + REC; recording only: red; paused: amber;
- everything stopped: the lights as before, READY for a few seconds, then Foxglove's panel;
- OBS quits: NOT CONNECTED briefly, lights back; OBS back: reconnected within 30 s plus margin;
- a wrong password: refused (plugin.log says so) and tried again; the right one: connected.

    python scripts/lab/broadcast.py --app <foxglove.exe>      (exit 0 PASS, 1 FAIL)
Build first: cargo build --release -p plugin-broadcast-status -p foxglove-cli -p foxglove-lab.
"""
import argparse, os, re, secrets, shutil, subprocess, time
from pathlib import Path

from labkit import App, Checks, REPO, TARGET, hue_is, restore_lights, rgb, same, temp_config, wait
from obs_mock import ObsMock

HERE = Path(__file__).resolve().parent
ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True)
ap.add_argument('--target', default=str(TARGET), help='where broadcast-status.exe, foxglove-cli and foxglove-lab are built')
args = ap.parse_args()

PLUGIN = 'org.foxglove.broadcast-status'
release = Path(args.target) / 'release'
lab, cli = release / 'foxglove-lab.exe', release / 'foxglove-cli.exe'


def secret(command, value=None):
    env = {**os.environ, 'FOXGLOVE_LAB_SECRET': value or ''}
    subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(HERE / 'plugin_secret.ps1'),
                    command, PLUGIN, 'obs-password'], env=env, check=True, capture_output=True)


check = Checks()
original = rgb(lab, {})
print('lights before:', original, flush=True)
cfg = temp_config('foxglove-broadcast-')
stage = cfg.parent / 'plugin'
stage.mkdir()
shutil.copy(REPO / 'crates/plugin-broadcast-status/plugin.toml', stage)
shutil.copy(release / 'broadcast-status.exe', stage)
dump = cfg / 'panel.pbm'
password = secrets.token_urlsafe(18)  # a test value for this run only; never printed
secret('set', password)
mock = ObsMock(password)
mock.start()
RIGHT_LAG = 10
try:
    with App(args.app, cfg, {'FOXGLOVE_DEV_UNSIGNED_PLUGINS': '1', 'FOXGLOVE_PLUGINS_AUTOINSTALL': str(stage),
                             'FOXGLOVE_DUMP_PANEL': str(dump)}, plugins=['broadcast-status']) as app:
        def facts():
            out = subprocess.run([str(cli), 'facts', 'broadcast.*'], capture_output=True, text=True, env=app.env).stdout
            return dict(re.findall(r'^broadcast\.(\w+) = (.*)$', out, re.M))

        def frame():
            try:
                return dump.read_bytes()
            except OSError:
                return b''

        def near(b, other=None):
            """Foxglove's own panel: the frame from before, give or take the clock's digits."""
            other = base if other is None else other
            return len(b) == len(other) and sum(x != y for x, y in zip(b, other)) < 0.08 * len(b)

        def on_screen():
            suite = cfg / 'readback.toml'
            suite.write_text('schema = 1\n[[cases]]\nname = "readback"\ndescription = "dump on screen"\n'
                             f'steps = [{{ panel_dump = {{ path = "{dump.as_posix()}", within_ms = 4000 }} }}]\n')
            r = subprocess.run([str(lab), '--allow-app', 'run', '--suite', str(suite), '--non-interactive', '--out',
                                str(cfg / f'rb-{time.time_ns()}')], capture_output=True, text=True, env=app.env, cwd=REPO)
            return 'PASS readback' in r.stdout

        def plugin_log():
            logs = list(cfg.rglob('plugin.log'))
            return logs[0].read_text(errors='replace') if logs else ''

        read = lambda: rgb(lab, app.env)
        ok, f = wait(facts, lambda f: f.get('connected') == 'true', 30)
        check('connects with the password from the secret store', ok, f)
        time.sleep(6)  # READY's few seconds pass
        base = frame()
        frames = {}

        mock.set(streaming=True)
        check('streaming: broadcast.streaming', *wait(facts, lambda f: f.get('streaming') == 'true', 5))
        check('streaming: red on both halves', *wait(read, hue_is(0), RIGHT_LAG))
        ok, frames['live'] = wait(frame, lambda b: b and b != base, 5)
        check('streaming: the LIVE screen replaces the panel', ok, 'frame changed')
        check('streaming: LIVE is on the screen (D7 06)', on_screen(), 'panel_dump readback')
        mock.set(scene='Gaming')
        check('scene switch: broadcast.scene follows', *wait(facts, lambda f: f.get('scene') == 'Gaming', 5))
        ok, frames['live-gaming'] = wait(frame, lambda b: b not in (base, frames['live']), 5)
        check('scene switch: the panel shows the new scene', ok, 'frame changed')
        mock.set(recording=True)
        ok, frames['both'] = wait(frame, lambda b: b not in (base, frames['live'], frames['live-gaming']), 5)
        check('streaming + recording: the LIVE + REC screen', ok and facts().get('recording') == 'true', facts())
        mock.set(streaming=False)
        ok, frames['rec'] = wait(frame, lambda b: b not in (base, frames['both'], frames['live-gaming']), 5)
        check('recording only: the REC screen', ok, 'frame changed')
        check('recording only: still red (normal lease)', *wait(read, hue_is(0), RIGHT_LAG))
        mock.set(paused=True)
        check('recording paused: amber on both halves', *wait(read, hue_is(35, 10), RIGHT_LAG))
        check('recording paused: broadcast.paused', facts().get('paused') == 'true', facts())
        mock.set(recording=False)
        check('everything stopped: the lights as before', *wait(read, lambda s: same(s, original), RIGHT_LAG))
        ok, ready = wait(frame, lambda b: b not in (frames['rec'],), 3, poll=0.1)
        check('everything stopped: READY shows', ok and not near(ready), 'frame changed')
        check("...then Foxglove's panel again", *wait(lambda: near(frame()), bool, 8))

        mock.set(recording=True)
        wait(read, hue_is(0), RIGHT_LAG)
        mock.stop()
        ok, f = wait(facts, lambda f: f.get('connected') == 'false', 5)
        check('OBS quits: broadcast.connected false', ok, f)
        check('OBS quits: the lights come back', *wait(read, lambda s: same(s, original), RIGHT_LAG))
        check('OBS quits: NOT CONNECTED briefly, then the panel', *wait(lambda: near(frame()), bool, 10))
        time.sleep(5)  # let the retry back off a little
        mock = ObsMock(password)
        mock.start()
        t = time.time()
        ok, f = wait(facts, lambda f: f.get('connected') == 'true', 45)
        check('OBS back: reconnects within the 30 s retry', ok, f'{time.time() - t:.0f} s, {f}')

        mock.stop()
        mock = ObsMock(secrets.token_urlsafe(18))  # OBS now wants a different password
        mock.start()
        ok, _ = wait(lambda: mock.log.count('auth refused'), lambda n: n >= 2, 40)
        check('wrong password: refused and tried again', ok, f"{mock.log.count('auth refused')} attempts refused")
        check('wrong password: plugin.log says so', 'refused the password' in plugin_log(), plugin_log().strip().splitlines()[-1:])
        mock.stop()
        mock = ObsMock(password)
        mock.start()
        check('right password again: connected', *wait(facts, lambda f: f.get('connected') == 'true', 45))
finally:
    mock.stop()
    secret('delete')
    restore_lights(args.app, lab, original)
check.finish()
