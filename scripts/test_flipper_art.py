"""Tests for flipper_art.py's art convention (#610): ink = a set bit, flagged in the blob header."""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flipper_art as fa  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURE = os.path.join(ROOT, "tests", "art", "ink-fixture")
STRIDE = (fa.ART_W + 7) // 8


def bit(data, x, y):
    return bool(data[y * STRIDE + x // 8] & (0x80 >> (x % 8)))


def frames_of(blob):
    """Unique frames of a blob, unpacked as the firmware's art_play does: from all-zero, whatever
    header byte 7 says (peripheral_status.c)."""
    unique = blob[8] | blob[9] << 8
    count = blob[10] | blob[11] << 8
    at = 12 + count
    offsets = [blob[at + 2 * i] | blob[at + 2 * i + 1] << 8 for i in range(unique)]
    buf = bytearray(STRIDE * fa.ART_H)
    out = []
    for i, start in enumerate(offsets):
        end = offsets[i + 1] if i + 1 < len(offsets) else len(blob)
        fa.unrle_xor(blob[start:end], buf)
        out.append(bytes(buf))
    return out


class InkConvention(unittest.TestCase):
    def setUp(self):
        sets = fa.load_input(FIXTURE)
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "out.bin")
            fa.write_blob(path, sets)
            with open(path, "rb") as f:
                self.blob = f.read()

    def test_the_fixture_blob_is_what_the_script_writes(self):
        # app/src/foxglove/art_encode.rs checks the same file: both encoders agree byte for byte.
        with open(FIXTURE + ".bin", "rb") as f:
            self.assertEqual(self.blob, f.read())

    def test_ink_is_a_set_bit_and_the_header_says_so(self):
        self.assertEqual(self.blob[7], fa.FLAG_INK)
        frame = frames_of(self.blob)[0]
        # upright (5, 10) is stored turned: column 139 - 10, row 5
        self.assertTrue(bit(frame, 139 - 10, 5), "black source = ink = set")
        self.assertFalse(bit(frame, 139 - 50, 50), "white source = field = clear")
        # 50% threshold: 127 is ink, 128 the field (upright rows 30 and 34, column 25)
        self.assertTrue(bit(frame, 139 - 30, 25))
        self.assertFalse(bit(frame, 139 - 34, 25))
        for y in range(fa.ART_H):
            self.assertEqual(frame[y * STRIDE + STRIDE - 1] & 0x0F, 0, "padding stays clear")


if __name__ == "__main__":
    unittest.main()
