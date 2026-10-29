"""The resumed 0.2 sitting's lab checks (docs/test-lab/sitting-0.2-h-i-r.md), offline: the overlay's
click-through probes (#549, #405/#406), the wallpaper readers (#586, #527, #70, #571) and the
right-panel redraw script (#621), on fixed inputs. Nothing here launches anything, makes input or
touches the keyboard.

    python -m unittest scripts/test_sitting_lab.py
"""
import os
import subprocess
import sys
import unittest
from datetime import timedelta
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'
os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
sys.path.insert(0, str(LAB))
import labkit  # noqa: E402
import wallpaper_check as wc  # noqa: E402
sys.path.pop(0)


def run(script, *args, env=None):
    e = {k: v for k, v in os.environ.items() if k != 'FOXGLOVE_APP_OK'}
    e.setdefault('USERNAME', 'test')
    e.update(env or {})
    return subprocess.run([sys.executable, str(LAB / script), *args], capture_output=True, text=True,
                          env=e, cwd=LAB, timeout=60)


class ClickThrough(unittest.TestCase):
    def test_transparent_and_layered_pass_clicks_on(self):
        self.assertTrue(labkit.click_through(labkit.WS_EX_TRANSPARENT | labkit.WS_EX_LAYERED | 0x8))
        self.assertFalse(labkit.click_through(labkit.WS_EX_LAYERED), 'layered alone still takes clicks')
        self.assertFalse(labkit.click_through(labkit.WS_EX_TRANSPARENT), 'transparent alone is only paint order')
        self.assertTrue(labkit.click_through(0, visible=False), 'a hidden window takes nothing')

    def test_the_panel_region_matches_its_rect_rounded_outward(self):
        self.assertTrue(labkit.same_box([100, 200, 421, 641], [100, 201, 420, 640]))
        self.assertFalse(labkit.same_box([100, 200, 421, 641], [100, 260, 420, 640]))
        self.assertFalse(labkit.same_box(None, [0, 0, 1, 1]), 'no region at all')

    def test_a_point_beside_the_panel_is_inside_the_overlay_and_away_from_it(self):
        overlay, panel = [0, 0, 2560, 1440], [2100, 900, 2540, 1400]
        x, y = labkit.outside(panel, overlay)
        self.assertTrue(0 <= x <= 2560 and 0 <= y <= 1440)
        self.assertFalse(panel[0] - 40 < x < panel[2] + 40 and panel[1] - 40 < y < panel[3] + 40)
        self.assertIsNone(labkit.outside([0, 0, 2560, 1440], overlay), 'a panel over everything')
        self.assertEqual(labkit.centre([0, 0, 10, 20]), (5, 10))


CONFIG = '''{
 "fuchs": {"general": {"user": {"playbackfullscreen": "stop", "playbackmaximized": "stop",
                                "playbackfocus": "run", "pausevram": true},
   "wallpaperconfig": {"selectedwallpapers": {
     "Monitor0": {"file": "C:/w/workshop/scene.pkg"},
     "//?/DISPLAY#SAM0E3C#1": {"file": "C:\\\\we\\\\bandit-hud\\\\index.html"},
     "//?/DISPLAY#DEL40EA#1": {"file": "C:/we/bandit-fury-stars/index.html"}}}}},
 "version": 1
}'''


