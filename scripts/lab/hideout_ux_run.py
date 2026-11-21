r"""Hideout UX run: the first-run flow on a hidden desktop, with a fixture app that says hello.

Seeds a scratch lab folder (a Found fixture app "Vulpes" that registers as `adopted`, and a copy of
hideout-ac-probe.exe as its program: it connects to the hub the way every app does, says
`initialize` and `module.ready`, and waits up to 60 s), starts the lab hub on its own `ql-*`
desktop through quiet-launch.ps1, and checks:

  A2/A3  Found card and its "Add <name>" button; the tray line says "No apps yet"; nothing reads "0 apps"
  A4     the dialog opens with Not now; window close = Not now (nothing starts, the dialog is gone)
  F3     tray model pick "Open Hideout" shows the main window (no cursor, no open menu)
  D2     the Allow label moves with the ticks ("Allow and start" only after "I understand")
  A5     Allow: the app says hello; tray line "1 app is running"; the row "Vulpes is running." is the Done card (Hide, Later, if you want)
  A6/F4  a real UI Automation read of the main window (uia_tree.ps1 -ProcessName hideout): every
         focusable node has a name; the window title is "Hideout (lab) · <page>"
  D1     the dialog opens with Not now focused
  C3/C4  Remove from Hideout: the row's menu, the question names the app, Cancel is focused and keeps it,
         Remove drops the row and the tray line, and the app stops

    python scripts/lab/hideout_ux_run.py --app <hideout.exe> --probe <hideout-ac-probe.exe> --lab <scratch folder>

  --only s1  (needs --sensors <sensors.exe>): the PC sensors flow, as the demo does it. The run copies the hub's
         exe to <lab>\hub (its config dir is <lab>\cfg), places sensors.exe and the repo's crates/sensors/module.toml in <lab>\hub\apps\sensors
         (what scripts/ops/demo-up.ps1 -SensorsExe does next to a build), and checks Found card, Add, the dialog's
         words, Allow, the app running, readings on its page (Temperature never reads 0), and a changed file
         offered as "Update in Hideout".

Needs FOXGLOVE_APP_OK=1 (On-call). Hidden desktop only: nothing is ever started on the user's. The lab
folder must not be the user's hub folder. Run scripts/ops/game-check.ps1 first.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from hideout_lab_checks import (  # noqa: E402
    POWERSHELL, QUIET, QuietHub, contains, has, ps_quote, texts, tray_status)
from labkit import Checks, require_app_ok, tool_arg  # noqa: E402

UIA = Path(__file__).resolve().parent / 'uia_tree.ps1'
MODULE_ID, NAME = 'vulpes', 'Vulpes'
PROBE_VERSION = '0.1.0'  # what hideout-ac-probe says in module.ready; a different record version is trust.mismatch
FOCUSABLE = {'Button', 'CheckBox', 'RadioButton', 'Edit', 'ComboBox', 'Hyperlink', 'ListItem', 'MenuItem', 'TabItem',
             'Slider'}


# ---- fixtures (pure) -----------------------------------------------------------------------------------

def write_text(path, text):
    """UTF-8 without a BOM: the hub's seed readers refuse a BOM."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(text.encode('utf-8'))


def toml_str(text):
    return '"' + str(text).replace('\\', '\\\\').replace('"', '\\"') + '"'


def hello_fixture(lab, probe):
    """Seeds `<lab>\\fx` (the app folder, its module.toml template with @EXE@ and @VERSION@, and the program
    that says hello) and `<lab>\\lab-discover.json` (the fake registry record with App, Exe, Version and
    Manifest, G1) so the Found card offers "Add Vulpes".
    The product id is `vulpes`, kind `adopted`, family `vulpes`. Returns the program's path."""
    lab = Path(lab).resolve()
    fx = lab / 'fx'
    exe = fx / 'venv' / 'Scripts' / 'arete.exe'  # the product's image name (products.json)
    exe.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(probe, exe)
    manifest = fx / 'hideout' / 'module.toml'
    write_text(manifest, '\n'.join([
        f'id = {toml_str(MODULE_ID)}', f'name = {toml_str(NAME)}', 'version = "@VERSION@"', 'kind = "adopted"',
        'family = "vulpes"', 'hub = "1.0"', 'description = "Lab fixture that says hello."',
        f'run = ["@EXE@", {toml_str(MODULE_ID)}]',
        'permissions = ["audio.inputs.read", "devices.read"]', '']))
    write_text(lab / 'lab-discover.json', json.dumps({
        'registry': {'Software\\Vulpes\\Installer': {'Version': PROBE_VERSION, 'App': str(fx), 'Exe': str(exe),
                                                       'Manifest': str(manifest)}},
        'env': {'LOCALAPPDATA': str(lab / 'local')}}, indent=2))
    return exe


