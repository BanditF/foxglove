"""we_trace.py (#1120): sender counting over a fake WE runner and fake pipe; no GUI, no real WE. Run from scripts/lab: python -m unittest test_we_trace -v
"""
import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent
sys.path.insert(0, str(LAB))
import we_trace as w  # noqa: E402

ARETE_SRC = Path(r'C:\src\arete\src')
EXE = 'C:/we/wallpaper64.exe'


class Counting(unittest.TestCase):
    def test_one_sender_passes(self):
        t = w.Trace()
        r = w.FakeRunner(t).as_sender('foxglove')
        r(EXE, '-control', 'pause')
        r(EXE, '-control', 'play')
        ok, text = w.report(t.records)
        self.assertTrue(ok)
        self.assertIn('foxglove', text)

    def test_two_senders_fail(self):
        t = w.Trace()
        rr = w.FakeRunner(t)
        rr.as_sender('foxglove')(EXE, '-control', 'applyProperties')
        rr.as_sender('arete')(EXE, '-control', 'pause')
        ok, _ = w.report(t.records)
        self.assertFalse(ok)
        self.assertEqual(set(w.senders(t.records)), {'foxglove', 'arete'})

    def test_pipe_clients_are_not_senders(self):
        t = w.Trace()
        fox = w.FakeRunner(t).as_sender('foxglove')
        w.FakePipe(t, lambda m, p: fox(EXE, '-control', 'pause')).call('arete', 'wallpaper-engine.pause')
        self.assertEqual(set(w.senders(t.records)), {'foxglove'})

    def test_other_exes_and_non_control_ignored(self):
        t = w.Trace()
        t.add('x', 'exe', ['C:/other/notepad.exe', '-control', 'pause'])
        t.add('y', 'exe', [EXE, '--version'])
        self.assertEqual(w.senders(t.records), {})

    def test_jsonl_roundtrip_and_cli(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / 't.jsonl'
            t = w.Trace(str(p))
            w.FakeRunner(t).as_sender('foxglove')(EXE, '-control', 'pause')
            self.assertEqual(len(w.load(p)), 1)
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(w.main(['analyze', str(p)]), 0)
                self.assertEqual(w.main(['analyze', str(p), '--expect-senders', '2']), 1)


@unittest.skipUnless((ARETE_SRC / 'arete' / 'claim.py').exists(), 'Arete checkout not present (CI)')
class RealAreteCode(unittest.TestCase):
    def test_today_both_apps_send(self):
        t = w.scenario(ARETE_SRC, False)
        self.assertEqual(set(w.senders(t.records)), {'foxglove', 'arete'})  # the #1120 gap: reported, not fixed

    def test_target_shape_via_hub_is_one_sender(self):
        t = w.scenario(ARETE_SRC, True)
        self.assertEqual(set(w.senders(t.records)), {'foxglove'})


if __name__ == '__main__':
    unittest.main()
