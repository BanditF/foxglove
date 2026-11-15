"""Every chip matches its effect's visual promise and hsv_hex maps the HSV grid correctly."""

import sys
import unittest

sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent))

import lighting_presets as lp  # noqa: E402


class HsvHex(unittest.TestCase):
    def test_primary_colors(self):
        self.assertEqual(lp.hsv_hex(0, 1.0), "#ff0000")
        self.assertEqual(lp.hsv_hex(120, 1.0), "#00ff00")
        self.assertEqual(lp.hsv_hex(240, 1.0), "#0000ff")

    def test_bringit_orange(self):
        # Bandit hue/saturation (22 deg, 92%) at full value
        self.assertEqual(lp.hsv_hex(22, 92 / 100), "#ff6a14")


class Chip(unittest.TestCase):
    def test_chip_solid_uses_direct_fill(self):
        svg = lp.chip({"swatch": "#f08a3c", "effect": "solid"})
        self.assertIn("fill=\"#f08a3c\"", svg)
        self.assertNotIn("<linearGradient", svg)

    def test_chip_breathe_has_gradient_fade(self):
        svg = lp.chip({"swatch": "#c8342b", "effect": "breathe"})
        self.assertIn('fill="url(#g)"', svg)
        self.assertIn("stop-opacity=\"0.25\"", svg)
        # Both ends fade, middle does not
        self.assertEqual(svg.count("stop-opacity=\"0.25\""), 2)

    def test_chip_swirl_has_rainbow_stops(self):
        svg = lp.chip({"swatch": "#f08a3c", "effect": "swirl"})
        self.assertIn('fill="url(#g)"', svg)
        # 7 stops for 6 segments: offsets 0, 1/6, 2/6, … 1
        stops = [s for s in svg.split("<stop ") if "offset" in s]
        self.assertEqual(len(stops), 7)

    def test_chip_contains_svg_structure(self):
        svg = lp.chip({"swatch": "#00ff00", "effect": "solid"})
        self.assertIn('<svg xmlns="http://www.w3.org/2000/svg"', svg)
        self.assertIn('width="64" height="24"', svg)


if __name__ == "__main__":
    unittest.main()
