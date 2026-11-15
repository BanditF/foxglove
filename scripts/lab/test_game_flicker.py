"""Tests for game_flicker.py pure helpers: band(), into_reload(), pushes()."""

import sys
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import game_flicker as gf  # noqa: E402


class Band(unittest.TestCase):
    def test_band_scales_the_top_share(self):
        self.assertEqual(gf.band([10, 20, 100, 200], 0.5), [10, 20, 100, 110])

    def test_band_has_a_minimum_height_of_8(self):
        self.assertEqual(gf.band([0, 0, 100, 1], 0.4), [0, 0, 100, 8])


class IntoReload(unittest.TestCase):
    def test_into_reload_finds_pushes_in_the_quiet_window(self):
        key = datetime(2024, 6, 15, 12, 0, 0, tzinfo=timezone.utc)
        pushes = [
            (key + timedelta(seconds=0), 'push A'),
            (key + timedelta(seconds=1), 'push B'),
            (key + timedelta(seconds=2), 'push C'),
        ]
        result = gf.into_reload(pushes, key)
        self.assertEqual([l for _, l in result], ['push A', 'push B'])

    def test_into_reload_excludes_the_end_boundary(self):
        key = datetime(2024, 6, 15, 12, 0, 0, tzinfo=timezone.utc)
        pushes = [
            (key + timedelta(seconds=0), 'push A'),
            (key + timedelta(seconds=2), 'push B'),
        ]
        result = gf.into_reload(pushes, key)
        self.assertEqual([l for _, l in result], ['push A'])

    def test_into_reload_uses_custom_quiet_s(self):
        key = datetime(2024, 6, 15, 12, 0, 0, tzinfo=timezone.utc)
        pushes = [
            (key + timedelta(seconds=2), 'push B'),
        ]
        result = gf.into_reload(pushes, key, quiet_s=3.5)
        self.assertEqual(len(result), 1)


class Pushes(unittest.TestCase):
    def test_pushes_filters_wallpaper_sync_lines(self):
        lines = [
            "2024-06-15 12:00:00Z wallpaper sync: test sent to monitor",
            "2024-06-15 12:00:01Z unrelated line",
        ]
        result = gf.pushes(lines, gf.utc("2024-06-15 11:00:00Z"))
        self.assertEqual(result, [(gf.utc("2024-06-15 12:00:00Z"), lines[0])])

    def test_pushes_requires_sent_to(self):
        lines = ["2024-06-15 12:00:00Z wallpaper sync: skipped, nothing changed"]
        self.assertEqual(gf.pushes(lines, gf.utc("2024-06-15 11:00:00Z")), [])

    def test_pushes_returns_empty_when_no_match(self):
        lines = ["no wallpaper sync here"]
        result = gf.pushes(lines, gf.utc("2024-06-15 11:00:00Z"))
        self.assertEqual(result, [])


if __name__ == "__main__":
    unittest.main()
