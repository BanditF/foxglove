r"""UI Automation tree of the Foxglove Settings window on the Scenes page (#1376): is the Start button there?

Starts a lab Foxglove on its own hidden desktop through scripts/lab/quiet-launch.ps1 (a private
desktop, a hard time limit, a kill-on-close job), on a temp FOXGLOVE_CONFIG_DIR (never the user's
settings), so no window, focus change or tray icon reaches the user's desktop. A start without `--tray`
opens Settings by itself. Then uia_tree.ps1 attaches to that pid ON THE LAB DESKTOP and, through UI
Automation only (no mouse, no keys), selects the Scenes page and dumps the tree. Checks, with the names
app/src/foxglove/a11y_tree_tests.rs `the_scenes_page_has_an_effect_group_a_start_button_and_a_live_region` uses:

  the tree is not empty (the window answers UI Automation with more than a bare window)
  the Scenes page is open (its sidebar entry is on)
  the Start button            Button "Start"
  the Effect group            Group "Effect" (AccessKit RadioGroup)
  the live region             a node whose UIA LiveSetting is Polite (see "Live region" below)

If the Scenes page cannot be opened, the case reads what is reachable (the Settings window's tree and its
live region) and reports Start and Effect as BLOCKED, naming what is missing.

    FOXGLOVE_APP_OK=1 python scripts/lab/uia_start.py --app <foxglove.exe> [--seconds 150] [--keep-tree <file>]
    (exit 0 PASS, 1 FAIL, 2 BLOCKED; the last line is RESULT PASS|FAIL|BLOCKED)

Needs FOXGLOVE_APP_OK=1 (On-call). Run scripts/ops/game-check.ps1 first. It never stops the user's own
Foxglove: quiet-launch refuses to start while a process named foxglove runs (BLOCKED, with its words), and
only the pid this case started is ever stopped, by its desktop's name. An aborted run (Ctrl+C, a killed
shell) is cleaned up at exit where Python can, else by the launcher's hard limit (--seconds, default 150):
the printed `scripts/lab/quiet-launch.ps1 -Stop <desktop>` ends it at once.

Waits: one deadline for the whole case. The window has no "drawn" event to wait on and the first UI
Automation read turns the accessibility tree on in that process, so uia_tree is tried up to 4 times, 4 s
apart, inside the deadline; each try is itself bounded (uia_tree's -TimeoutSec).

Live region: UIA's LiveSetting (property 30135) is read by uia_tree.ps1 (field liveSetting: 0 off,
1 polite, 2 assertive) through COM UI Automation, because the .NET client does not know that property.
A node that was not read says null, never a guess. FAIL needs every node read and none polite; if no
polite node is found and some (or all) nodes say null, the check is BLOCKED (not FAIL): a Narrator or
Accessibility Insights look is then the proof. `uia_tree.ps1 -LiveProbe` says whether this PC can read it.

Human only: "Narrator reads the Start button on the Scenes page" (docs/scripts/uia_start.md).
"""
import argparse
import atexit
import json
import os
import re
import secrets
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from hideout_lab_checks import POWERSHELL, QUIET, hidden_ps, launcher_verdict, ps_quote, stop_script  # noqa: E402
from labkit import Checks, REAL_CONFIG, require_app_ok, temp_config, tool_arg  # noqa: E402

UIA = Path(__file__).resolve().parent / 'uia_tree.ps1'
PAGE = 'Scenes'
MIN_NODES = 10    # a bare window plus a few; the Settings window measured about 140
ATTEMPTS = 4      # uia_tree reads, inside the one deadline
GAP_S = 4         # between two reads
POLITE = 1        # UIA LiveSetting: 0 off, 1 polite, 2 assertive


# ---- pure parts (tested window-free in scripts/test_uia_start.py) --------------------------------------

def launch_script(exe, cfg, desktop, seconds):
    """The PowerShell text that starts Foxglove through quiet-launch.ps1: the config folder is the
    temp one, nothing else is passed (no --tray: a plain start opens Settings)."""
    return (f"& {ps_quote(QUIET)} -Exe {ps_quote(exe)} -Env @{{ FOXGLOVE_CONFIG_DIR = {ps_quote(cfg)} }} "
            f"-DesktopName {ps_quote(desktop)} -TimeoutSec {int(seconds)}")


