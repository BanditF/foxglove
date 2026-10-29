"""Tests for flipper_art.py's art convention (#610): ink = a set bit, flagged in the blob header."""

import os
import re
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flipper_art as fa  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURE = os.path.join(ROOT, "tests", "art", "ink-fixture")
BUILTINS = os.path.join(ROOT, "boards", "shields", "nice_view_custom", "widgets", "custom_art.c")
STRIDE = (fa.ART_W + 7) // 8


def bit(data, x, y):
    return bool(data[y * STRIDE + x // 8] & (0x80 >> (x % 8)))


def frames_of(blob):
    """Unique frames of a blob, unpacked as the firmware's art_play does: from all-zero for an
    ink-flagged blob, from all-0xff for one from before #610 (peripheral_status.c set_base)."""
    unique = blob[8] | blob[9] << 8
    count = blob[10] | blob[11] << 8
    at = 12 + count
    offsets = [blob[at + 2 * i] | blob[at + 2 * i + 1] << 8 for i in range(unique)]
    base = 0x00 if blob[7] & fa.FLAG_INK else 0xFF
    buf = bytearray([base]) * (STRIDE * fa.ART_H)
    out = []
    for i, start in enumerate(offsets):
        end = offsets[i + 1] if i + 1 < len(offsets) else len(blob)
        fa.unrle_xor(blob[start:end], buf)
        out.append(bytes(buf))
    return out


def legacy_blob(blob):
    """The blob as an app before #610 wrote it: flags 0 and every frame's art bits inverted. Only
    delta 0 changes (an XOR of two inverted frames is the XOR of the originals)."""
    count = blob[10] | blob[11] << 8
    at = 12 + count
    first = blob[at] | blob[at + 1] << 8
    second = blob[at + 2] | blob[at + 3] << 8 if blob[8] > 1 else len(blob)
    buf = bytearray(STRIDE * fa.ART_H)
    fa.unrle_xor(blob[first:second], buf)
    for y in range(fa.ART_H):
        for x in range(fa.ART_W):
            buf[y * STRIDE + x // 8] ^= 0x80 >> (x % 8)
    delta0 = fa.rle(bytes(buf))
    head = bytearray(blob[:first])
    head[7] = 0
    shift = len(delta0) - (second - first)
    for i in range(1, blob[8] | blob[9] << 8):
        o = head[at + 2 * i] | head[at + 2 * i + 1] << 8
        o += shift
        head[at + 2 * i], head[at + 2 * i + 1] = o & 0xFF, o >> 8
    return bytes(head) + delta0 + blob[second:]


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

    def test_a_blob_from_before_610_plays_the_same(self):
        old = legacy_blob(self.blob)
        self.assertEqual(old[7], 0)
        self.assertNotEqual(old, self.blob)
        self.assertEqual(frames_of(old), [
            bytes(b | (0x0F if i % STRIDE == STRIDE - 1 else 0) for i, b in enumerate(f))
            for f in frames_of(self.blob)
        ], "inverted from 0xff, only the 4 padding bits per row differ")


class BuiltIns(unittest.TestCase):
    def test_built_in_art_is_ink_bits(self):
        src = open(BUILTINS, encoding="utf-8").read()
        names = re.findall(r'\.name = "(\w+)"', src)
        for si, name in enumerate(names):
            body = re.search(
                r"set_%d_delta_0\[\] = \{(.*?)\};" % si, src, re.S
            ).group(1)
            delta = bytes(int(t, 16) for t in re.findall(r"0x([0-9a-f]{2})", body))
            buf = bytearray(STRIDE * fa.ART_H)
            fa.unrle_xor(delta, buf)
            for y in range(fa.ART_H):
                self.assertEqual(buf[y * STRIDE + STRIDE - 1] & 0x0F, 0, "set %d padding" % si)
            if name.startswith("Bandit_"):
                # The Bandit HUD scenes are line work on a dark field (about 13% lit, art-src
                # notes): a set stored the old way round (field bits set) fails here.
                ink = sum(bin(b).count("1") for b in buf)
                self.assertLess(ink, fa.ART_W * fa.ART_H * 35 // 100, "%s is ink bits" % name)


if __name__ == "__main__":
    unittest.main()