def allow_label(state):
    """The dialog's Allow button text ("Allow", "Allow 2", "Allow and start", "Allow none")."""
    return next((t for t in texts(state) if re.fullmatch(r'Allow( and start| none| \d+)?', t)), None)


def dialog_open(state):
    """The permission dialog is up (the Found card has a Not now too, so key on the dialog's title)."""
    return any(t.startswith('Let ') and t.endswith(' run in Hideout?') for t in texts(state))


def unnamed_focusables(tree):
    """Roles of focusable UIA nodes that carry no name (the screen-reader rubric, item 4)."""
    out, stack = [], [tree]
    while stack:
        n = stack.pop()
        if n.get('role') in FOCUSABLE and not (n.get('name') or '').strip() and n.get('enabled', True):
            out.append(n.get('role'))
        stack.extend(n.get('children') or [])
    return out


# ---- the run -------------------------------------------------------------------------------------------

def uia_read(desktop, out_file, page, seconds=60):
    """Runs uia_tree.ps1 on the hub's own desktop (attach mode: reads the window, acts on nothing)."""
    cmd = (f"& {ps_quote(POWERSHELL)} -NoProfile -ExecutionPolicy Bypass -File {ps_quote(UIA)} -ProcessId {{pid}} "
           f"-ProcessName hideout -Title ('Hideout (lab) ' + [char]0xB7 + ' {page}') -OutFile {ps_quote(out_file)}")
    env = {**os.environ, 'FOXGLOVE_APP_OK': '1'}
    r = subprocess.run([POWERSHELL, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(QUIET), '-Desktop', desktop,
                        '-Run', cmd, '-RunTimeoutSec', str(seconds)], capture_output=True, text=True, env=env,
                       timeout=seconds + 30, creationflags=0x08000000)
    return r.returncode, (r.stdout + r.stderr).strip()[-300:]


def run_checks(hub, check, desktop, exe):
    hub.wait(lambda s: has(s, 'Apps'), 20)  # the first frame may not be drawn yet
    hub.do('focus', 'Apps')
    ok, s = hub.wait(lambda s: has(s, f'Add {NAME}'), 10)
    check('A3 the Found card offers Add Vulpes', has(s, f'Add {NAME}'),
          f'{[t for t in texts(s) if NAME in t][:3]}; page shows {texts(s)[:40]}')
    line = tray_status(s) or ''
    check('A2 no app is shown as 0', line == 'No apps yet' and not contains(s, '0 apps'), f'tray line {line!r}')

    # A4: Add opens the dialog (the app is added off); closing the window is Not now.
    hub.do('click', f'Add {NAME}')
    ok, s = hub.wait(dialog_open, 10)
    check('A4 Add opens the permission dialog with Not now', ok and has(s, 'Not now'),
          f'Allow label {allow_label(s)!r}')
    check('D1 the dialog opens with Not now focused', (s['main'].get('focus') or '') == 'Not now',
          f"focus {s['main'].get('focus')!r}")
    hub.capture('A4-dialog')
    hub.do('close-window')
    time.sleep(1)
    hub.show_main()  # F3: the tray model's Open Hideout, no menu, no cursor
    s = hub.state()
    check('F3 Open Hideout from the tray model shows the main window', s['main']['on_screen'],
          f"foreground {s.get('foreground')!r}")
    closed = not dialog_open(s)
    check('A4 window close is Not now: the dialog is gone', closed,
          'gone' if closed else 'the dialog is still open after the window was closed and reopened')
    check('A4 window close starts nothing', not hub.pids(exe), f'running: {bool(hub.pids(exe))}')
    if not closed:
        hub.do('key', 'esc', 'main')

    # Esc is Not now too; then the label moves with the ticks (D2).
    if not dialog_open(hub.state()):
        hub.do('click', 'Turn on')
        hub.wait(dialog_open, 10)
    hub.do('key', 'esc', 'main')
    s = hub.state()
    check('A4 Esc is Not now: the dialog is gone, nothing started', not dialog_open(s) and not hub.pids(exe),
          f'dialog open: {dialog_open(s)}; running: {bool(hub.pids(exe))}')
    if not dialog_open(s):
        hub.do('click', 'Turn on')
        hub.wait(dialog_open, 10)
    s = hub.state()
    sensitive = has(s, 'I understand')
    labels = [allow_label(s)]
    if sensitive:
        hub.do('click', 'I understand')
        labels.append(allow_label(hub.state()))
    check('D2 Allow and start only after I understand' if sensitive else 'D2 the Allow label moves with the ticks',
          labels[-1] == 'Allow and start' and (not sensitive or labels[0] != 'Allow and start'), f'labels {labels}')
    hub.capture('D2-ticks')

    # A5: Allow starts it; it says hello.
    hub.do('click', labels[-1] or 'Allow and start')
    ok, s = hub.wait(lambda s: 'running' in (tray_status(s) or ''), 30)
    line = tray_status(s) or ''
    check('A5 tray line says 1 app is running', ok and line.startswith('1 app is running'), repr(line))
    check('A5 the row says it is running', has(s, f'{NAME} is running.'),
          f'{[t for t in texts(s) if "running" in t][:4]}')
    check('A5 the Done card: is running, Hide, Later if you want', has(s, 'Hide') and has(s, 'Later, if you want'),
          f'page shows {texts(s)[8:20]}')
    hub.capture('A5-running')

    # A6/F4: a real UIA read.
    out = Path(hub.shots) / 'hideout-uia.json'
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists():
        out.unlink()
    rc, why = uia_read(desktop, out, 'Apps')
    if rc != 0 or not out.exists():
        check('A6 UIA read of the main window', False, f'rc {rc}: {why}', blocked=True)
        remove_checks(hub, check, exe)
        return
    tree = json.loads(out.read_text(encoding='utf-8-sig'))
    bad = unnamed_focusables(tree)
    check('A6 every focusable node has a name', not bad, f'{len(bad)} unnamed: {sorted(set(bad))}')
    names = []
    stack = [tree]
    while stack:
        n = stack.pop()
        names.append(n.get('name') or '')
        stack.extend(n.get('children') or [])
    check('F4 window title reads Hideout (lab) · <page>', any(t.startswith('Hideout (lab) · ') for t in names) or
          (tree.get('name') or '').startswith('Hideout (lab)'), f'{[t for t in names if t.startswith("Hideout")][:3]}')
    remove_checks(hub, check, exe)


