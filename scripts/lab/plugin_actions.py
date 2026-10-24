"""Board check for key-triggered plugin actions (#212): a key on the keyboard pauses the focus timer.

Starts Foxglove on a temp FOXGLOVE_CONFIG_DIR with the focus timer auto-installed (test hook) and its
pause action bound to F13 in foxglove.toml [plugins.bindings]. The keyboard then taps F13
(tests/hil/app-actions.toml, D7 02 &kp F13), and foxglove-cli reads the timer's facts: focus.paused
turns true and focus.remaining stops; a second tap resumes it. The user's real settings are never
touched. Whatever Foxglove was running is stopped first and started again after (by path).

    python scripts/lab/plugin_actions.py --app <foxglove.exe>
Build first: scripts/pack-plugin.ps1 plugin-focus-timer, cargo build --release -p foxglove-cli
-p foxglove-lab. Exit code 0 on PASS, 1 on FAIL, 2 when BLOCKED (the PC is locked: Windows then
delivers no hotkeys at all, so the case needs an unlocked session).
"""
import argparse, ctypes, os, re, shutil, subprocess, sys, tempfile, time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', REPO / 'target'))
PLUGIN = 'org.foxglove.focus-timer'
STOP_FOXGLOVE = ("Get-Process | ? { $_.ProcessName -like 'foxglove*' -and $_.ProcessName -notin "
                 "'foxglove-lab','foxglove-cli' } | % { $_.Path; Stop-Process -Id $_.Id -Confirm:$false }")

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=Path, help='the Foxglove build under test')
ap.add_argument('--plugin', type=Path, default=TARGET / 'plugins' / PLUGIN, help='staged plugin folder')
ap.add_argument('--cli', type=Path, default=TARGET / 'release' / 'foxglove-cli.exe')
ap.add_argument('--lab', type=Path, default=TARGET / 'release' / 'foxglove-lab.exe')
args = ap.parse_args()
for p in (args.app, args.plugin / 'plugin.toml', args.cli, args.lab):
    if not p.exists():
        sys.exit(f'missing {p}')

# A locked PC sends keys to the secure desktop, and no global hotkey fires.
if not ctypes.windll.user32.OpenInputDesktop(0, False, 0x0100):
    print('BLOCKED: the PC is locked (no input desktop), so Windows delivers no hotkeys; run it unlocked')
    sys.exit(2)

root = Path(tempfile.mkdtemp(prefix='foxglove-actions-'))
cfg = root / 'config'
cfg.mkdir()
for k in (Path(os.environ['APPDATA']) / 'Foxglove' / 'config').glob('keymap-*.studio'):
    shutil.copy(k, cfg)  # so the runner can resolve &kp
(cfg / 'foxglove.toml').write_text(
    f'schema = 1\n\n[plugins.bindings.F13]\nplugin = "{PLUGIN}"\naction = "pause"\n', encoding='utf-8')
env = {**os.environ, 'FOXGLOVE_CONFIG_DIR': str(cfg)}

results = []
def check(name, ok, detail):
    results.append(ok)
    print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}", flush=True)


def facts():
    out = subprocess.run([str(args.cli), 'facts', 'focus.*'], capture_output=True, text=True, env=env).stdout
    return dict(re.findall(r'^(focus\.\w+) = (.*)$', out, re.M))


def wait_facts(pred, seconds):
    end = time.time() + seconds
    while True:
        f = facts()
        if pred(f) or time.time() > end:
            return pred(f), f
        time.sleep(0.3)


def remaining(f):
    return int(f['focus.remaining']) if f.get('focus.remaining', '').lstrip('-').isdigit() else None


def counting(seconds=3.5):
    """True when focus.remaining goes down over `seconds`, False when it holds still."""
    a = remaining(facts())
    time.sleep(seconds)
    b = remaining(facts())
    return a is not None and b is not None and b < a, (a, b)


def tap_f13():
    r = subprocess.run([str(args.lab), '--allow-app', 'run', '--suite', str(REPO / 'tests/hil/app-actions.toml'),
                        '--non-interactive', '--out', str(root / f'lab-{time.time_ns()}')],
                       capture_output=True, text=True, env=env, cwd=REPO)
    return 'PASS f13-tap' in r.stdout, (r.stdout + r.stderr).strip().splitlines()[-1:]


def kill_plugin():  # a hard-stopped app leaves its plugin running until its next ping fails
    subprocess.run(['powershell', '-NoProfile', '-Command',
                    'Get-Process focus-timer -ErrorAction SilentlyContinue | Stop-Process -Force'], capture_output=True)


previous = subprocess.run(['powershell', '-NoProfile', '-Command', STOP_FOXGLOVE],
                          capture_output=True, text=True).stdout.split()
time.sleep(2)
kill_plugin()
app = subprocess.Popen([str(args.app)], env={**env, 'FOXGLOVE_DEV_UNSIGNED_PLUGINS': '1',
                                            'FOXGLOVE_PLUGINS_AUTOINSTALL': str(args.plugin)})
try:
    ok, f = wait_facts(lambda f: remaining(f) is not None, 30)
    check('focus timer installed and publishing', ok and f.get('focus.paused') == 'false', f)
    ok, ab = counting()
    check('timer counts down before the key', ok, f'focus.remaining {ab}')
    ok, out = tap_f13()
    check('keyboard taps F13 (D7 02 &kp F13)', ok, out)
    paused, f = wait_facts(lambda f: f.get('focus.paused') == 'true', 3)
    check('F13 pauses the timer', paused, f)
    ok, ab = counting()
    check('paused timer holds still', not ok and ab[0] is not None and ab[0] == ab[1], f'focus.remaining {ab}')
    ok, out = tap_f13()
    ok2, f = wait_facts(lambda f: f.get('focus.paused') == 'false', 3)
    check('a second F13 resumes it', paused and ok and ok2, f)
    ok, ab = counting()
    check('resumed timer counts down', ok, f'focus.remaining {ab}')
finally:
    app.terminate()
    try:
        app.wait(10)
    except subprocess.TimeoutExpired:
        app.kill()
    kill_plugin()
    for p in previous:
        subprocess.Popen([p])
    print('restored:', previous, flush=True)
print('RESULT', 'PASS' if results and all(results) else 'FAIL')
sys.exit(0 if results and all(results) else 1)
