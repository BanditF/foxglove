"""labkit's harness fixes from the #618 exit-check run. Nothing here launches anything or makes input.

- UTF-8 out: a lab script printing "→" through labkit on a cp1252 stream (a piped Windows console)
  crashed with UnicodeEncodeError; labkit now writes UTF-8 whatever the stream was.
- The harness's own input: injections recorded by labkit (and foxglove-lab, same file format) are
  left out of the "user is working" gate, and the user's own input still blocks.

    python -m unittest scripts/test_lab_harness.py
"""
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'


def labkit():
    os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
    sys.path.insert(0, str(LAB))
    try:
        import labkit as k
    finally:
        sys.path.pop(0)
    return k


class Utf8Out(unittest.TestCase):
    def test_a_check_prints_an_arrow_under_a_cp1252_stdout(self):
        # PYTHONIOENCODING=cp1252 gives the child the strict cp1252 stdout a piped console has.
        env = {**os.environ, 'PYTHONIOENCODING': 'cp1252'}
        env.setdefault('APPDATA', str(Path.home()))
        env.setdefault('USERNAME', 'test')
        code = 'import labkit; labkit.Checks()("arrow", True, "Foxglove: overlay window → restored")'
        r = subprocess.run([sys.executable, '-c', code], capture_output=True, env=env, cwd=LAB, timeout=60)
        self.assertEqual(r.returncode, 0, r.stderr.decode('utf-8', 'replace'))
        self.assertIn('PASS arrow: Foxglove: overlay window → restored', r.stdout.decode('utf-8'))

    def test_utf8_reconfigures_a_cp1252_stream(self):
        k = labkit()
        raw = io.BytesIO()
        stream = io.TextIOWrapper(raw, encoding='cp1252')
        print('a → b', file=k.utf8(stream), flush=True)
        self.assertEqual(raw.getvalue().decode('utf-8').strip(), 'a → b')

    def test_utf8_leaves_other_streams_alone(self):
        k = labkit()
        s = io.StringIO()
        self.assertIs(k.utf8(s), s)


class HarnessInput(unittest.TestCase):
    def setUp(self):
        self.k = labkit()
        self.dir = tempfile.TemporaryDirectory()
        self.path = Path(self.dir.name) / 'lab-injections.log'

    def tearDown(self):
        self.dir.cleanup()

    def test_input_just_after_an_injection_is_the_harness(self):
        # The user last typed at 1000; the lab clicked from 1100 to 1100.2; the PC saw input at 1101.
        self.k.record_injection(1100.0, 1100.2, 1000.0, 'tray click (lab hook)', self.path)
        idle, r = self.k.user_idle(now=1130.0, raw=1101.0, path=self.path)
        self.assertEqual(idle, 130.0)
        self.assertEqual(r[3], 'tray click (lab hook)')

    def test_the_users_own_input_still_blocks(self):
        self.k.record_injection(1100.0, 1100.2, 1000.0, 'tray click (lab hook)', self.path)
        # Input 2.5 s after the click is past the 2 s window: the user's.
        idle, r = self.k.user_idle(now=1130.0, raw=1102.7, path=self.path)
        self.assertAlmostEqual(idle, 27.3)
        self.assertIsNone(r)
        # Input before the injection (and before its 0.1 s slack) is the user's too.
        idle, r = self.k.user_idle(now=1130.0, raw=1099.5, path=self.path)
        self.assertAlmostEqual(idle, 30.5)
        self.assertIsNone(r)

    def test_back_to_back_injections_chain_to_the_users_last_input(self):
        # The second injection began while the PC's last input was the first's: the user's last
        # input recorded with it is the first's "before", so the chain ends at the user.
        self.k.record_injection(1100.0, 1100.2, 1000.0, 'position 11 tap', self.path)
        before, _ = self.k.user_last_input(1100.1, self.k.injections(self.path))
        self.k.record_injection(1105.0, 1105.1, before, 'position 12 tap', self.path)
        idle, r = self.k.user_idle(now=1200.0, raw=1105.5, path=self.path)
        self.assertEqual(idle, 200.0)

    def test_records_are_the_format_foxglove_lab_writes(self):
        # crates/foxglove-lab/src/injections.rs: "<start> <end> <user_last or -> <what>".
        self.path.write_text('100.000 100.250 40.500 position 11 tap\njunk\n1.000 2.000 - invoke 43\n',
                             encoding='utf-8')
        self.assertEqual(self.k.injections(self.path),
                         [(100.0, 100.25, 40.5, 'position 11 tap'), (1.0, 2.0, None, 'invoke 43')])
        # No user input known before an injection: idle since it began (the cautious reading).
        idle, _ = self.k.user_idle(now=10.0, raw=1.5, path=self.path)
        self.assertEqual(idle, 9.0)

    def test_injecting_records_the_block(self):
        with self.k.injecting('tray click (lab hook)', self.path):
            pass
        (start, end, _, what), = self.k.injections(self.path)
        self.assertLessEqual(start, end)
        self.assertEqual(what, 'tray click (lab hook)')


if __name__ == '__main__':
    unittest.main()