def remove_checks(hub, check, exe):
    """C3/C4: Remove from Hideout on the running fixture app."""
    more = f'More actions for {NAME}'
    ok, s = hub.wait(lambda s: has(s, more), 10)
    if not check('C3 the row has "More actions for Vulpes"', ok, f'{[t for t in texts(s) if "More" in t][:3]}'):
        return
    hub.do('click', more)
    ok, s = hub.wait(lambda s: has(s, 'Remove from Hideout…'), 5)
    check('C3 the menu offers Remove from Hideout…', ok, f'{texts(s)[:30]}')
    hub.do('click', 'Remove from Hideout…')
    question = f'Remove {NAME} from Hideout?'
    ok, s = hub.wait(lambda s: has(s, question), 5)
    check('C3 the question names the app', ok, f'{texts(s)[:30]}')
    check('C3 Cancel is focused', (s['main'].get('focus') or '') == 'Cancel', f"focus {s['main'].get('focus')!r}")
    check('C3 the adopted text says its own settings and files stay', contains(s, 'Its own settings and files stay'),
          f'{[t for t in texts(s) if "stay" in t][:2]}')
    hub.capture('C3-remove-confirm')
    hub.do('click', 'Cancel')
    ok, s = hub.wait(lambda s: not has(s, question), 5)
    check('C3 Cancel keeps the app running', has(s, f'{NAME} is running.') and bool(hub.pids(exe)),
          f'row {has(s, f"{NAME} is running.")}; running {bool(hub.pids(exe))}')
    hub.wait(lambda s: has(s, more), 5)
    hub.do('click', more)
    hub.wait(lambda s: has(s, 'Remove from Hideout…'), 5)
    hub.do('click', 'Remove from Hideout…')
    hub.wait(lambda s: has(s, question), 5)
    hub.do('click', 'Remove')
    ok, s = hub.wait(lambda s: not has(s, f'{NAME} is running.') and not has(s, question), 15)
    check('C4 the row is gone after Remove', ok, f'{[t for t in texts(s) if NAME in t][:3]}')
    ok, s = hub.wait(lambda s: (tray_status(s) or '') == 'No apps yet', 10)
    check('C4 the tray line drops the app', ok, repr(tray_status(s)))
    end = time.time() + 15
    while hub.pids(exe) and time.time() < end:
        time.sleep(0.5)
    check('C4 the app was stopped', not hub.pids(exe), f'running: {bool(hub.pids(exe))}')


