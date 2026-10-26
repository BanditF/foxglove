import hashlib
import tempfile
import unittest
from pathlib import Path

from firmware.release_assets import assets


class ReleaseAssets(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.src = Path(self.tmp.name) / 'out'
        self.dest = Path(self.tmp.name) / 'release'
        self.src.mkdir()
        prefix = 'foxglove-fw-v0.2.0-1a2b3c4'
        for kind in ['left-dev', 'left-ga', 'right-dev', 'right-ga', 'settings-reset']:
            (self.src / f'{prefix}-{kind}.uf2').write_bytes(kind.encode())

    def tearDown(self):
        self.tmp.cleanup()

    def test_ships_the_ga_halves_and_the_settings_reset_with_their_sums(self):
        names = assets('0.2.0', self.src, self.dest)
        self.assertEqual(names, ['foxglove-0.2.0-left.uf2', 'foxglove-0.2.0-right.uf2',
                                 'foxglove-0.2.0-settings-reset.uf2', 'SHA256SUMS'])
        self.assertEqual((self.dest / 'foxglove-0.2.0-left.uf2').read_bytes(), b'left-ga', 'GA, not dev')
        self.assertEqual((self.dest / 'foxglove-0.2.0-settings-reset.uf2').read_bytes(), b'settings-reset')
        sums = (self.dest / 'SHA256SUMS').read_text(encoding='ascii').splitlines()
        self.assertEqual(len(sums), 3)
        for line in sums:
            digest, name = line.split('  ')
            self.assertEqual(digest, hashlib.sha256((self.dest / name).read_bytes()).hexdigest())
        self.assertIn('foxglove-0.2.0-settings-reset.uf2', [s.split('  ')[1] for s in sums])

    def test_a_missing_settings_reset_fails(self):
        next(self.src.glob('*-settings-reset.uf2')).unlink()
        with self.assertRaisesRegex(ValueError, 'settings-reset: 0 files'):
            assets('0.2.0', self.src, self.dest)
        self.assertFalse(self.dest.exists(), 'nothing written')

    def test_two_of_a_kind_fail(self):
        (self.src / 'foxglove-fw-v0.2.0-9z9z9z9-left-ga.uf2').write_bytes(b'other')
        with self.assertRaisesRegex(ValueError, 'left: 2 files'):
            assets('0.2.0', self.src, self.dest)


if __name__ == '__main__':
    unittest.main()
