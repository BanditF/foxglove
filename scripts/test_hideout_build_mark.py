"""scripts/hideout_build_mark.py (#1466): only the build without lab switches passes, and the
marker the script looks for is the one the source writes.

    python -m unittest scripts/test_hideout_build_mark.py
"""
import re
import tempfile
import unittest
from pathlib import Path

import hideout_build_mark as mark

REPO = Path(__file__).resolve().parent.parent


class BuildMark(unittest.TestCase):
    def test_only_the_off_build_passes(self):
        self.assertIsNone(mark.problem(b'\x00MZ' + mark.OFF + b'\x00rest'))
        self.assertIn('lab-switches on', mark.problem(b'MZ' + mark.ON + b'\x00'))
        self.assertIn('lab-switches on', mark.problem(mark.OFF + b'\x00' + mark.ON))
        self.assertIn('no build marker', mark.problem(b'MZ nothing here'))
        self.assertIn('no build marker', mark.problem(b''))

    def test_the_exit_code_follows(self):
        with tempfile.TemporaryDirectory() as d:
            ok, bad = Path(d) / 'ok.exe', Path(d) / 'bad.exe'
            ok.write_bytes(b'MZ' + mark.OFF)
            bad.write_bytes(b'MZ' + mark.ON)
            self.assertEqual(mark.main(['x', str(ok)]), 0)
            self.assertEqual(mark.main(['x', str(bad)]), 1)
            self.assertEqual(mark.main(['x', str(Path(d) / 'missing.exe')]), 1)
            self.assertEqual(mark.main(['x']), 1)

    def test_the_source_writes_these_markers(self):
        lab = (REPO / 'crates/hideout-registry/src/lab.rs').read_text(encoding='utf-8')
        found = dict(re.findall(
            r'#\[cfg\((not\(feature = "lab-switches"\)|feature = "lab-switches")\)\]\s*'
            r'pub const BUILD_MARK: &str = "([^"]+)";', lab))
        self.assertEqual(found.get('feature = "lab-switches"').encode(), mark.ON)
        self.assertEqual(found.get('not(feature = "lab-switches")').encode(), mark.OFF)

    def test_the_installer_build_checks_the_exe_before_packing(self):
        ps = (REPO / 'scripts/build-hideout-installer.ps1').read_text(encoding='utf-8')
        self.assertLess(ps.index('hideout_build_mark.py'), ps.index('candle.exe -nologo'))
        self.assertIn('--no-default-features', ps)


if __name__ == '__main__':
    unittest.main()