SENSORS_NAME = 'PC sensors'
RAW_IDS = ('facts.write', 'facts.read', 'sensor.*', 'keyboard.game_mode', 'org.foxglove')


def sensors_fixture(root, hub_exe, sensors_exe, module_toml):
    r"""Places what demo-up.ps1 -SensorsExe places next to a build: a copy of the hub's exe in `<root>\hub` with
    `apps\sensors\sensors.exe` and `module.toml` beside it, plus the lab's config dir `<root>\cfg` with an empty fake registry.
    The hub's exe folder must stay outside the config dir: a service file below the hub's own folder is refused.
    Returns (the hub exe copy, the placed module.toml)."""
    root = Path(root).resolve()
    hub = root / 'hub'
    hub.mkdir(parents=True, exist_ok=True)
    app = hub / 'hideout.exe'
    shutil.copy2(hub_exe, app)
    placed = hub / 'apps' / 'sensors'
    placed.mkdir(parents=True, exist_ok=True)
    shutil.copy2(sensors_exe, placed / 'sensors.exe')
    manifest = placed / 'module.toml'
    shutil.copy2(module_toml, manifest)
    write_text(root / 'cfg' / 'lab-discover.json',
               json.dumps({'registry': {}, 'env': {'LOCALAPPDATA': str(root / 'local')}}))
    return app, manifest


def value_after(state, label):
    """A detail row's drawn value: the text right after the row's own `<label>, ...` node (the row is read out
    as "Processor, reading" while it waits and "Processor, 12 percent" after), or None."""
    ts = texts(state)
    for i, t in enumerate(ts[:-1]):
        if t.startswith(label + ', '):
            return ts[i + 1]
    return None


def bump_version(manifest):
    """Rewrites the placed module.toml's version (0.1.0 -> 0.1.1): the file changed since the app was added."""
    text = Path(manifest).read_text(encoding='utf-8')
    new = re.sub(r'(?m)^version = "[^"]*"', 'version = "9.9.9"', text, count=1)
    if new == text:
        raise ValueError('no version line in the manifest')
    write_text(manifest, new)
    return '9.9.9'


def reading_ok(value):
    """A sensor value reads as a number with a unit, or "Not reported"; a bare 0 / 0 % / 0 C is the old lie."""
    if value is None:
        return False
    v = value.strip()
    if v == 'Not reported':
        return True
    m = re.match(r'^(\d+(?:[.,]\d+)?)\s*\S+', v)
    return bool(m) and float(m.group(1).replace(',', '.')) != 0.0 or (bool(m) and '%' in v)


