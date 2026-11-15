"""Tests for pure helper functions in plugin_panels (no GUI, network, keyboard, or file I/O)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import plugin_panels as pp  # noqa: E402


class HelperFunctions(unittest.TestCase):
    def test_text35_w_calculation(self):
        self.assertEqual(pp.text35_w("A"), 3)
        self.assertEqual(pp.text35_w(""), -1)
        self.assertEqual(pp.text35_w("HI"), 7)

    def test_text57x2_w_with_gap(self):
        self.assertEqual(pp.text57x2_w("X", gap=0), 10)
        self.assertEqual(pp.text57x2_w("AB", gap=2), 22)
        self.assertEqual(pp.text57x2_w("", gap=4), -4)

    def test_sprite_ascii_art(self):
        s = pp.sprite(["..#....", ".####.#"])
        w, h = s.size
        px = s.load()
        # width is longest row (7 chars -> 7 cols), height is number of rows (2)
        self.assertEqual(w, 7)
        self.assertEqual(h, 2)
        self.assertEqual(px[0, 0], 0)
        self.assertEqual(px[2, 0], 1)
        self.assertEqual(px[3, 1], 1)

    def test_pomodoro_bar_rounds_progress(self):
        # inner = b["w"] - 4 = 56; fill = round(56 * progress)
        img0 = pp.pomodoro_frame("24:59", "focus", 0.0)
        px0 = img0.load()
        # outline only draws border pixels, so px[2,40] (left edge of bar) is set
        self.assertEqual(px0[2, 40], 1)

        img1 = pp.pomodoro_frame("24:59", "focus", 1.0)
        px1 = img1.load()
        # fill covers all inner pixels; rightmost filled pixel at x=4+56-1=59, y=40
        self.assertEqual(px1[4 + 56 - 1, 40], 1)



if __name__ == "__main__":
    unittest.main()
