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


class PreSittingRun(unittest.TestCase):
    """The 10/29 pre-sitting run's three failures (On-call, #90), each from its own inputs."""

    def test_the_panels_hit_test_is_waited_for_not_read_once(self):
        # overlay_tray --focus FAIL "panel open: the overlay takes the mouse (not transparent)":
        # tray.set_panel answers from the frame that drew the panel; the region follows in that
        # frame, the passthrough only after its paint. A style read once right after the region
        # sees WS_EX_TRANSPARENT still; waiting (bounded) sees the panel take the mouse.
        transparent = 0xC0138  # as the run read it: WS_EX_TRANSPARENT | WS_EX_LAYERED | topmost, …
        taking = transparent & ~labkit.WS_EX_TRANSPARENT & ~labkit.WS_EX_LAYERED  # winit's hittest on
        reads = iter([transparent, transparent, taking])
        style = lambda: next(reads, labkit.WS_EX_LAYERED)
        self.assertTrue(labkit.click_through(transparent), 'a single read right after the region: FAIL')
        ok, last = labkit.wait(style, lambda s: not labkit.click_through(s), 2, poll=0.01)
        self.assertTrue(ok, hex(last))
        text = (LAB / 'overlay_tray.py').read_text(encoding='utf-8')
        self.assertIn("ok, style = wait(lambda: win.exstyle(hwnd), lambda s: not click_through(s, win.visible(hwnd)), 5,",
                      text)
        self.assertNotIn('not click_through(win.exstyle(hwnd), win.visible(hwnd))', text, 'no one-shot read')

    def test_the_test_app_gets_the_users_keyboard(self):
        # FAIL "Settings focused: only over the keys (a window region): None": the temp store had
        # no [connection.last], so the test app's overlay never connected and drew no keys.
        store = ('schema = 1\n\n[connection.last]\nid = "COM4"\npid = 24926\ntransport = "serial"\nvid = 7504\n\n'
                 '[layers.names]\n0 = "QWERTY"\n')
        seed = labkit.remembered_keyboard(store)
        import tomllib
        self.assertEqual(tomllib.loads('schema = 1\n' + seed)['connection']['last'],
                         {'id': 'COM4', 'pid': 24926, 'transport': 'serial', 'vid': 7504})
        self.assertEqual(labkit.remembered_keyboard('schema = 1\n[lighting]\nfollow_accent = true\n'), '')
        self.assertEqual(labkit.remembered_keyboard('[connection.last]\nid = "COM4"\n'), '', 'incomplete')
        self.assertEqual(labkit.remembered_keyboard('not = [toml'), '')
        text = (LAB / 'overlay_tray.py').read_text(encoding='utf-8')
        self.assertIn("temp_config('foxglove-overlay-tray-', KEYBOARD)", text)

    def test_a_closed_pipe_says_the_app_ended(self):
        # The crash: OSError 22 writing the API pipe after the test app had gone.
        class Broken:
            def write(self, _):
                raise OSError(22, 'Invalid argument')

            def read(self, _):
                raise OSError(22, 'Invalid argument')
        api = object.__new__(labkit.Api)
        api.pipe, api.next = Broken(), 0
        with self.assertRaises(labkit.AppGone) as e:
            api.call('tray.panel', {})
        self.assertIn('the API pipe is closed', str(e.exception))
        self.assertTrue(issubclass(labkit.AppGone, RuntimeError))
        text = (LAB / 'overlay_tray.py').read_text(encoding='utf-8')
        self.assertIn('except AppGone:\n        raise', text, "call() doesn't swallow it")
        self.assertIn("check('Settings minimized: the test app keeps running (in the tray)'", text)

    def test_how_the_test_app_ended(self):
        # 10/29: no panic line, no crash event; it quit with code 0 when Settings was minimized
        # (KeyPeek's quit before the overlay ever connected; the app no longer does that).
        log = ['2026-10-29 09:08:57Z Foxglove 0.2.0 started (pid 650076, 228943 s after boot)',
               '2026-10-29 09:08:57Z Foxglove: already running; showing that one']
        self.assertIn('quit on its own (exit code 0), not a crash', labkit.exit_report(0, log))
        self.assertIn('already running', labkit.exit_report(0, log))
        now = log + ['2026-10-29 10:00:00Z Foxglove: the overlay window closed; quitting']
        self.assertIn('the overlay window closed; quitting', labkit.exit_report(0, now))
        self.assertIn('access violation', labkit.exit_report(-1073741819, log), 'signed 0xC0000005')
        self.assertIn('access violation', labkit.exit_report(0xC0000005, log))
        panic = log + ['2026-10-29 09:09:00Z Foxglove: panic in main: boom at src/x.rs:1']
        self.assertIn('a panic: ', labkit.exit_report(101, panic))
        self.assertEqual(labkit.exit_report(None, log), 'still running')

    def test_the_app_object_reports_its_exit(self):
        import tempfile
        cfg = Path(tempfile.mkdtemp(prefix='foxglove-lab-test-')) / 'config'
        (cfg / 'logs').mkdir(parents=True)
        (cfg / 'logs' / 'foxglove.log').write_text('2026-10-29 09:08:53Z Foxglove 0.2.0 started (pid 1)\n',
                                                    encoding='utf-8')
        app = labkit.App(sys.executable, cfg)
        app.proc = subprocess.Popen([sys.executable, '-c', 'raise SystemExit(0)'])
        app.proc.wait(30)
        self.assertEqual(app.exited(), 0)
        self.assertIn('not a crash', app.exit_report())

    def test_the_windows_accent_is_the_one_foxglove_reads(self):
        # wallpaper_follow FAIL: fury-forest (no palette) expected 39/70, the app applied 42/97.
        # The test read DWM's AccentColor (the picked colour); accent.rs reads UISettings' Accent,
        # the accent palette's entry 3. Both from this PC's registry on 10/29.
        picked = wc.dwm_rgb(0xFF3F9FD3)
        self.assertEqual(picked, (0xd3, 0x9f, 0x3f))
        self.assertEqual(wc.hue_sat(*picked), (39, 70), "what the run expected")
        palette = bytes.fromhex('FAE76C00 F8D33400 C28E0600 9C6F0500 84570400 65380300 42140100 88179800')
        accent = wc.palette_accent(palette)
        self.assertEqual(accent, (0x9c, 0x6f, 0x05), "UISettings' Accent read the same on 10/29")
        self.assertEqual(wc.hue_sat(*accent), (42, 97), "what the app applied: right")
        self.assertIsNone(wc.palette_accent(b'\x00' * 8))
        forest = {'id': 'fury-forest', 'motifs': []}
        hs, source = wc.expected(forest, (accent, 'UISettings Accent'))
        self.assertEqual(hs, (42, 97))
        self.assertIn('the Windows accent #9c6f05', source)
        pattern = {'palette': {'accent': '#d9a441'}}
        hs, source = wc.expected(pattern, (accent, 'UISettings Accent'))
        self.assertEqual((hs, source.split(' #')[0]), ((39, 70), 'the wallpaper palette'), 'a palette wins')
        self.assertEqual(wc.expected(forest, (None, 'unreadable'))[0], None)
        text = (LAB / 'wallpaper_follow.py').read_text(encoding='utf-8')
        self.assertNotIn("'AccentColor'", text, 'the picked colour is never the expectation')

    def test_the_lighting_flags(self):
        store = '[lighting]\nfollow_accent = true\n\n[overlay_place]\nmonitor = ""\n'
        self.assertTrue(wc.follow_accent(store))
        self.assertFalse(wc.follow_wallpaper(store))


if __name__ == '__main__':
    unittest.main()
