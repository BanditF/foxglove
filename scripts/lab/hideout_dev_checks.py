"""Hideout Settings > Developer checks (DF1 Remove confirm, DF2 empty-folder line, DF5 dev-off
lines) on a hidden desktop: no window, no focus change and no tray on the user's desktop.

The lab hub (`hideout.exe --lab`, HIDEOUT_LAB=1, HIDEOUT_LAB_CAPTURE=1, its own HIDEOUT_CONFIG_DIR)
is started through scripts/lab/quiet-launch.ps1 (a private desktop, a hard time limit, a kill-on-close
job) and driven over its lab control pipe (`hideout.exe --lab-do <verb>`, docs/hideout-lab.md) from
the default desktop: the pipe is not per desktop, and `focus`/`click`/`state` use AccessKit, not the
mouse. Prints PASS/FAIL/BLOCKED per step and RESULT (exit 0 PASS, 1 FAIL, 2 BLOCKED). Needs
FOXGLOVE_APP_OK=1 (On-call). Run scripts/ops/game-check.ps1 first; this script does not start while
it reports a game (pass --no-game-check only for a test).

    FOXGLOVE_APP_OK=1 python scripts/lab/hideout_dev_checks.py --app <hideout.exe> --lab <empty folder>
        --folder <folder to list as the dev folder> [--allow-running]

--lab (an existing, empty folder) must not be the user's hub folder. --folder is created if it is missing and must pass the
dev-folder rules (not under AppData, Temp or Downloads; not writable by other accounts). The tray,
the folder picker (Add a folder...) and drawn pixels are not covered: see docs/lab-quiet-launch.md.
"""
import argparse, base64, os, subprocess, sys, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from labkit import tool_arg, Checks, require_app_ok  # noqa: E402
from hideout_lab_checks import Hub, texts, has, contains, node_index, step  # noqa: E402

POWERSHELL = str(Path(os.environ.get('SystemRoot', r'C:\Windows')) / 'System32' / 'WindowsPowerShell' / 'v1.0' / 'powershell.exe')
QUIET = Path(__file__).resolve().parent / 'quiet-launch.ps1'

# The words, as in crates/hideout-shell/src/dev_settings.rs (a unit test compares them with it).
HEADING = 'Developer'
FOLDERS_HEADING = 'Dev folders'
NO_APPS = 'No apps from here yet'
FOOTNOTE = 'Developer mode is on for this PC. Hideout as shipped keeps it off.'
MODE_OFF = 'Developer mode is off on this PC, so dev folders do nothing.'
MODE_OFF_MORE = ('Apps from them ask for your OK at their next build. To use dev folders, start '
                 'Hideout with HIDEOUT_DEV_MODE=1.')
CONFIRM_TITLE = 'Remove this dev folder?'
CONFIRM_CANCEL = 'Cancel'
CONFIRM_OK = 'Remove folder'
CONFIRM_NO_APPS = 'No apps from here. Nothing else changes.'
REMOVED = 'Removed.'
REMOVE_PREFIX = 'Remove dev folder '
WORDS = {
    'HEADING': HEADING, 'FOLDERS_HEADING': FOLDERS_HEADING, 'NO_APPS': NO_APPS, 'FOOTNOTE': FOOTNOTE,
    'MODE_OFF': MODE_OFF, 'MODE_OFF_MORE': MODE_OFF_MORE, 'CONFIRM_TITLE': CONFIRM_TITLE,
    'CONFIRM_CANCEL': CONFIRM_CANCEL, 'CONFIRM_OK': CONFIRM_OK, 'CONFIRM_NO_APPS': CONFIRM_NO_APPS,
    'REMOVED': REMOVED,
}


# ---- pure parts (tested window-free in scripts/test_hideout_dev_checks.py) -------------------------

def toml_literal(path):
    """A TOML literal string for a Windows path (no escapes); refuses a quote."""
    if "'" in str(path) or '\n' in str(path):
        raise ValueError(f'a quote or newline in {path!r}')
    return f"'{path}'"


def seed_config(folder=None):
    """hideout.toml text: the dev folder list (or an empty file)."""
    return "[dev]\nfolders = []\n" if folder is None else f"[dev]\nfolders = [{toml_literal(folder)}]\n"


def ps_quote(text):
    """A PowerShell single-quoted string; a quote in a path is refused, not escaped."""
    if any(c in str(text) for c in "'\n\0‘’‚‛"):  # PowerShell reads curly quotes as quotes
        raise ValueError(f'a quote, newline or NUL in {text!r}')
    return f"'{text}'"


