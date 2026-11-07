"""The sparse identity package (#179): makeappx accepts the manifest (schema only, unsigned).

Runs scripts/pack-identity.ps1 into a temp folder when the Windows SDK's makeappx is installed;
skipped otherwise (CI without the SDK). Nothing is signed, registered or installed.
"""
import glob
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def makeappx():
    base = os.environ.get('ProgramFiles(x86)')
    if os.name != 'nt' or not base:
        return None
    found = sorted(glob.glob(os.path.join(base, r'Windows Kits\10\bin\10.*\x64\makeappx.exe')))
    return found[-1] if found else None


class IdentityPackage(unittest.TestCase):
    @unittest.skipUnless(makeappx(), 'no Windows SDK makeappx')
    def test_the_unsigned_package_packs(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = subprocess.run(
                ['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
                 str(ROOT / 'scripts' / 'pack-identity.ps1'), '-Out', tmp],
                capture_output=True, text=True, timeout=120)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertTrue((Path(tmp) / 'foxglove-identity.msix').is_file())

    @unittest.skipUnless(makeappx(), 'no Windows SDK makeappx')
    def test_stage_puts_the_logos_next_to_the_exe(self):
        with tempfile.TemporaryDirectory() as tmp:
            exe_dir = Path(tmp) / 'release'
            exe_dir.mkdir()
            (exe_dir / 'foxglove.exe').write_bytes(b'')  # a stand-in: never run
            r = subprocess.run(
                ['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
                 str(ROOT / 'scripts' / 'pack-identity.ps1'), '-Out', str(Path(tmp) / 'out'),
                 '-Stage', str(exe_dir)],
                capture_output=True, text=True, timeout=120)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertTrue((exe_dir / 'foxglove-identity.msix').is_file())
            for logo in ('logo50.png', 'logo150.png', 'logo44.png'):
                self.assertTrue((exe_dir / 'Assets' / logo).is_file(), logo)

    def test_the_manifest_is_neutral(self):
        text = (ROOT / 'app' / 'packaging' / 'identity' / 'AppxManifest.xml').read_text(encoding='utf-8')
        self.assertIn('ProcessorArchitecture="neutral"', text)


if __name__ == '__main__':
    unittest.main()
