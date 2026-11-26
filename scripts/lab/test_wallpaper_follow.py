"""The pure helpers of wallpaper_follow.py, tested without its app-level case:
near(): the hue (circular) / sat (linear) tolerance it compares halves with;
manifests(): the foxglove.json manifests under a Wallpaper Engine folder, keyed by id.
Importing wallpaper_follow runs its module-level case (it would launch a test app and read the
user's PC), so the import stands in the labkit and wallpaper_check calls that touch them.

    python -m unittest test_wallpaper_follow -v   (from scripts/lab)
"""
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)

import labkit  # noqa: E402
import wallpaper_check  # noqa: E402


def _import_follow():
    we = Path(tempfile.mkdtemp(prefix='wallpaper-follow-test-'))
    for wid, accent in (('w-a', '#c89664'), ('w-b', '#f0b678'), ('w-c', None)):
        d = we / 'projects' / 'myprojects' / wid
        d.mkdir(parents=True)
        (d / 'foxglove.json').write_text(json.dumps({'id': wid, 'palette': {'accent': accent} if accent else {}}),
                                         encoding='utf-8')
    # Both halves on (30, 50): within near()'s default 3 of every colour the case below expects.
    state = [{'on': True, 'hue': 30, 'sat': 50, 'brt': 100, 'effect': 0},
             {'on': True, 'hue': 30, 'sat': 50, 'brt': 100, 'effect': 0}]

    class App:
        def __init__(self, exe, cfg, env=None, plugins=()):
            self.env = dict(env or {})
        def __enter__(self): return self
        def __exit__(self, *exc): return False

    class Checks:
        def __init__(self): self.results = []
        def __call__(self, name, ok, detail, blocked=False):
            self.results.append(blocked or ok)
            return ok
        def finish(self): pass

    argv = sys.argv
    # --app/--lab are checked for existence by their type= functions; --lab only feeds the stubs.
    lab = str(Path(labkit.__file__).resolve())
    sys.argv = ['wallpaper_follow.py', '--app', lab, '--lab', lab,
                '--we', str(we), '--walls', 'w-a', 'w-b', 'w-c']
    with mock.patch.object(labkit, 'require_app_ok', lambda: None),             mock.patch.object(labkit, 'Checks', Checks),             mock.patch.object(labkit, 'users_keyboard', lambda: ''),             mock.patch.object(labkit, 'temp_config', lambda prefix, store='': Path(tempfile.mkdtemp(prefix=prefix))),             mock.patch.object(labkit, 'rgb', lambda lab, env: state),             mock.patch.object(labkit, 'App', App), \
            mock.patch.object(labkit, 'we_stand_in', lambda we: Path(we)),             mock.patch.object(labkit, 'restore_lights', lambda *a: None),             mock.patch.object(wallpaper_check, 'windows_accent', lambda: ((250, 191, 123), 'the test stub')),             mock.patch.object(wallpaper_check, 'picked_accent', lambda: (211, 159, 63)):
        import wallpaper_follow
    sys.argv = argv
    return wallpaper_follow


wallpaper_follow = _import_follow()


class Near(unittest.TestCase):
    def test_identical_hue_and_sat_are_near(self):
        self.assertTrue(wallpaper_follow.near((120, 80), (120, 80)))

    def test_the_default_tolerance_is_three(self):
        self.assertTrue(wallpaper_follow.near((120, 80), (122, 80)))
        self.assertTrue(wallpaper_follow.near((120, 80), (120, 83)))
        self.assertFalse(wallpaper_follow.near((120, 80), (124, 80)))
        self.assertFalse(wallpaper_follow.near((120, 80), (120, 84)))

    def test_hue_distance_wraps_around_the_circle(self):
        self.assertTrue(wallpaper_follow.near((359, 50), (1, 50)))
        self.assertFalse(wallpaper_follow.near((350, 50), (10, 50)))
        self.assertFalse(wallpaper_follow.near((0, 0), (180, 0)))

    def test_a_wider_tolerance_admits_further_hues(self):
        self.assertFalse(wallpaper_follow.near((10, 10), (14, 10)))
        self.assertTrue(wallpaper_follow.near((10, 10), (14, 10), tol=4))


class Manifests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.we = Path(self.dir.name)

    def tearDown(self): self.dir.cleanup()

    def manifest(self, wid, body):
        d = self.we / 'projects' / 'myprojects' / wid
        d.mkdir(parents=True)
        (d / 'foxglove.json').write_text(body, encoding='utf-8')

    def test_manifests_are_keyed_by_their_id(self):
        self.manifest('w-a', json.dumps({'id': 'w-a', 'title': 'A'}))
        self.manifest('w-b', json.dumps({'id': 'w-b', 'title': 'B'}))
        m = wallpaper_follow.manifests(self.we)
        self.assertEqual(sorted(m), ['w-a', 'w-b'])
        self.assertEqual((m['w-a']['title'], m['w-b']['title']), ('A', 'B'))

    def test_broken_and_idless_manifests_are_skipped(self):
        self.manifest('w-bad', 'not json')
        self.manifest('w-no-id', json.dumps({'title': 'no id'}))
        self.manifest('w-ok', json.dumps({'id': 'w-ok'}))
        self.assertEqual(wallpaper_follow.manifests(self.we), {'w-ok': {'id': 'w-ok'}})

    def test_a_folder_with_no_manifests_is_empty(self):
        self.assertEqual(wallpaper_follow.manifests(self.we), {})
        self.assertEqual(wallpaper_follow.manifests(self.we / 'elsewhere'), {})

    def test_the_stand_in_folder_has_the_manifests_and_no_program(self):
        # #1519: what a test app gets instead of the installed Wallpaper Engine folder.
        self.manifest('w-a', json.dumps({'id': 'w-a'}))
        d = self.we / 'projects' / 'myprojects' / 'w-a'
        (d / 'project.json').write_text('{}', encoding='utf-8')
        (d / 'keyboard').mkdir()
        (d / 'keyboard' / 'right.bin').write_bytes(b'x')
        (d / 'scene.pkg').write_bytes(b'big')
        (self.we / 'wallpaper64.exe').write_bytes(b'')
        (self.we / 'config.json').write_text('{"user": 1}', encoding='utf-8')
        out = labkit.we_stand_in(self.we)
        self.assertNotEqual(out, self.we)
        files = sorted(p.relative_to(out).as_posix() for p in out.rglob('*') if p.is_file())
        self.assertEqual(files, ['config.json', 'projects/myprojects/w-a/foxglove.json',
                                 'projects/myprojects/w-a/keyboard/right.bin',
                                 'projects/myprojects/w-a/project.json'])
        self.assertEqual((out / 'config.json').read_text(encoding='utf-8'), '{}')
        self.assertEqual(wallpaper_follow.manifests(out), {'w-a': {'id': 'w-a'}})


if __name__ == '__main__':
    unittest.main()
