"""The Bandit Motion set as it ships (app/resources/art/bandit-motion/): every blob passes the
photosensitivity check of tools/art-src/bandit/motion/check.py and fits the push budget, and the
generator in tools/art-src/bandit still runs and makes the same pieces."""

import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
MOTION = os.path.join(ROOT, "tools", "art-src", "bandit", "motion")
BLOBS = os.path.join(ROOT, "app", "resources", "art", "bandit-motion")
sys.path.insert(0, HERE)
sys.path.insert(0, MOTION)
import check  # noqa: E402
import flipper_art as fa  # noqa: E402

STRIDE = (fa.ART_W + 7) // 8


def upright_frames(blob):
    """The blob's frames in play order, upright (68 x 140) rows of booleans. Stored frames are
    turned: upright (x, y) is stored row x, column 139 - y."""
    unique = blob[8] | blob[9] << 8
    count = blob[10] | blob[11] << 8
    order = list(blob[12:12 + count])
    at = 12 + count
    offsets = [blob[at + 2 * i] | blob[at + 2 * i + 1] << 8 for i in range(unique)]
    buf = bytearray(STRIDE * fa.ART_H)
    frames = []
    for i, start in enumerate(offsets):
        end = offsets[i + 1] if i + 1 < len(offsets) else len(blob)
        fa.unrle_xor(blob[start:end], buf)
        frames.append([[bool(buf[x * STRIDE + (139 - y) // 8] & (0x80 >> ((139 - y) % 8)))
                        for x in range(check.W)] for y in range(check.H)])
    return [frames[i] for i in order]


def blobs():
    for name in sorted(os.listdir(BLOBS)):
        with open(os.path.join(BLOBS, name), "rb") as f:
            yield name, f.read()


class ShippedMotionSet(unittest.TestCase):
    def test_every_piece_passes_the_photosensitivity_check(self):
        for name, blob in blobs():
            with self.subTest(name):
                self.assertEqual(blob[:4], b"NVA1")
                self.assertTrue(blob[7] & fa.FLAG_INK)
                worst, blocks = check.steps(upright_frames(blob))
                self.assertTrue(check.passes(worst, blocks), "%s: %d px, %d blocks" % (name, worst, blocks))

    def test_every_piece_fits_the_budget(self):
        for name, blob in blobs():
            self.assertLessEqual(len(blob), 12 * 1024, name)


class Generator(unittest.TestCase):
    def test_it_makes_every_shipped_piece(self):
        with tempfile.TemporaryDirectory() as d:
            subprocess.run([sys.executable, "-I", os.path.join(MOTION, "bandit_motion.py"), d],
                           check=True)
            made = {n.split("_", 1)[1].replace("_", "-") for n in os.listdir(d)}
            shipped = {n[len("bandit-"):-len(".bin")] for n, _ in blobs()}
            self.assertEqual(made, shipped)
            for n in os.listdir(d):
                seq, _ = check.frames(d, n)
                self.assertTrue(check.passes(*check.steps(seq)), n)


if __name__ == "__main__":
    unittest.main()