def s1_checks(hub, check, exe, manifest):
    """The PC sensors flow: Found card, Add, Allow, running, readings, changed file."""
    hub.wait(lambda s: has(s, 'Apps'), 20)  # the first frame may not be drawn yet
    hub.do('focus', 'Apps')
    ok, s = hub.wait(lambda s: has(s, f'Add {SENSORS_NAME}'), 15)
    check('S1 the Found card offers Add PC sensors', ok, f'{[t for t in texts(s) if "ensor" in t][:3]}; {texts(s)[:30]}')
    if not ok:
        return
    hub.do('click', f'Add {SENSORS_NAME}')
    ok, s = hub.wait(dialog_open, 10)
    check('S1 Add opens the permission dialog', ok, f'{texts(s)[:30]}')
    if not ok:
        return
    raw = [t for t in texts(s) if any(r in t for r in RAW_IDS)]
    check('S1 the dialog uses plain words (no raw ids)', not raw, f'{raw[:3]}')
    hub.capture('S1-dialog')
    if has(s, 'I understand'):
        hub.do('click', 'I understand')
    label = allow_label(hub.state())
    hub.do('click', label or 'Allow')
    ok, s = hub.wait(lambda s: (tray_status(s) or '') == '1 app is running', 30)
    check('S1 Allow starts the app (tray: 1 app is running)', ok, repr(tray_status(s)))
    check('S1 sensors.exe is running', bool(hub.pids(exe)), f'{hub.pids(exe)}')
    # the page: the hub says someone listens when the page is open, so the app samples
    hub.do('focus', SENSORS_NAME)
    ok, s = hub.wait(lambda s: has(s, 'Processor') and has(s, 'Temperature'), 20)
    check('S1 the PC sensors page shows Processor and Temperature', ok, f'{texts(s)[:40]}')
    if ok:
        ok, s = hub.wait(lambda s: not has(s, 'Reading…'), 30)
        check('S1 the readings arrive (no row still says Reading…)', ok, f'{texts(s)[14:60]}')
        cpu, temp = value_after(s, 'Processor'), value_after(s, 'Graphics card temperature')
        check('S1 Processor reads a percentage', bool(cpu) and '%' in cpu, f'{cpu!r}')
        check('S1 Temperature is a number or "Not reported", never 0', reading_ok(temp), f'{temp!r}')
        hub.capture('S1-page')
    # the changed-since-added path: the file next to the exe now says another version
    bump_version(manifest)
    hub.do('focus', 'Apps')
    hub.do('click', 'Add an app…')  # the list looks at the folders again when Add an app is opened
    ok, s = hub.wait(lambda s: has(s, 'Update in Hideout'), 30)
    check('S1 a changed file is offered as "Update in Hideout"', ok, f'{texts(s)[14:60]}')
    hub.capture('S1-update')
    if not ok:
        return
    # #1455: the row says "changed since you added it", and Update in Hideout stops the running app,
    # registers the new file, asks again, and starts it again as the new version
    row = [t for t in texts(s) if SENSORS_NAME in t and 'changed since you added it' in t]
    check('S1 the row says changed since you added it', bool(row), f'{row[:2]}')
    old = hub.pids(exe)
    hub.do('click', 'Update in Hideout')
    ok, s = hub.wait(dialog_open, 15)
    check('S1 Update in Hideout opens the Allow dialog', ok, f'{texts(s)[:30]}')
    stopped = not set(old) & set(hub.pids(exe))
    check('S1 the old process was stopped by the update', stopped, f'old {old}; now {hub.pids(exe)}')
    if not ok:
        return
    if has(s, 'I understand'):
        hub.do('click', 'I understand')
    hub.do('click', allow_label(hub.state()) or 'Allow')
    ok, s = hub.wait(lambda s: (tray_status(s) or '') == '1 app is running' and not has(s, 'Update in Hideout'), 30)
    check('S1 the app runs again after Allow, no Update left (tray: 1 app is running)', ok, repr(tray_status(s)))
    new = hub.pids(exe)
    check('S1 it is a new process, not the old one', bool(new) and not set(old) & set(new), f'old {old}; new {new}')
    hub.capture('S1-updated')
    # #1443: Add an app... looks at the folders again; the updated app must not be offered an Update again
    hub.do('click', 'Add an app…')
    time.sleep(2)
    s = hub.state()
    check('S1 reopening Add an app… offers no second Update', not has(s, 'Update in Hideout'),
          f'{[t for t in texts(s) if "pdate" in t][:3]}')


TOOL_TITLE = 'Allow a program outside Hideout?'
TOOL_HEADER = 'A program outside Hideout asks for'
KEPT_NOW = 'Your OK stayed, except Sensitive and keyboard'
KEPT_WORDS = 'Sensitive items and the keyboard need your OK again. It runs without them until then.'
KEEP_LABEL = 'Keep my OK after updates in this folder, except Sensitive and keyboard'


def tool_dialog_open(state):
    return has(state, TOOL_TITLE)


def tool_asks(state):
    """The Apps page's header line about a program waiting for the user, or None."""
    return next((t for t in texts(state) if t.startswith(TOOL_HEADER)), None)


def lab_cli_pipe(log):
    r"""The lab hub's alias for outside tools, from its log: `\\.\pipe\foxglove-<user><suffix>`."""
    m = re.search(r'serving (\\\\\.\\pipe\\hideout-\S+)', log)
    return m.group(1).replace('pipe\\hideout-', 'pipe\\foxglove-') if m else None


def run_cli(cli, pipe, *args):
    """Runs the outside tool once, windowless, against the lab hub's pipe only; returns (code, output)."""
    env = {**os.environ, 'FOXGLOVE_PIPE': pipe}
    r = subprocess.run([str(cli), *args], env=env, capture_output=True, text=True, encoding='utf-8',
                       errors='replace', timeout=30, creationflags=0x08000000)
    return r.returncode, (r.stdout + r.stderr).strip()


def tools_fixture(root, hub_exe, cli_exe):
    r"""`<root>\hub` holds the hub's exe copy, `<root>\cfg` the lab config, `<root>\tools\a` the outside tool.
    Returns (hub exe copy, tool copy)."""
    root = Path(root).resolve()
    (root / 'hub').mkdir(parents=True, exist_ok=True)
    app = root / 'hub' / 'hideout.exe'
    shutil.copy2(hub_exe, app)
    (root / 'tools' / 'a').mkdir(parents=True, exist_ok=True)
    tool = root / 'tools' / 'a' / 'foxglove-cli.exe'
    shutil.copy2(cli_exe, tool)
    write_text(root / 'cfg' / 'lab-discover.json',
               json.dumps({'registry': {}, 'env': {'LOCALAPPDATA': str(root / 'local')}}))
    return app, tool


