"""scripts/lab/uia_start.py's pure parts (#1376): the launch text, the pid line, the verdicts on a dumped
tree and the FOXGLOVE_APP_OK gate. Window-free: nothing is launched, no UI Automation call is made.

    python -m unittest scripts/test_uia_start.py
"""
import os
import re
import subprocess
import sys
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'
os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
sys.path.insert(0, str(LAB))
import uia_start as u  # noqa: E402


def node(role, name, children=(), state=None, live=None, enabled=True):
    return {'role': role, 'name': name, 'automationId': '', 'enabled': enabled, 'state': state,
            'rect': {'x': 0, 'y': 0, 'w': 1, 'h': 1}, 'liveSetting': live, 'children': list(children)}


def scenes_tree(start=True, effect=True, scenes_on=True, live=1, extra=12):
    page = [node('Group', 'Effect', [node('RadioButton', 'Temperature', state='on')])] if effect else []
    if start:
        page.append(node('Button', 'Start'))
    page.append(node('Text', '', live=live))
    sidebar = [node('Button', p, state='on' if p == 'Scenes' and scenes_on else 'off')
               for p in ('Keyboard', 'Lights', 'Scenes', 'Screens', 'Plugins', 'App')]
    filler = [node('Text', f'row {i}', live=0 if live is not None else None) for i in range(extra)]
    return node('Window', 'Foxglove', sidebar + page + filler)


def all_read(tree):
    """The tree with LiveSetting read on every node: a null becomes 0 (off)."""
    for n in u.nodes_of(tree):
        if n['liveSetting'] is None:
            n['liveSetting'] = 0
    return tree


def verdicts(tree, **kw):
    return {name: (ok, blocked) for name, ok, _detail, blocked in u.judge(tree, **kw)}


class Pure(unittest.TestCase):
    def test_a_good_tree_passes_every_check(self):
        v = verdicts(scenes_tree())
        self.assertEqual(set(v), {'the tree is not empty', 'the Scenes page is open', 'the Start button',
                                  'the Effect group', 'the live region'})
        self.assertTrue(all(ok and not blocked for ok, blocked in v.values()), v)

    def test_a_missing_start_button_fails(self):
        v = verdicts(scenes_tree(start=False))
        self.assertEqual(v['the Start button'], (False, False))
        self.assertEqual(v['the Effect group'], (True, False))

    def test_the_name_must_be_the_right_role(self):
        tree = scenes_tree(start=False)
        tree['children'].append(node('Text', 'Start'))
        self.assertEqual(verdicts(tree)['the Start button'], (False, False))
        self.assertIn('Text', [d for n, ok, d, b in u.judge(tree) if n == 'the Start button'][0])

    def test_a_missing_effect_group_fails(self):
        self.assertEqual(verdicts(scenes_tree(effect=False))['the Effect group'], (False, False))

    def test_an_empty_tree_fails(self):
        self.assertEqual(verdicts(node('Window', 'Foxglove'))['the tree is not empty'], (False, False))

    def test_a_page_that_is_not_open_fails_the_page_check(self):
        v = verdicts(scenes_tree(scenes_on=False))
        self.assertEqual(v['the Scenes page is open'], (False, False))

    def test_an_unreachable_page_blocks_start_and_effect_not_fails_them(self):
        v = verdicts(scenes_tree(start=False, effect=False, scenes_on=False), page_opened=False)
        for name in ('the Scenes page is open', 'the Start button', 'the Effect group'):
            self.assertEqual(v[name], (False, True), name)
        self.assertEqual(v['the live region'], (True, False))

    def test_live_region_states(self):
        self.assertEqual(verdicts(scenes_tree(live=1))['the live region'], (True, False))
        self.assertEqual(verdicts(scenes_tree(live=2))['the live region'], (True, False))
        self.assertEqual(verdicts(all_read(scenes_tree(live=1)))['the live region'], (True, False))
        self.assertEqual(verdicts(all_read(scenes_tree(live=0)))['the live region'], (False, False))  # all read, none polite
        self.assertEqual(verdicts(scenes_tree(live=None))['the live region'], (False, True))  # nothing read: BLOCKED
        # Some nodes read, none polite, the rest null: the region may be one of those. BLOCKED, never a guess.
        self.assertEqual(verdicts(scenes_tree(live=0))['the live region'], (False, True))
        detail = [d for name, _ok, d, _b in u.judge(scenes_tree(live=0)) if name == 'the live region'][0]
        self.assertIn('not read on', detail)

    def test_pid_line(self):
        said = ['QUIET-LAUNCH desktop=ql-uiaab12cd pid=4242 image=foxglove.exe timeout=150\n']
        self.assertEqual(u.launched_pid(said), ('ql-uiaab12cd', 4242))
        self.assertIsNone(u.launched_pid(['refused: foxglove.exe is already running (pid 7)\n']))

    def test_launch_text_uses_the_temp_config_and_no_other_switch(self):
        text = u.launch_script(r'C:\b\foxglove.exe', r'C:\Temp\foxglove-uiastart-x\config', 'ql-uiaab12cd', 150)
        self.assertIn("FOXGLOVE_CONFIG_DIR = 'C:\\Temp\\foxglove-uiastart-x\\config'", text)
        self.assertIn("-DesktopName 'ql-uiaab12cd' -TimeoutSec 150", text)
        for banned in ('-AllowRunning', '-ArgList', '--tray', 'Stop-Process', 'taskkill'):
            self.assertNotIn(banned, text)

    def test_uia_command_attaches_by_pid_and_selects_scenes(self):
        cmd = u.uia_command(r'C:\Temp\t.json')
        self.assertIn('-ProcessId {pid}', cmd)
        self.assertIn('-Page Scenes', cmd)
        self.assertIn('-Title Foxglove', cmd)
        self.assertNotIn('-Page', u.uia_command(r'C:\Temp\t.json', None))

    def test_page_missing_reads_uia_trees_words(self):
        self.assertTrue(u.page_missing("no sidebar entry named 'Scenes' that can be selected, invoked or toggled (elements of that name: )"))
        self.assertFalse(u.page_missing('timed out: the window did not answer within 20 s (hung?)'))

    def test_the_case_is_blocked_without_the_apps_ok_and_launches_nothing(self):
        env = {k: v for k, v in os.environ.items() if k != 'FOXGLOVE_APP_OK'}
        r = subprocess.run([sys.executable, str(LAB / 'uia_start.py'), '--app', sys.executable], env=env,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('RESULT BLOCKED', r.stdout)

    def test_the_script_never_stops_a_process_by_name(self):
        text = (LAB / 'uia_start.py').read_text(encoding='utf-8')
        code = text.split('"""', 2)[2]
        for banned in ('Stop-Process', 'taskkill', 'Get-Process', 'AllowRunning'):
            self.assertNotIn(banned, code)
        self.assertIsNone(re.search(r'(?<![.\w])kill\(', code), "labkit.kill() stops by name; only our own launcher handle may be killed")


if __name__ == '__main__':
    unittest.main()
