"""Window-free test of scripts/lab/quiet-launch.ps1: runs its -SelfTest (arg validation, name rules,
quoting, env block, teardown order, P/Invoke compiles). Starts no desktop and no process of its own."""
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name('quiet-launch.ps1')
PROOF = Path(__file__).with_name('quiet-launch-proof.ps1')
NO_WINDOW = 0x08000000


@unittest.skipUnless(sys.platform == 'win32', 'Windows PowerShell only')
class QuietLaunchSelfTest(unittest.TestCase):
    def test_selftest(self):
        r = subprocess.run(['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(SCRIPT), '-SelfTest'],
                           capture_output=True, text=True, timeout=120, creationflags=NO_WINDOW)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn('self-test: ok', r.stdout)

    def test_proof_parses(self):
        # Parse only: the proof opens a window and is run by On-call.
        cmd = ("$e = $null; [void][System.Management.Automation.Language.Parser]::ParseFile("
               f"'{PROOF}', [ref]$null, [ref]$e); exit $e.Count")
        r = subprocess.run(['powershell.exe', '-NoProfile', '-Command', cmd], capture_output=True, text=True,
                           timeout=60, creationflags=NO_WINDOW)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)


if __name__ == '__main__':
    unittest.main()
