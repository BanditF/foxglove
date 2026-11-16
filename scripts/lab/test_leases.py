"""leases.py (#236/#250): the real script runs against a fake app, API and keyboard, so no GUI, network, hardware or files outside a temp dir. Run from scripts/lab: python -m unittest test_leases -v
"""
import contextlib
import io
import os
import sys
import types
import unittest
import unittest.mock as m
from pathlib import Path

os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
LAB = Path(__file__).resolve().parent
sys.path.insert(0, str(LAB))

import labkit  # noqa: E402

A, B = 'test-lab-leases-a', 'test-lab-leases-b'
BEFORE = [{'on': True, 'hue': 30, 'sat': 80, 'brt': 60, 'effect': 2}] * 2


class FakeBoard:
    def __init__(self, broken=None):
        self.broken, self.issued, self.leases = broken, [], []
        self.state, self.calls, self.closes, self.connects = [dict(h) for h in BEFORE], [], [], []

    def connect(self, name, permissions):
        self.connects.append((name, permissions))
        api = types.SimpleNamespace(name=name)
        api.call, api.close = lambda meth, params: self.call(name, meth, params), lambda: self.close(name)
        return api

    def call(self, owner, method, params):
        self.calls.append((owner, method, params))
        if method == 'devices.list':
            return {'devices': [{'id': 'kb', 'caps': ['lighting']}, {'id': 'screen', 'caps': ['feed']}]}
        if method == 'lighting.lease':
            self.issued.append((owner, params['color']['hue'], params['priority'], 'lease-%d' % (len(self.issued) + 1)))
            self.leases.append(self.issued[-1])
            self.refresh()
            return {'lease': self.issued[-1][3]}
        if method == 'lighting.release':
            if self.broken != 'release-ignored':
                self.leases = [l for l in self.leases if not (l[0] == owner and l[3] == params['lease'])]
                self.refresh()
            return {}
        raise AssertionError('unexpected API call ' + method)

    def close(self, owner):
        self.closes.append(owner)
        self.leases = [l for l in self.leases if l[0] != owner]
        self.refresh()

    def refresh(self):
        if not self.leases:
            self.state = [dict(h) for h in BEFORE]
            return
        l = self.leases[-1] if self.broken == 'latest-wins' else max(self.leases, key=lambda l: {'low': 0, 'normal': 1, 'high': 2}[l[2]])
        self.state = [{'on': True, 'hue': l[1], 'sat': 100, 'brt': 100, 'effect': 0}] * 2


@contextlib.contextmanager
def FakeApp(exe, cfg):
    yield types.SimpleNamespace(env={'FOXGLOVE_CONFIG_DIR': str(cfg)})


def run_leases(broken=None):
    board, saved_argv, buf, clock = FakeBoard(broken), sys.argv, io.StringIO(), {'t': 1000.0}  # a fake clock: sleeping only advances it
    sys.argv = ['leases.py', '--app', sys.executable, '--lab', str(LAB / 'labkit.py')]
    code = None
    try:
        with m.patch.object(labkit, 'App', FakeApp), m.patch.object(labkit, 'connect', board.connect), \
                m.patch.object(labkit, 'rgb', lambda lab, env: [dict(h) for h in board.state]), \
                m.patch('time.sleep', side_effect=lambda s: clock.update(t=clock['t'] + s)), \
                m.patch('time.time', side_effect=lambda: clock['t']), contextlib.redirect_stdout(buf):
            sys.modules.pop('leases', None)
            try:
                import leases  # noqa: F401
            except SystemExit as e:
                code = e.code
    finally:
        sys.argv = saved_argv
        sys.modules.pop('leases', None)
    return board, code, buf.getvalue()


class Leases(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.board, cls.code, cls.out = run_leases()

    def test_lease_order_is_low_green_normal_blue_high_red_low_yellow(self):
        self.assertEqual([(c[0], c[1]) for c in self.board.calls], [(A, 'devices.list'), (A, 'lighting.lease'), (B, 'lighting.lease'), (A, 'lighting.lease'), (B, 'lighting.lease'), (A, 'lighting.release')])
        leases = [c[2] for c in self.board.calls if c[1] == 'lighting.lease']
        self.assertEqual(([p['color']['hue'] for p in leases], [p['priority'] for p in leases]),
                         ([120, 240, 0, 60], ['low', 'normal', 'high', 'low']))
        self.assertTrue(all(p['device'] == 'kb' and p['color']['saturation'] == 100 and p['color']['brightness'] == 100 for p in leases))
        self.assertEqual(self.board.connects, [(A, ['devices.read', 'lighting.write']), (B, ['devices.read', 'lighting.write'])])

    def test_it_gives_back_its_own_high_lease_and_closes_b_then_a(self):
        high = next(i for owner, hue, prio, i in self.board.issued if owner == A and hue == 0)
        self.assertEqual([c[2] for c in self.board.calls if c[1] == 'lighting.release'], [{'lease': high}])
        self.assertEqual(self.board.closes, [B, A])

    def test_every_readback_matches_and_the_script_says_pass(self):
        self.assertEqual(self.code, 0)
        self.assertIn('RESULT PASS', self.out)
        self.assertNotIn('FAIL', self.out)
        for n in ('A low green shows', 'B normal blue shows over A low', 'A high red shows over B normal', 'B low yellow does not take over from red', 'A releases red: B normal blue is back', "B's connection closes: A low green is back", 'last lease ends: the lights are as before', '...and still as before 5 s later'):
            self.assertIn('PASS ' + n + ':', self.out)

    def test_a_broken_board_fails(self):
        _, code, out = run_leases('latest-wins')
        self.assertTrue(code == 1 and 'RESULT FAIL' in out and 'FAIL B low yellow does not take over from red:' in out)
        _, code, out = run_leases('release-ignored')
        self.assertTrue(code == 1 and 'RESULT FAIL' in out and 'FAIL A releases red: B normal blue is back:' in out)


if __name__ == '__main__':
    unittest.main()