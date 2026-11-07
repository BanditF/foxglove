"""Tests for local_config.py: defaults are this PC's, a local.toml and env vars override."""
from pathlib import Path
import tempfile
import unittest

import local_config as lc


class LocalConfigTest(unittest.TestCase):
    def test_defaults_are_this_pcs_values(self):
        c = lc.load(env={}, files=[])
        self.assertEqual(c['keyboard']['left_serial'], '1E89EF085AA8A635')
        self.assertEqual(c['keyboard']['right_serial'], '35B4541F78DE9CF8')
        self.assertEqual(lc.get('worktrees.root', env={}, files=[]),
                         r'C:\Users\fuchs\splitkeeb\worktrees')
        self.assertEqual(lc.serials(env={}, files=[]),
                         {'1E89EF085AA8A635': 'left', '35B4541F78DE9CF8': 'right'})

    def test_a_local_toml_then_the_environment_override(self):
        with tempfile.TemporaryDirectory() as d:
            first = Path(d) / 'a.toml'
            second = Path(d) / 'b.toml'
            first.write_text('[keyboard]\nleft_serial = "AAAA"\n[flash]\nrollback = "D:/fw"\n',
                             encoding='utf-8')
            second.write_text('[keyboard]\nright_serial = "IGNORED"\n', encoding='utf-8')
            files = [Path(d) / 'missing.toml', first, second]
            c = lc.load(env={}, files=files)
            self.assertEqual(c['keyboard']['left_serial'], 'AAAA')
            self.assertEqual(c['keyboard']['right_serial'], '35B4541F78DE9CF8',
                             'only the first file found counts; other keys keep defaults')
            self.assertEqual(c['flash']['rollback'], 'D:/fw')
            env = {'FOXGLOVE_LEFT_SERIAL': 'BBBB', 'FOXGLOVE_WORKTREES': 'E:/wt'}
            c = lc.load(env=env, files=files)
            self.assertEqual(c['keyboard']['left_serial'], 'BBBB')
            self.assertEqual(c['worktrees']['root'], 'E:/wt')

    def test_identity_is_off_unless_turned_on(self):
        self.assertEqual(lc.load(env={}, files=[])['identity'],
                         {'sparse': False, 'publisher': ''})
        with tempfile.TemporaryDirectory() as d:
            on = Path(d) / 'local.toml'
            on.write_text('[identity]\nsparse = true\npublisher = "CN=Foxglove Dev, O=BanditF"\n',
                          encoding='utf-8')
            c = lc.load(env={}, files=[on])['identity']
            self.assertEqual(c, {'sparse': True, 'publisher': 'CN=Foxglove Dev, O=BanditF'})
            c = lc.load(env={'FOXGLOVE_SPARSE_IDENTITY': '0'}, files=[on])['identity']
            self.assertFalse(c['sparse'], 'an explicit 0 opts out')
            c = lc.load(env={'FOXGLOVE_SPARSE_IDENTITY': '1',
                             'FOXGLOVE_IDENTITY_PUBLISHER': 'CN=X'}, files=[])['identity']
            self.assertEqual(c, {'sparse': True, 'publisher': 'CN=X'})
            c = lc.load(env={'FOXGLOVE_SPARSE_IDENTITY': ''}, files=[on])['identity']
            self.assertTrue(c['sparse'], 'an empty variable counts as unset')

    def test_bad_serials_are_refused(self):
        for env in ({'FOXGLOVE_LEFT_SERIAL': 'x|.*'},
                    {'FOXGLOVE_LEFT_SERIAL': '35B4541F78DE9CF8'}):
            with self.assertRaises(ValueError):
                lc.serials(env=env, files=[])

    def test_where_local_toml_is_looked_for(self):
        found = list(lc.candidates(env={'FOXGLOVE_LOCAL_TOML': 'X:/mine.toml'},
                                   repo=Path('R:/repo'), workspace=Path('W:/ws')))
        self.assertEqual(found, [Path('X:/mine.toml'), Path('R:/repo/tools/local.toml'),
                                 Path('W:/ws/tools/local.toml')])

    def test_the_example_lists_every_key(self):
        import tomllib
        example = lc.REPO / 'tools' / 'local.example.toml'
        with open(example, 'rb') as f:
            keys = {(s, k) for s, v in tomllib.load(f).items() for k in v}
        self.assertEqual(keys, {(s, k) for s, v in lc.DEFAULTS.items() for k in v})


if __name__ == '__main__':
    unittest.main()
