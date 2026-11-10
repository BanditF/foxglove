"""The Hideout lab checks' verdicts (#1003): flicker, flyout placement, drawn texts, manifests and
the FOXGLOVE_APP_OK gate. Window-free: nothing is launched and no pipe is opened.

    python -m unittest scripts/test_hideout_lab_checks.py
"""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'
os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
sys.path.insert(0, str(LAB))
import hideout_lab_checks as hl  # noqa: E402

STATE = {'main': {'nodes': [
    {'role': 'Tab', 'name': 'Focus timer, running', 'selected': True},
    {'role': 'Label', 'value': "Corsair's iCUE SDK file (iCUESDK.x64_2019.dll)"},
    {'role': 'TextInput', 'value': ''},
    {'role': 'Button', 'name': 'Choose file'},
]}, 'tray': {'menu': [{'text': 'Hideout', 'enabled': False}, {'text': 'Open Hideout', 'enabled': True}]}}


class Verdicts(unittest.TestCase):
    def test_flicker_needs_no_visible_flyout_frame_and_the_main_window(self):
        quiet = {'rows': [{'t': 10, 'w': 'main', 'flyout_visible': False}], 'counts': {'flyout': 2}}
        self.assertTrue(hl.flicker_verdict(quiet, True)[0])
        self.assertFalse(hl.flicker_verdict(quiet, False)[0])
        flash = {'rows': [{'t': 10, 'flyout_visible': True}, {'t': 160, 'flyout_visible': True}], 'counts': {}}
        ok, why = hl.flicker_verdict(flash, True)
        self.assertFalse(ok)
        self.assertIn('150 ms', why)

    def test_the_flyout_sits_against_the_taskbar_inside_the_work_area(self):
        mon, work = [0, 0, 1920, 1080], [0, 0, 1920, 1032]  # taskbar at the bottom
        self.assertTrue(hl.against_taskbar({'rect': [1500, 700, 1860, 1020], 'monitor': mon, 'work': work})[0])
        self.assertFalse(hl.against_taskbar({'rect': [1500, 300, 1860, 600], 'monitor': mon, 'work': work})[0])
        self.assertFalse(hl.against_taskbar({'rect': [1500, 800, 1860, 1070], 'monitor': mon, 'work': work})[0])
        top = [0, 48, 1920, 1080]
        self.assertTrue(hl.against_taskbar({'rect': [1500, 56, 1860, 400], 'monitor': mon, 'work': top})[0])
        self.assertFalse(hl.against_taskbar(None)[0])

    def test_texts_fields_and_the_tray_line(self):
        self.assertTrue(hl.has(STATE, 'Focus timer', prefix=True))
        self.assertTrue(hl.contains(STATE, 'iCUE SDK'))
        self.assertEqual(hl.field_after(STATE, "Corsair's iCUE SDK file (iCUESDK.x64_2019.dll)"), 2)
        self.assertIsNone(hl.field_after(STATE, 'Nothing'))
        self.assertEqual(hl.tray_status(STATE), 'Hideout')
        self.assertIsNone(hl.tray_status({'tray': {'menu': None}}))

    def test_manifest_names_and_exe_come_from_the_lab_folder(self):
        with tempfile.TemporaryDirectory() as d:
            m = Path(d) / 'lab-modules' / 'org.foxglove.focus-timer'
            m.mkdir(parents=True)
            (m / 'module.toml').write_text('id = "org.foxglove.focus-timer"\nname = "Focus timer"\n'
                                           'run = ["focus-timer.exe"]\n', encoding='utf-8')
            self.assertEqual(hl.manifest_names(d), {'org.foxglove.focus-timer': 'Focus timer'})
            self.assertEqual(hl.module_exe(d, 'org.foxglove.focus-timer'), m / 'focus-timer.exe')
            self.assertIsNone(hl.module_exe(d, 'org.foxglove.none'))

    def test_it_refuses_without_on_calls_ok(self):
        env = {k: v for k, v in os.environ.items() if k != 'FOXGLOVE_APP_OK'}
        r = subprocess.run([sys.executable, str(LAB / 'hideout_lab_checks.py'), '--app', sys.executable,
                            '--lab', str(LAB)], capture_output=True, text=True, env=env, cwd=LAB, timeout=60)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('RESULT BLOCKED', r.stdout)


if __name__ == '__main__':
    unittest.main()