def launched_pid(said):
    """(desktop, pid) from quiet-launch's `QUIET-LAUNCH desktop=<name> pid=<pid> ...` line, else None."""
    m = re.search(r'QUIET-LAUNCH desktop=(\S+) pid=(\d+)', ''.join(said))
    return (m.group(1), int(m.group(2))) if m else None


def uia_command(out_file, page=PAGE):
    """The text quiet-launch's -Run runs on the lab desktop; {pid} is filled in there. `page` None
    reads the window as it is (the fallback when the page cannot be opened)."""
    cmd = (f"& {ps_quote(POWERSHELL)} -NoProfile -ExecutionPolicy Bypass -File {ps_quote(UIA)} "
           f"-ProcessId {{pid}} -Title Foxglove -TimeoutSec 20 -OutFile {ps_quote(out_file)}")
    return cmd + (f' -Page {page}' if page else '')


def nodes_of(tree):
    """Every node of a uia_tree.ps1 record tree, parents first."""
    out, stack = [], [tree]
    while stack:
        n = stack.pop()
        out.append(n)
        stack.extend(reversed(n.get('children') or []))
    return out


def find(nodes, role, name, state=None):
    return [n for n in nodes if n.get('role') == role and n.get('name') == name
            and (state is None or n.get('state') == state)]


def judge(tree, page_opened=True):
    """[(name, ok, detail, blocked)] for one dumped tree. `page_opened` False: the Scenes page could
    not be opened, so Start and Effect are BLOCKED (not reachable), not FAIL."""
    nodes = nodes_of(tree)
    res = [('the tree is not empty', len(nodes) >= MIN_NODES, f'{len(nodes)} nodes under "{tree.get("name")}"', False)]
    sidebar = find(nodes, 'Button', PAGE)
    opened = bool(find(nodes, 'Button', PAGE, 'on'))
    res.append((f'the {PAGE} page is open', opened,
                f'sidebar entry {PAGE}: {[n.get("state") for n in sidebar] or "not in the tree"}', not page_opened))
    for label, role, name in (('the Start button', 'Button', 'Start'), ('the Effect group', 'Group', 'Effect')):
        hit = find(nodes, role, name)
        if hit:
            res.append((label, True, f'{role} "{name}" found ({"enabled" if hit[0].get("enabled") else "DISABLED"})', False))
        else:
            named = [(n.get('role'), n.get('name')) for n in nodes if n.get('name') == name]
            res.append((label, False, f'no {role} named "{name}" (same name, other role: {named or "none"}); buttons shown: '
                                      f'{[n.get("name") for n in nodes if n.get("role") == "Button"][:12]}',
                        not page_opened))
    live = [n for n in nodes if n.get('liveSetting') in (POLITE, 2)]
    unread = [n for n in nodes if n.get('liveSetting') is None]
    if live:
        res.append(('the live region', True, f'{len(live)} node(s) with LiveSetting '
                                             f'{sorted({n["liveSetting"] for n in live})} (1 polite, 2 assertive), role {live[0].get("role")}', False))
    elif not unread:
        res.append(('the live region', False, f'LiveSetting was read on all {len(nodes)} nodes and none is polite', False))
    elif len(unread) == len(nodes):
        res.append(('the live region', False, 'LiveSetting was not read (every node says null; uia_tree says why): look with '
                                              'Narrator or Accessibility Insights', True))
    else:
        res.append(('the live region', False, f'no polite node among those read, and LiveSetting was not read on {len(unread)} of '
                                              f'{len(nodes)} nodes (first: {[(n.get("role"), n.get("name")) for n in unread[:4]]}): '
                                              'look with Narrator or Accessibility Insights', True))
    return res


def page_missing(output):
    """uia_tree said the -Page entry could not be selected (the page, not the window, is the problem)."""
    return 'no sidebar entry named' in output


# ---- the run -----------------------------------------------------------------------------------------