def wait_ask(hub, want=True, seconds=15):
    return hub.wait(lambda s: bool(tool_asks(s)) == want, seconds)


def open_tool_dialog(hub):
    for _ in range(2):  # the first click can land before the page shows the line's button
        hub.wait(lambda s: has(s, 'Review…'), 5)
        hub.do('click', 'Review…')
        ok, s = hub.wait(tool_dialog_open, 5)
        if ok:
            break
    return ok, s


def tools_checks(hub, check, tool):
    """An outside tool (foxglove-cli) asks in the hub's own dialog, per program file (#1471).
    The lab hub opts into the installed hub's rule with HIDEOUT_LAB_TOOL_ASK=1 (see the run's env)."""
    root = Path(tool).parents[2]
    pipe = lab_cli_pipe(hub.log())
    check('T0 the log names the hub pipe the tool can reach', bool(pipe), repr(pipe))
    if not pipe:
        return
    hub.wait(lambda s: has(s, 'Apps'), 20)
    hub.do('focus', 'Apps')
    code, out = run_cli(tool, pipe, 'profile')
    ok, s = wait_ask(hub)
    check('T1 a first run asks: the Apps page says a program outside Hideout asks', ok,
          f'cli {code} {out[:80]!r}; {[t for t in texts(s) if "outside" in t][:2]}')
    if not ok:
        return
    ok, s = open_tool_dialog(hub)
    check('T2 Review opens the hub dialog for the program file',
          ok and any('foxglove-cli.exe' in t for t in texts(s)), f'{[t for t in texts(s) if "Program file" in t][:2]}')
    if not ok:
        return
    hub.capture('T2-tool-dialog')
    # the window's close button is Not now
    hub.do('close-window')
    time.sleep(1)
    hub.show_main()
    s = hub.state()
    check('T3 the close button dismisses the dialog', not tool_dialog_open(s),
          'dialog still open' if tool_dialog_open(s) else 'gone')
    time.sleep(1)
    check('T3 nothing was allowed by the close (the page does not list the program)',
          not has(hub.state(), 'Programs outside Hideout, 1 program'), f'ask line after close: {tool_asks(hub.state())!r}')
    # Deny (Not now): nothing is remembered; the tool asks again when it runs again
    run_cli(tool, pipe, 'profile')
    wait_ask(hub)
    opened, s = open_tool_dialog(hub)
    ok = False
    if opened:
        time.sleep(1.5)
        hub.do('click', 'Not now')
        ok, s = hub.wait(lambda s: not tool_dialog_open(s), 20)
    # a dialog that did not open again after the close is the harness (the ask line is late), not a Deny result
    check('T4 Not now (Deny) closes the dialog', ok, f'{texts(s)[:20]}' if opened else 'dialog did not open again (harness-only)',
          blocked=not opened)
    run_cli(tool, pipe, 'profile')
    ok, s = wait_ask(hub)
    check('T4 Deny is not remembered: the same file still asks', ok, f'{tool_asks(s)!r}')
    # Allow
    ok, s = open_tool_dialog(hub)
    if not ok:
        check('T5 the dialog opens again', False, f'{texts(s)[:20]}')
        return
    time.sleep(1.5)  # Allow is armed after 1 s
    hub.do('click', allow_label(hub.state()) or 'Allow')
    ok, s = hub.wait(lambda s: not tool_dialog_open(s) and not tool_asks(s), 10)
    check('T5 Allow closes the dialog and the ask is gone', ok, f'{tool_asks(s)!r}')
    code, out = run_cli(tool, pipe, 'profile')
    time.sleep(2)
    s = hub.state()
    check('T6 a second run of the same file does not ask again', not tool_asks(s),
          f'cli {code} {out[:60]!r}; {tool_asks(s)!r}')
    # a copy elsewhere is another program file
    copy = root / 'tools' / 'b' / 'foxglove-cli.exe'
    copy.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(tool, copy)
    run_cli(copy, pipe, 'profile')
    ok, s = wait_ask(hub)
    check('T7 a copy of the file in another folder asks', ok, f'{tool_asks(s)!r}')
    ok, s = open_tool_dialog(hub)
    if ok:
        time.sleep(1.5)
        hub.do('click', allow_label(hub.state()) or 'Allow')
        hub.wait(lambda s: not tool_dialog_open(s), 10)
    # #1470: the Apps page lists both allowed programs; Forget removes one, with no confirm
    ok, s = hub.wait(lambda s: has(s, 'Programs outside Hideout, 2 programs'), 10)
    check('F1 the Apps page lists "Programs outside Hideout, 2 programs"', ok,
          f'{[t for t in texts(s) if "outside Hideout" in t][:3]}')
    hub.capture('F1-list')
    forgets = [t for t in texts(s) if t.startswith('Forget ')]
    if ok and forgets:
        hub.do('click', forgets[0])
        time.sleep(1)
        s = hub.state()
        check('F2 Forget asks nothing (no dialog)', not tool_dialog_open(s) and not dialog_open(s), f'{texts(s)[:12]}')
        ok, s = hub.wait(lambda s: has(s, 'Programs outside Hideout, 1 program'), 10)
        check('F3 Forget on one program leaves the other', ok, f'{[t for t in texts(s) if "outside Hideout" in t][:3]}')
        run_cli(copy, pipe, 'profile')
        ok, s = wait_ask(hub)
        check('F4 the forgotten file asks again at its next run', ok, f'{tool_asks(s)!r}')
        ok, s = open_tool_dialog(hub)
        if ok:
            hub.do('click', 'Not now')
            hub.wait(lambda s: not tool_dialog_open(s), 10)
        run_cli(tool, pipe, 'profile')
        time.sleep(2)
        check('F5 the other file still does not ask', not tool_asks(hub.state()), f'{tool_asks(hub.state())!r}')
    # the allowed file, changed in place: the old Allow is void
    with open(tool, 'ab') as f:
        f.write(b'\0' * 16)
    ok, s = wait_ask(hub, False, 5)
    run_cli(tool, pipe, 'profile')
    ok, s = wait_ask(hub)
    check('T8 the same file with other bytes asks again', ok, f'{tool_asks(s)!r}')


