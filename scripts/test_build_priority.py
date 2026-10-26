"""Builds run at BelowNormal priority (build-slot.ps1): the slot's process drops to it, what it
starts inherits it, and leaving the slot puts the old class back. Window-free: hidden
PowerShell processes only, and no build slot is taken."""
import subprocess
import sys
import unittest
from pathlib import Path

SLOT = Path(__file__).resolve().parent / 'build-slot.ps1'

SCRIPT = r'''
$ErrorActionPreference = 'Stop'
. '{slot}'
$me = Get-Process -Id $PID
$me.PriorityClass = 'Normal'
$was = Set-BuildPriority
$me.Refresh()
$own = $me.PriorityClass
$child = powershell -NoProfile -NonInteractive -Command '(Get-Process -Id $PID).PriorityClass.ToString()'
Restore-BuildPriority $was
$me.Refresh()
"$was,$own,$child,$($me.PriorityClass)"
'''


@unittest.skipUnless(sys.platform == 'win32', 'Windows priority classes')
class BuildPriority(unittest.TestCase):
    def test_slot_lowers_itself_and_its_children_then_restores(self):
        out = subprocess.run(
            ['powershell', '-NoProfile', '-NonInteractive', '-Command', SCRIPT.replace('{slot}', str(SLOT))],
            capture_output=True, text=True, timeout=120, creationflags=subprocess.CREATE_NO_WINDOW)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(out.stdout.strip().splitlines()[-1], 'Normal,BelowNormal,BelowNormal,Normal')


if __name__ == '__main__':
    unittest.main()
