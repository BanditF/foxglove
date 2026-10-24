"""Synthetic UF2s for check_hooks.py."""
import struct
import tempfile
import unittest
from pathlib import Path

from firmware import check_hooks as c


def uf2(payload):
    blocks = []
    for n, i in enumerate(range(0, max(len(payload), 1), 256)):
        chunk = payload[i:i + 256]
        blocks.append(struct.pack('<8I', 0x0A324655, 0x9E5D5157, 0x2000, 0x26000 + i, len(chunk),
                                  n, 0, 0xADA52840) + chunk.ljust(476, b'\0') +
                      struct.pack('<I', 0x0AB16F30))
    return b''.join(blocks)


class CheckHooks(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.dir.cleanup()

    def file(self, name, payload):
        path = Path(self.dir.name) / name
        path.write_bytes(uf2(payload))
        return str(path)

    def test_ga_clean_and_dev_complete(self):
        ga = self.file('ga.uf2', b'FOXGLOVE_HALF=left;PROFILE=ga')
        dev = self.file('dev.uf2', b'x' * 300 + b'FOXGLOVE_TEST_HOOK=position\0'
                        b'FOXGLOVE_TEST_HOOK=bootloader\0')
        self.assertEqual(c.check([ga], [(dev, {'position', 'bootloader'})]), [])

    def test_ga_with_a_hook_fails(self):
        ga = self.file('ga.uf2', b'FOXGLOVE_TEST_HOOK=guard_test')
        problems = c.check([ga], [])
        self.assertEqual(len(problems), 1)
        self.assertIn('guard_test', problems[0])

    def test_marker_across_a_block_boundary(self):
        payload = b'y' * 250 + b'FOXGLOVE_TEST_HOOK=position'  # starts in block 0, ends in 1
        self.assertEqual(c.hooks(uf2(payload)), {'position'})

    def test_dev_missing_a_hook_fails(self):
        dev = self.file('dev.uf2', b'FOXGLOVE_TEST_HOOK=position')
        problems = c.check([], [(dev, {'position', 'bootloader'})])
        self.assertIn('bootloader', problems[0])

    def test_main_exit_codes(self):
        ga = self.file('ga.uf2', b'clean')
        self.assertEqual(c.main(['--ga', ga]), 0)
        bad = self.file('bad.uf2', b'FOXGLOVE_TEST_HOOK=position')
        self.assertEqual(c.main(['--ga', bad]), 1)


if __name__ == '__main__':
    unittest.main()
