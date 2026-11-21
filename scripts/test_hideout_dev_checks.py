"""Window-free tests for scripts/lab/hideout_dev_checks.py: the words match dev_settings.rs, the seed
and the launcher script are well formed, and nothing is launched.

    python -m unittest scripts/test_hideout_dev_checks.py
"""
import base64
import os
import re
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
sys.path.insert(0, str(HERE / 'lab'))
import hideout_dev_checks as dc  # noqa: E402
# scripts/lab has same-named test files (test_hideout_footprint.py): discovery of the later scripts/
# tests would import the lab one if its folder stayed on the path.
sys.path[:] = [p for p in sys.path if p != str(HERE / 'lab')]

RS = HERE.parent / 'crates' / 'hideout-shell' / 'src' / 'dev_settings.rs'


def rust_consts():
    out = {}
    for m in re.finditer(r'pub const (\w+): &str =\s*((?:"(?:[^"\\]|\\.)*"\s*\\?\s*)+);', RS.read_text(encoding='utf-8')):
        parts = re.findall(r'"((?:[^"\\]|\\.)*)"', m.group(2))
        out[m.group(1)] = ''.join(parts).replace('\\u{2026}', '\u2026')
    return out


class Words(unittest.TestCase):
    def test_match_dev_settings_rs(self):
        rs = rust_consts()
        for name, text in dc.WORDS.items():
            self.assertEqual(rs.get(name), text, name)

    def test_remove_prefix_matches_accessible_name(self):
        self.assertIn('Remove dev folder', RS.read_text(encoding='utf-8'))


class Seed(unittest.TestCase):
    def test_toml_literal_keeps_backslashes(self):
        self.assertEqual(dc.seed_config(r'C:\Dev\app'), "[dev]\nfolders = ['C:\\Dev\\app']\n")

    def test_empty_seed(self):
        self.assertEqual(dc.seed_config(None), '[dev]\nfolders = []\n')

    def test_quote_refused(self):
        with self.assertRaises(ValueError):
            dc.toml_literal("C:\\it's")


class Launcher(unittest.TestCase):
    def test_script_has_real_hashtable_and_limit(self):
        s = dc.launch_script(r'C:\x\hideout.exe', r'C:\lab', 'ql-dfa1', {'HIDEOUT_LAB': '1'}, 240)
        self.assertIn('-Env @{ HIDEOUT_LAB = \'1\' }', s)
        self.assertIn('-TimeoutSec 240', s)
        self.assertIn("-DesktopName 'ql-dfa1'", s)
        self.assertNotIn('-AllowRunning', s)
        self.assertIn('-AllowRunning', dc.launch_script('a', 'b', 'c', {}, 1, allow_running=True))

    def test_stop_by_desktop_name(self):
        self.assertTrue(dc.stop_script('ql-dfa1').endswith("-Stop 'ql-dfa1'"))

    def test_quote_in_path_refused(self):
        with self.assertRaises(ValueError):
            dc.ps_quote("a'b")

    def test_curly_quote_refused(self):
        with self.assertRaises(ValueError):
            dc.ps_quote('a’b')

    def test_encoded_round_trip(self):
        self.assertEqual(base64.b64decode(dc.encoded('x y')).decode('utf-16-le'), 'x y')


class Nodes(unittest.TestCase):
    def test_remove_button_found(self):
        state = {'main': {'nodes': [{'name': 'Dev folders'}, {'name': r'Remove dev folder C:\Dev\app'}]}}
        self.assertEqual(dc.remove_button(state), (1, r'Remove dev folder C:\Dev\app'))
        self.assertEqual(dc.remove_button({'main': {'nodes': []}}), (None, None))


class Gate(unittest.TestCase):
    def test_blocked_without_app_ok(self):
        import subprocess
        env = {k: v for k, v in os.environ.items() if k != 'FOXGLOVE_APP_OK'}
        r = subprocess.run([sys.executable, str(HERE / 'lab' / 'hideout_dev_checks.py'), '--app', sys.executable,
                            '--lab', str(HERE), '--folder', str(HERE)], env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('RESULT BLOCKED', r.stdout)


if __name__ == '__main__':
    unittest.main()
