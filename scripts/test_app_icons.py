"""Pure helpers in app_icons: mirror, shrink, fox_layers, tile_geometry."""

import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image, ImageChops, ImageFilter

sys.path.insert(0, str(Path(__file__).resolve().parent))

import app_icons as ai  # noqa: E402


class TestMirror(unittest.TestCase):
    def test_reflects_across_vertical_center(self):
        p = [(10, 5), (30, 20)]
        m = ai.mirror(p)
        self.assertEqual(m[0], (ai.FW - 10, 5))
        self.assertEqual(m[1], (ai.FW - 30, 20))

    def test_double_mirror_returns_original(self):
        p = [(100, 200), (400, 600)]
        self.assertEqual(ai.mirror(ai.mirror(p)), p)


class TestShrink(unittest.TestCase):
    def test_shrink_moves_toward_centroid(self):
        # Square centred at (100,100); factor 0.5 halves the distance to centre.
        sq = [(0, 0), (200, 0), (200, 200), (0, 200)]
        shrunk = ai.shrink(sq, 0.5)
        # Corner at x=0 moves to 100 + (-100)*0.5 = 50
        self.assertAlmostEqual(shrunk[0][0], 50)
        self.assertAlmostEqual(shrunk[2][0], 150)

    def test_factor_one_is_identity(self):
        p = [(10, 20), (30, 40)]
        self.assertEqual(ai.shrink(p, 1.0), p)


class TestFoxLayers(unittest.TestCase):
    def test_simple_has_fewer_layers_than_full(self):
        simple_count = len(ai.fox_layers(simple=True))
        full_count = len(ai.fox_layers(simple=False))
        self.assertLess(simple_count, full_count)
        # full has inner ears (2 shrunk dark) + eyes (2 cream) = 4 more layers
        self.assertEqual(full_count - simple_count, 4)



class TestTileGeometry(unittest.TestCase):
    def test_returns_positive_scale_within_bounds(self):
        scale, ox, oy = ai.tile_geometry(256, False)
        self.assertGreater(scale, 0)
        self.assertGreaterEqual(ox, -1)
        self.assertGreaterEqual(oy, -1)


def _blocks(mask, size, whole=False):
    """The output pixels a supersampled L mask touches (whole: covers completely) as a set."""
    small = mask.resize((size, size), Image.BOX)
    limit = 254 if whole else 0
    return {(x, y) for y in range(size) for x in range(size) if small.getpixel((x, y)) > limit}


def _badge_pixels(img, variant):
    """The output pixels that are the badge's orange at full alpha."""
    orange = ai.TRAY_COLOURS[variant]["badge"]
    return {(x, y) for y in range(img.height) for x in range(img.width)
            if img.getpixel((x, y)) == orange + (255,)}


class TestTrayEye(unittest.TestCase):
    """The Screen glow eye (#1085): an almond rim with a pupil, no disc."""

    def test_the_eye_has_no_half_pixels_at_16_and_20(self):
        for variant in ("white", "black"):
            for size in (16, 20):
                img = ai.tray(size, variant, "screen")
                foot = ai.eye_parts(size)[0]
                for x, y in _blocks(foot, size):
                    self.assertIn(img.getpixel((x, y))[3], (0, 255), (variant, size, x, y))

    def test_the_eye_is_rim_gap_pupil(self):
        for variant in ("white", "black"):
            for size in (16, 20):
                img = ai.tray(size, variant, "screen")
                rows = ai.EYE_MAPS[size]
                w, h = len(rows[0]), len(rows)
                y = size - h + h // 2
                runs = []
                for x in range(size - w, size):
                    on = img.getpixel((x, y))[3] == 255
                    if not runs or runs[-1] != on:
                        runs.append(on)
                self.assertEqual(runs, [True, False, True, False, True], (variant, size))

    def test_the_eye_is_wider_than_tall_and_pause_is_round(self):
        for size in (16, 20, 24, 32, 44):
            _, rim, pupil = ai.eye_parts(size)
            x0, y0, x1, y1 = rim.getbbox()
            self.assertGreaterEqual((x1 - x0) / (y1 - y0), 1.4, size)
        for size in (16, 20, 24, 32):
            pause = _badge_pixels(ai.tray(size, "white", "paused"), "white")
            xs, ys = [p[0] for p in pause], [p[1] for p in pause]
            ratio = (max(xs) - min(xs) + 1) / (max(ys) - min(ys) + 1)
            self.assertAlmostEqual(ratio, 1.0, delta=0.15, msg=size)
        eye = _badge_pixels(ai.tray(16, "white", "screen"), "white")
        pause = _badge_pixels(ai.tray(16, "white", "paused"), "white")
        both = eye | pause
        xs, ys = [p[0] for p in both], [p[1] for p in both]
        area = (max(xs) - min(xs) + 1) * (max(ys) - min(ys) + 1)
        self.assertGreaterEqual(len(eye ^ pause) / area, 0.40)

    def test_the_fox_keeps_its_gap(self):
        for variant in ("white", "black"):
            for size in ai.TRAY_SIZES:
                img = ai.tray(size, variant, "screen")
                foot = ai.eye_parts(size)[0]
                gap = 1.0 if size <= 24 else 1.5
                ring = ImageChops.subtract(foot.filter(ImageFilter.MaxFilter(int(2 * gap * 16 + 1))), foot)
                cover = ring.resize((size, size), Image.BOX)
                near = foot.resize((size, size), Image.BOX)
                deep = 0
                for y in range(size):
                    for x in range(size):
                        c = cover.getpixel((x, y)) / 255
                        if near.getpixel((x, y)) == 0:  # no eye here: only the fox could be
                            self.assertLessEqual(img.getpixel((x, y))[3], 255 * (1 - c) + 4, (variant, size, x, y))
                        deep += c > 0.5
                self.assertTrue(deep, (variant, size))

    def test_only_screen_files_change(self):
        saved = ai.RES
        try:
            with tempfile.TemporaryDirectory() as tmp:
                ai.RES = Path(tmp)
                ai.write_tray()
                for f in sorted(Path(tmp).glob("tray-icon-*.png")):
                    if "-screen" in f.name:
                        continue
                    kept = Image.open(saved / f.name).convert("RGBA")
                    self.assertEqual(Image.open(f).convert("RGBA").tobytes(), kept.tobytes(), f.name)
        finally:
            ai.RES = saved


if __name__ == "__main__":
    unittest.main()
