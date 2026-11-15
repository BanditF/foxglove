"""Every pure helper in plugin_icons produces correct SVG fragments for the Plugins tab icons."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import plugin_icons as pi  # noqa: E402


class ArcOutput(unittest.TestCase):
    def test_arc_is_valid_path_with_correct_large_arc_flag(self):
        short = pi.arc(12, 13, 5, 0, 90)     # 90° < 180°, large-arc flag=0
        long_ = pi.arc(12, 13, 5, 0, 270)    # 270° > 180°, large-arc flag=1
        self.assertIn('<path d="M', short)
        self.assertIn('A5 5 0 0 1', short)
        self.assertIn('<path d="M', long_)
        self.assertIn('A5 5 0 1 1', long_)


class ColourLookup(unittest.TestCase):
    def test_rgb_role_returns_url_gradient_and_known_roles_return_palette_value(self):
        self.assertEqual(pi.colour("rgb", {"r": "#ff0"}), "url(#rgb)")
        pal = pi.DARK
        self.assertEqual(pi.colour("ink", pal), pal["ink"])
        self.assertEqual(pi.colour("accent", pal), pal["accent"])
        self.assertEqual(pi.colour("r", pal), pal["r"])


class GlyphBodyAndSvg(unittest.TestCase):
    def test_glyph_body_applies_palette_colours_to_styled_g_elements(self):
        parts = [("ink", '<path d="M1 1 H2"/>'), ("accent", '<rect x="3" y="3" width="4" height="4"/>')]
        body = pi.glyph_body(parts, pal=pi.DARK)
        self.assertIn('stroke="#ece8e1"', body)   # ink from DARK
        self.assertIn('stroke="#ff7a1a"', body)    # accent from DARK

    def test_glyph_svg_has_size_and_stroke_attributes(self):
        parts = [("ink", '<circle cx="12" cy="12" r="5"/>')]
        svg = pi.glyph_svg(parts, size=48)
        self.assertIn('width="48"', svg)
        self.assertIn('height="48"', svg)
        self.assertIn(pi.STROKE, svg)

    def test_tile_svg_contains_gradient_definition_brackets_scaled_glyph(self):
        parts = [("ink", '<rect x="5" y="5" width="10" height="10"/>')]
        svg = pi.tile_svg(parts, pi.DARK)
        self.assertIn('<linearGradient id="rgb"', svg)
        self.assertIn('rx="10"', svg)
        self.assertIn(pi.DARK["bracket"], svg)


class GradientDef(unittest.TestCase):
    def test_gradient_defines_three_colour_stops(self):
        svg = pi.gradient({"r": "#f00", "g": "#0f0", "b": "#00f"})
        self.assertEqual(svg.count('<stop'), 3)
        self.assertIn('#f00', svg)
        self.assertIn('#00f', svg)


if __name__ == "__main__":
    unittest.main()