def uia_read(desktop, out_file, page, seconds=60):
    """uia_tree.ps1 on the lab desktop (attach mode). Returns (exit code, last words)."""
    env = {**os.environ, 'FOXGLOVE_APP_OK': '1'}
    r = subprocess.run([POWERSHELL, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(QUIET), '-Desktop', desktop,
                        '-Run', uia_command(out_file, page), '-RunTimeoutSec', str(seconds)], capture_output=True,
                       text=True, env=env, timeout=seconds + 30, creationflags=0x08000000)
    return r.returncode, (r.stdout + r.stderr).strip()[-400:]


def load_tree(path):
    try:
        return json.loads(Path(path).read_text(encoding='utf-8-sig'))
    except (OSError, ValueError):
        return None


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'))
    ap.add_argument('--seconds', type=int, default=150, help="the lab Foxglove's hard time limit (60..600)")
    ap.add_argument('--keep-tree', help='copy the last dumped tree (JSON) here')
    args = ap.parse_args()
    if not 60 <= args.seconds <= 600:
        ap.error('--seconds is 60..600')
    require_app_ok()
    check = Checks()

    cfg = temp_config('foxglove-uiastart-')
    assert Path(cfg) != REAL_CONFIG and 'foxglove-uiastart-' in str(cfg), "the lab never runs on the user's config"
    out_file = str(Path(cfg).parent / 'uia-tree.json')
    desktop = f'ql-uia{secrets.token_hex(3)}'
    started = {'launcher': None}

    def teardown():
        """Ends the lab Foxglove by its desktop's name (its own job); idempotent, also at exit."""
        launcher, started['launcher'] = started['launcher'], None
        if launcher is None:
            return
        try:
            hidden_ps(stop_script(desktop)).wait(30)
            launcher.wait(30)
        except subprocess.TimeoutExpired:
            launcher.kill()  # our own launcher handle; its job closes the app

    atexit.register(teardown)
    print(f'lab desktop {desktop}; config {cfg}; to end it by hand: scripts/lab/quiet-launch.ps1 -Stop {desktop}', flush=True)
    deadline = time.time() + args.seconds - 30
    try:
        launcher = started['launcher'] = hidden_ps(launch_script(args.app, cfg, desktop, args.seconds))
        said = []
        if not launcher_verdict(launcher, said, 60):
            why = ''.join(said).strip()[:300] or 'it said nothing'
            check('Foxglove starts on the lab desktop', False, f'quiet-launch: {why}', blocked=True)
            check.finish()
        found = launched_pid(said)
        check('Foxglove starts on the lab desktop', bool(found), f'pid {found[1] if found else "?"} on {desktop}')
        if not found:
            check.finish()
        tree, page_opened, last = None, True, ''
        for attempt in range(1, ATTEMPTS + 1):
            Path(out_file).unlink(missing_ok=True)
            rc, last = uia_read(desktop, out_file, PAGE if page_opened else None)
            tree = load_tree(out_file)
            print(f'uia_tree try {attempt}: exit {rc}, {len(nodes_of(tree)) if tree else 0} nodes', flush=True)
            if rc == 3 and 'BLOCKED' in last:
                check('uia_tree reads the lab Foxglove', False, last, blocked=True)
                check.finish()
            if rc == 0 and tree and find(nodes_of(tree), 'Button', 'Start'):
                break
            if page_opened and page_missing(last):
                page_opened = False  # the page is not reachable: read the window as it is, once more
                continue
            if time.time() + GAP_S > deadline:
                break
            time.sleep(GAP_S)
        if tree is None:
            check('the window answers UI Automation', False, f'no tree after {ATTEMPTS} tries: {last}')
        else:
            for name, ok, detail, blocked in judge(tree, page_opened):
                check(name, ok, detail, blocked=blocked and not ok)
            if args.keep_tree:
                Path(args.keep_tree).write_bytes(Path(out_file).read_bytes())
            if not page_opened:
                print(f'MISSING: the {PAGE} page could not be opened through UI Automation: {last}', flush=True)
    finally:
        teardown()
    print('human only: Narrator reads the Start button on the Scenes page (not run here)', flush=True)
    check.finish()


if __name__ == '__main__':
    main()