def launch_script(exe, lab, desktop, env, seconds, allow_running=False):
    """The PowerShell text that starts the lab hub through quiet-launch.ps1 (real hashtable)."""
    pairs = '; '.join(f'{k} = {ps_quote(v)}' for k, v in env.items())
    return (f"& {ps_quote(QUIET)} -Exe {ps_quote(exe)} -ArgList '--lab' -Env @{{ {pairs} }} "
            f"-DesktopName {ps_quote(desktop)} -TimeoutSec {int(seconds)}"
            + (' -AllowRunning' if allow_running else ''))


def stop_script(desktop):
    return f"& {ps_quote(QUIET)} -Stop {ps_quote(desktop)}"


def encoded(script):
    return base64.b64encode(script.encode('utf-16-le')).decode('ascii')


def remove_button(state):
    """The first node index whose name starts with 'Remove dev folder ', and its text."""
    i = node_index(state, 'main', lambda n: (n.get('name') or '').startswith(REMOVE_PREFIX))
    return i, (state['main']['nodes'][i]['name'] if i is not None else None)


# ---- the hub on a hidden desktop -------------------------------------------------------------------

def hidden_ps(script):
    """Runs `script` in a PowerShell that never shows a console."""
    env = {k: v for k, v in os.environ.items() if not k.startswith('HIDEOUT_DEV_')}  # quiet-launch passes ours on
    return subprocess.Popen(
        [POWERSHELL, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', encoded(script)],
        env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        creationflags=0x08000000)  # CREATE_NO_WINDOW


class QuietHub(Hub):
    """A lab hub that lives on its own hidden desktop; `do`/`state` are the parent class's."""

    def __init__(self, exe, lab, shots, desktop, extra_env, seconds=240, allow_running=False):
        super().__init__(exe, lab, shots)
        for k in [k for k in self.env if k.startswith('HIDEOUT_DEV_')]:
            del self.env[k]  # an ambient HIDEOUT_DEV_* must not decide what a run shows
        self.extra_env = dict(extra_env)
        self.desktop, self.seconds, self.allow_running = desktop, seconds, allow_running
        self.launcher = None

    def start(self):
        env = {k: self.env[k] for k in ('HIDEOUT_CONFIG_DIR', 'HIDEOUT_LAB', 'HIDEOUT_LAB_CAPTURE')}
        env.update(self.extra_env)
        if self.do('state', check=False).get('ok'):
            raise RuntimeError('a lab hub already answers on the lab pipe; refusing to drive it')
        self.launcher = hidden_ps(launch_script(self.exe, self.lab, self.desktop, env, self.seconds,
                                                self.allow_running))
        ok, _ = self.wait(lambda s: s.get('ok'), 30, read=lambda: self.do('state', check=False))
        if not ok:
            self.stop()
            out = self.launcher.stdout.read() if self.launcher.poll() is not None else ''
            raise RuntimeError(f'the lab hub did not answer on its pipe within 30 s: {out.strip()[:300]}')
        if f'folder {self.lab}' not in self.log():
            self.stop()
            raise RuntimeError("the hub that answered is not this run's (no start line for this folder in its log)")

    def stop(self):
        if self.launcher is None:
            return
        try:
            hidden_ps(stop_script(self.desktop)).wait(30)  # the launcher's own teardown, by name
            self.launcher.wait(30)
        except subprocess.TimeoutExpired:
            self.launcher.kill()  # our own launcher handle; its job closes the hub
        self.launcher = None


def pinned(hub, check, name, pred, seconds=5, detail=''):
    ok, s = hub.wait(pred, seconds)
    check(name, ok, detail or 'ok')
    return s


# ---- the checks ------------------------------------------------------------------------------------

def open_settings(hub):
    # The sidebar may not be drawn yet right after the pipe answers: retry for a bounded 10 s.
    ok, _ = hub.wait(lambda a: a.get('ok'), 10, read=lambda: hub.do('focus', 'Settings', check=False))
    if not ok:
        raise RuntimeError('the Settings page never appeared in the sidebar within 10 s')
    return hub.state()


def df1_df2(hub, check, folder, cfg):
    def run():
        s = open_settings(hub)
        check('DF1.0 Settings > Developer lists the folder', has(s, HEADING) and has(s, FOLDERS_HEADING)
              and any(t.lower().endswith(Path(folder).name.lower()) for t in texts(s)), 'dev folder row drawn')
        check('DF2.1 a folder with no apps says so', has(s, NO_APPS), f'{NO_APPS!r} drawn')
        check('DF5.0 dev mode on: the footnote', has(s, FOOTNOTE), 'footnote drawn')
        i, name = remove_button(s)
        if i is None:
            check('DF1.1 each folder has a Remove button', False, 'no "Remove dev folder ..." button')
            return
        hub.do('click', f'@{i}')
        s = hub.state()
        check('DF1.2 Remove asks first, with the words', has(s, CONFIRM_TITLE) and has(s, CONFIRM_NO_APPS)
              and has(s, CONFIRM_CANCEL) and has(s, CONFIRM_OK), 'confirm drawn')
        check('DF1.3 nothing is removed yet', folder_listed(cfg), 'hideout.toml still lists the folder')
        check('DF1.4 Cancel is the focused default', (s['main'].get('focus') or '') == CONFIRM_CANCEL,
              f"focus {s['main'].get('focus')!r}")
        hub.do('click', CONFIRM_CANCEL)
        s = hub.state()
        check('DF1.5 Cancel closes the confirm and keeps the folder',
              not has(s, CONFIRM_TITLE) and folder_listed(cfg), 'confirm gone, folder kept')
        i, name = remove_button(s)
        hub.do('click', f'@{i}')
        hub.do('click', CONFIRM_OK)
        s = pinned(hub, check, 'DF1.6 Remove folder removes it and says so',
                   lambda s: has(s, REMOVED) and not folder_listed(cfg), detail='"Removed." drawn, list empty')
    step(check, 'DF1/DF2 run', run)


def folder_listed(cfg):
    p = Path(cfg) / 'hideout.toml'
    return p.exists() and 'folders' in p.read_text(encoding='utf-8', errors='replace')


def df5_off(hub, check, with_folder):
    def run():
        s = open_settings(hub)
        if with_folder:
            check('DF5.1 dev off with a listed folder: the two lines',
                  has(s, MODE_OFF) and has(s, MODE_OFF_MORE), 'off line and its second line drawn')
            check('DF5.2 no Remove button while dev mode is off', remove_button(s)[0] is None, 'none drawn')
        else:
            check('DF5.3 HIDEOUT_DEV_UNSIGNED alone shows no Developer section',
                  not has(s, HEADING) and not has(s, FOLDERS_HEADING), 'section absent')
    step(check, 'DF5 off run', run)


def one_run(args, check, label, desktop, folder, extra_env, steps):
    lab = Path(args.lab) / label
    lab.mkdir(parents=True, exist_ok=True)
    (lab / 'hideout.toml').write_text(seed_config(folder), encoding='utf-8')
    hub = QuietHub(args.app, lab, lab / 'lab-shots', desktop, extra_env, allow_running=args.allow_running)
    try:
        hub.start()
        steps(hub, check, lab)
    except Exception as e:  # noqa: BLE001
        check(f'{label}: hub', False, f'{type(e).__name__}: {e}', blocked=True)
    finally:
        hub.stop()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'))
    ap.add_argument('--lab', required=True, type=tool_arg('--lab', kind='dir'), help='an existing, empty folder for the lab hubs (one subfolder per run)')
    ap.add_argument('--folder', required=True, help='the dev folder to list (created if missing)')
    ap.add_argument('--allow-running', action='store_true', help='pass -AllowRunning to the launcher')
    args = ap.parse_args()
    require_app_ok()
    home = Path(os.environ.get('LOCALAPPDATA') or Path.home() / 'AppData' / 'Local') / 'Hideout'
    lab = Path(args.lab).resolve()
    if lab == home.resolve() or home.resolve() in lab.parents:
        print(f"BLOCKED: {args.lab} is the user's hub folder", flush=True)
        print('RESULT BLOCKED', flush=True)
        sys.exit(2)
    if any(lab.iterdir()):
        print(f'BLOCKED: {args.lab} is not empty', flush=True)
        print('RESULT BLOCKED', flush=True)
        sys.exit(2)
    folder = Path(args.folder)
    folder.mkdir(parents=True, exist_ok=True)
    check = Checks()
    sfx = os.urandom(4).hex()
    one_run(args, check, 'dev-on', f'ql-dfa{sfx}', folder, {'HIDEOUT_DEV_MODE': '1'},
            lambda hub, c, lab: df1_df2(hub, c, str(folder), lab))
    one_run(args, check, 'dev-off', f'ql-dfb{sfx}', folder, {},
            lambda hub, c, lab: df5_off(hub, c, True))
    one_run(args, check, 'unsigned-only', f'ql-dfc{sfx}', None, {'HIDEOUT_DEV_UNSIGNED': '1'},
            lambda hub, c, lab: df5_off(hub, c, False))
    check.finish()


if __name__ == '__main__':
    main()