def keep_checks(hub, check, exe, manifest):
    """#1465: Keep my OK on a changed app file asks again for what is sensitive (the Changed-row words)."""
    hub.wait(lambda s: has(s, 'Apps'), 20)
    hub.do('focus', 'Apps')
    ok, s = hub.wait(lambda s: has(s, f'Add {SENSORS_NAME}'), 15)
    if not ok:
        check('K0 the Found card offers Add PC sensors', False, f'{texts(s)[:30]}')
        return
    hub.do('click', f'Add {SENSORS_NAME}')
    ok, s = hub.wait(dialog_open, 10)
    if not ok:
        check('K0 Add opens the dialog', False, f'{texts(s)[:30]}')
        return
    if has(s, 'I understand'):
        hub.do('click', 'I understand')
    hub.do('click', KEEP_LABEL + ', not checked')
    ok, s = hub.wait(lambda s: any(t.startswith('I understand:') for t in texts(s)), 10)
    check('K1 ticking Keep asks for I understand', ok, f'{[t for t in texts(s) if "understand" in t][:3]}')
    if ok:
        node = next(t for t in texts(s) if t.startswith('I understand:'))
        hub.do('click', node)
        time.sleep(1)
    hub.do('click', allow_label(hub.state()) or 'Allow')
    ok, s = hub.wait(lambda s: (tray_status(s) or '') == '1 app is running', 30)
    check('K2 Allow with Keep starts the app', ok, repr(tray_status(s)))
    # only the app file changes (a changed module.toml is another path: it voids the answer)
    # a running exe can't be replaced: stop it, then swap a changed copy in at once (os.replace is atomic, so the
    # supervisor's restart never reads a half-written file); retry if the supervisor won the race
    new = Path(str(exe) + '.new')
    shutil.copy2(exe, new)
    with open(new, 'ab') as f:
        f.write(b'\0' * 16)
    for _ in range(50):
        for pid in hub.pids(exe):
            hub.kill(pid)
        try:
            os.replace(new, exe)
            break
        except OSError:
            time.sleep(0.1)
    hub.do('focus', 'Apps')
    hub.do('click', 'Add an app\u2026')
    ok, s = hub.wait(lambda s: contains(s, KEPT_NOW), 30)
    row = [t for t in texts(s) if 'New build' in t]
    check('K3 the Permissions line says the OK stayed, except Sensitive and keyboard', ok, f'{row[:3]}')
    ok, s = hub.wait(lambda s: (tray_status(s) or '') == '1 app is running', 20)
    check('K3 the new build runs (the folder grant kept the OK)', ok, repr(tray_status(s)))
    check('K4 Keep is still On for the folder', has(s, 'Turn off keep my OK after updates') or contains(s, 'Turn off'),
          f'{[t for t in texts(s) if "Keep" in t][:3]}')
    hub.capture('K3-changed')
    log_before = len(hub.log())
    # this fixture has no Sensitive item waiting, so KEPT_WORDS (#1465) can't show: that needs a Sensitive grant
    check('K4 no Sensitive item waits here, so no Review is offered (the #1465 words need one)',
          not contains(s, KEPT_WORDS), 'not shown, as expected')
    time.sleep(1)
    new_log = hub.log()[log_before:]
    check('K5 nothing sensitive started before Allow: no grant line after the change',
          not re.search(r'sensor\.|keyboard', new_log) or 'grant' not in new_log.lower(), new_log[-200:])


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'), help='the hideout.exe under test')
    ap.add_argument('--probe', type=tool_arg('--probe'), help='hideout-ac-probe.exe (the hello fixture)')
    ap.add_argument('--lab', required=True, type=tool_arg('--lab', kind='dir'), help='an existing, EMPTY scratch folder: the lab hub config dir (seeded by the run)')
    ap.add_argument('--only', default='first-run', choices=('first-run', 's1', 'tools', 'keep'), help='which flow to run')
    ap.add_argument('--sensors', type=tool_arg('--sensors'), help='sensors.exe (for --only s1, keep)')
    ap.add_argument('--cli', type=tool_arg('--cli'), help='foxglove-cli.exe (for --only tools)')
    ap.add_argument('--seconds', type=int, default=300, help='hard time limit of the hidden hub (1..3600)')
    ap.add_argument('--allow-running', action='store_true', help='other hideout.exe processes may be running')
    args = ap.parse_args()
    if not 1 <= args.seconds <= 3600:
        ap.error('--seconds must be 1..3600')
    require_app_ok()
    home = Path(os.environ.get('LOCALAPPDATA', '')) / 'Hideout'
    lab = Path(args.lab).resolve()
    if lab == home.resolve() or home.resolve() in lab.parents:
        print(f"BLOCKED: {lab} is the user's hub folder\nRESULT BLOCKED", flush=True)
        sys.exit(2)
    if lab.exists() and any(lab.iterdir()):
        print(f'BLOCKED: {lab} is not empty (a run needs a fresh scratch folder)' + chr(10) + 'RESULT BLOCKED', flush=True)
        sys.exit(2)
    if args.only in ('s1', 'keep') and not args.sensors:
        ap.error('--only s1 and --only keep need --sensors <sensors.exe>')
    if args.only == 'tools' and not args.cli:
        ap.error('--only tools needs --cli <foxglove-cli.exe>')
    if args.only == 'first-run' and not args.probe:
        ap.error('--probe <hideout-ac-probe.exe> is needed for the first-run flow')
    module_toml = Path(__file__).resolve().parents[2] / 'crates' / 'sensors' / 'module.toml'
    try:
        if args.only in ('s1', 'keep'):
            app, manifest = sensors_fixture(lab, args.app, args.sensors, module_toml)
            exe = Path(app).parent / 'apps' / 'sensors' / 'sensors.exe'
        elif args.only == 'tools':
            app, tool = tools_fixture(lab, args.app, args.cli)
        else:
            app, exe = args.app, hello_fixture(lab, args.probe)
    except (OSError, ValueError) as e:
        print(f'BLOCKED: seeding {lab}: {e}' + chr(10) + 'RESULT BLOCKED', flush=True)
        sys.exit(2)
    desktop = 'ql-ux' + os.urandom(3).hex()
    cfg = lab / 'cfg' if args.only in ('s1', 'keep', 'tools') else lab
    extra = {'HIDEOUT_LAB_TOOL_ASK': '1'} if args.only == 'tools' else {}
    hub = QuietHub(str(app), cfg, lab.parent / 'shots', desktop, extra, seconds=args.seconds,
                   allow_running=args.allow_running)
    check = Checks()
    try:
        hub.start()
        if args.only == 's1':
            s1_checks(hub, check, exe, manifest)
        elif args.only == 'keep':
            keep_checks(hub, check, exe, manifest)
        elif args.only == 'tools':
            tools_checks(hub, check, tool)
        else:
            run_checks(hub, check, desktop, exe)
    except Exception as e:  # noqa: BLE001 (reported as the run's FAIL, the hub still stops)
        check('run', False, f'{type(e).__name__}: {e}')
    finally:
        hub.stop()
    check.finish()


if __name__ == '__main__':
    main()