class Wallpaper(unittest.TestCase):
    def test_monitors_come_in_foxgloves_order(self):
        sel = wc.selected(CONFIG)
        self.assertEqual([m for m, _ in sel], ['//?/DISPLAY#DEL40EA#1', '//?/DISPLAY#SAM0E3C#1', 'Monitor0'])
        self.assertEqual(sel[1][1], 'C:/we/bandit-hud/index.html', 'backslashes become slashes')

    def test_the_playback_settings_behind_586(self):
        self.assertEqual(wc.playback(CONFIG)['playbackfullscreen'], 'stop')
        self.assertEqual(wc.playback('{"version": 1}'), {})

    def test_the_accent_is_foxgloves(self):
        # accent::hue_sat and wallpaper_manifest::most_colourful, worked by hand.
        self.assertEqual(wc.hue_sat(0x0f, 0xb5, 0xae), (177, 92))
        self.assertEqual(wc.hue_sat(0xd9, 0xa4, 0x41), (39, 70))
        self.assertEqual(wc.hue_sat(9, 9, 9), (0, 0))
        reef = {'palette': {'stops': ['#e8dcc0', '#0fb5ae', '#13294b']}}
        self.assertEqual(wc.manifest_accent(reef), (0x0f, 0xb5, 0xae))
        named = {'palette': {'accent': '#f2e8cf', 'stops': ['#0fb5ae', '#13294b']}}
        self.assertEqual(wc.manifest_accent(named), (0xf2, 0xe8, 0xcf))
        self.assertIsNone(wc.manifest_accent({'motifs': []}), 'no palette: the Windows accent')
        self.assertEqual(wc.most_colourful(['#ff0000', '#00ff00']), (255, 0, 0), 'a tie keeps the first')

    def test_follow_wallpaper_is_off_unless_the_store_says_so(self):
        self.assertFalse(wc.follow_wallpaper('[lighting]\nfollow_accent = true\n\n[overlay_place]\n'))
        self.assertTrue(wc.follow_wallpaper('[lighting]\nfollow_accent = true\nfollow_wallpaper = true\n'))
        self.assertFalse(wc.follow_wallpaper('[other]\nfollow_wallpaper = true\n'))

    def test_log_lines_since_a_time(self):
        since = wc.utc('2026-10-29T09:00:00Z')
        fox = ['2026-10-29 08:59:59Z Foxglove: wallpaper sync: sent to 1 monitor(s): layer GAME, game mode on',
               '2026-10-29 09:00:03Z Foxglove: wallpaper sync: sent to 1 monitor(s): layer GAME, game mode on']
        self.assertEqual(len(wc.log_since(fox, since, 'game mode on')), 1)
        we = ['2026-10-29T08:00:00Z, 1: Command ignored because Windows is reentrant.',
              '2026-10-29T09:01:00Z, 2: Command ignored because Windows is reentrant.',
              '2026-10-29T09:02:00Z, 3: DXGI device lost in render loop.']
        self.assertEqual(len(wc.we_log_since(we, since, 'reentrant')), 1)
        self.assertEqual(len(wc.we_log_since(we, since, 'DXGI device lost')), 1)

    def test_a_reload_is_a_gap_in_the_snapshots(self):
        def snap(s, game):
            t = wc.utc('2026-10-29T09:00:00Z') + timedelta(seconds=s)
            return {'at': t.isoformat().replace('+00:00', 'Z'), 'foxglove': {'game': game}}
        kept = [snap(0, True), snap(2, True), snap(40, True), snap(42, True)]
        gaps = wc.reloads(kept)
        self.assertEqual([(b - a).total_seconds() for a, b, *_ in gaps], [38.0])
        self.assertEqual(gaps[0][2:], (True, True))
        lost = [snap(0, True), snap(30, False)]
        self.assertEqual(wc.reloads(lost)[0][2:], (True, False))
        self.assertEqual(wc.reloads([snap(0, True), snap(2, True), snap(4, True)]), [], 'every 2 s: no gap')


class Scripts(unittest.TestCase):
    def test_wallpaper_follow_launches_only_with_on_calls_ok(self):
        r = run('wallpaper_follow.py', '--app', sys.executable, '--lab', sys.executable)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('FOXGLOVE_APP_OK=1', r.stdout)

    def test_right_redraws_is_blocked_without_a_redraw_count(self):
        # A "lab" that prints no RIGHT_REDRAWS line (no HELLO bit 25, or not the lab): BLOCKED, and
        # nothing after the first status read runs.
        r = run('right_redraws.py', '--lab', sys.executable)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('BLOCKED the right reports its redraw count', r.stdout)
        self.assertNotIn('cycle 1', r.stdout)

    def test_the_redraw_count_is_read_from_status(self):
        sys.path.insert(0, str(LAB))
        try:
            from right_redraws import parse_redraws as parse
        finally:
            sys.path.pop(0)
        self.assertEqual(parse('HELLO ...\nRIGHT_REDRAWS: 87\nRGB_STATE ...'), 87)
        self.assertIsNone(parse('RIGHT_REDRAWS: unknown (no HELLO bit 25)'))
        self.assertIsNone(parse('RIGHT_REDRAWS: error timeout'))

    def test_the_overlay_cases_take_the_focus_only_with_focus(self):
        text = (LAB / 'overlay_tray.py').read_text(encoding='utf-8')
        self.assertIn('if args.focus:\n    require_user_idle(120)', text)
        self.assertIn('if args.focus and hwnd:', text)


if __name__ == '__main__':
    unittest.main()
