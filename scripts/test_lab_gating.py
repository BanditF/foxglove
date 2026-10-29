"""The lab cases that launch Foxglove refuse to run without On-call's OK (#618): the soak and the
overlay/tray cases exit 2 (BLOCKED) before anything starts when FOXGLOVE_APP_OK isn't 1, and every
labkit.App does the same. Nothing here launches anything.

    python -m unittest scripts/test_lab_gating.py
"""
import os
import subprocess
import sys
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'


def run(script, *args):
    env = {k: v for k, v in os.environ.items() if k != 'FOXGLOVE_APP_OK'}
    env.setdefault('APPDATA', str(Path.home()))
    env.setdefault('USERNAME', 'test')
    return subprocess.run([sys.executable, str(LAB / script), *args], capture_output=True, text=True,
                          env=env, cwd=LAB, timeout=60)


class AppLaunchesNeedOnCall(unittest.TestCase):
    def test_the_soak_refuses_without_the_ok(self):
        r = run('soak.py', '--app', 'C:/nowhere/foxglove.exe')
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('FOXGLOVE_APP_OK=1', r.stdout)

    def test_the_overlay_and_tray_cases_refuse_without_the_ok(self):
        r = run('overlay_tray.py', '--app', 'C:/nowhere/foxglove.exe')
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('RESULT BLOCKED', r.stdout)

    def test_every_app_run_needs_the_ok(self):
        os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
        sys.path.insert(0, str(LAB))
        try:
            import labkit
        finally:
            sys.path.pop(0)
        saved = os.environ.pop('FOXGLOVE_APP_OK', None)
        try:
            with self.assertRaises(SystemExit) as e:
                with labkit.App('C:/nowhere/foxglove.exe', Path('.')):
                    self.fail('the app would have started')
            self.assertEqual(e.exception.code, 2)
        finally:
            if saved is not None:
                os.environ['FOXGLOVE_APP_OK'] = saved


if __name__ == '__main__':
    unittest.main()
