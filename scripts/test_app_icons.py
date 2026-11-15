"""Pure helpers in app_icons: mirror, shrink, fox_layers, tile_geometry."""

import sys
import unittest
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
