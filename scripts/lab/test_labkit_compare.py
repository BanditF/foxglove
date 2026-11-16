"""labkit's pure comparison helpers: same(a, b) and hue_is(hue, tolerance).

These are what the lab scripts (pomodoro, leases) use to check underglow state, but the tests
exercise labkit directly, not those scripts.
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import labkit as k  # noqa: E402


def half(on=True, hue=0, sat=100, brt=255, effect=0):
    """One underglow half as the lab scripts' rgb() reports it."""
    return dict(on=bool(on), hue=hue, sat=sat, brt=brt, effect=effect)


RED = [half(), half()]


class Same(unittest.TestCase):
    """same(a, b) compares (on, hue, sat, effect) for two 2-element underglow states."""

    def test_matching_states_are_same(self):
        self.assertTrue(k.same(RED, [half(), half()]))

    def test_different_on_state_is_not_same(self):
        self.assertFalse(k.same(RED, [half(on=False), half()]))

    def test_different_hues_are_not_same(self):
        self.assertFalse(k.same([half(hue=120), half(hue=120)], [half(hue=240), half(hue=240)]))

    def test_brightness_is_ignored(self):
        self.assertTrue(k.same([half(brt=100), half(brt=100)], [half(brt=50), half(brt=99)]))

    def test_wrong_count_is_not_same(self):
        self.assertFalse(k.same(RED, [RED[0]]))


class HueIs(unittest.TestCase):
    """hue_is(hue, tolerance) returns a predicate matching two ON LEDs within tolerance."""

    def test_exact_hue_matches(self):
        self.assertTrue(k.hue_is(0)(RED))

    def test_tolerance_accepts_close_hues(self):
        self.assertTrue(k.hue_is(120, tolerance=5)([half(hue=116), half(hue=124)]))

    def test_wraparound_hues_match(self):
        self.assertTrue(k.hue_is(0, tolerance=5)([half(hue=355), half(hue=5)]))

    def test_out_of_tolerance_rejects(self):
        self.assertFalse(k.hue_is(120)([half(hue=130), half(hue=120)]))

    def test_off_led_fails(self):
        self.assertFalse(k.hue_is(0)([half(on=False), half()]))


if __name__ == "__main__":
    unittest.main()
