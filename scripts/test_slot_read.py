"""The data-safe flow of scripts/lab/slot_read.py (#911) against a fake keyboard. Nothing here runs the
lab, launches an app or touches a keyboard.

    python -m unittest scripts/test_slot_read.py
"""
import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'


def load():
    os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
    sys.path.insert(0, str(LAB))
    try:
        import slot_read
        return slot_read
    finally:
        sys.path.remove(str(LAB))


slot_read = load()


class FakeLab:
    """Stands in for the lab's slot-read cases over a fake keyboard of kept places."""

    def __init__(self, places, fail=()):
        self.places = dict(places)      # slot -> bytes
        self.fail = dict(fail)          # case -> remaining failures
        self.calls = []
        self.original = dict(places)

    def __call__(self, case, env):
        self.calls.append(case)
        if self.fail.get(case, 0) > 0:
            self.fail[case] -= 1
            return 1, f'{case} failed'
        slot, base = int(env['FOXGLOVE_SLOT_READ_SLOT']), env['FOXGLOVE_SLOT_READ_BACKUP']
        if case == 'slot-read-backup':
            Path(base + '.bin').write_bytes(self.places[slot])
            Path(base + '.json').write_text(json.dumps({'slot': slot, 'name': 'n', 'crc': 0xabc, 'len': len(self.places[slot])}))
            return 0, 'backed up'
        if case == 'slot-read-write':
            assert Path(base + '.bin').read_bytes() == self.places[slot], 'written before a matching backup'
            self.places[slot] = b'FIXTURE'
            return 0, 'written'
        if case == 'slot-read-restore':
            self.places[slot] = Path(base + '.bin').read_bytes()
            return 0, 'restored'
        if case == 'slot-read-verify':
            return (0, 'same') if self.places[slot] == Path(base + '.bin').read_bytes() else (1, 'differs')
        raise AssertionError(case)


class Check:
    def __init__(self):
        self.rows = []

    def __call__(self, name, ok, detail, blocked=False):
        self.rows.append((name, ok, blocked))
        return ok


class SlotReadFlow(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.base = str(Path(self.dir) / 'slot1')
        self.out = io.StringIO()

    def run_flow(self, fake, body, dry_run=False):
        check = Check()
        with redirect_stdout(self.out):
            ok = slot_read.run_with_backup(fake, check, 1, self.base, dry_run, body, retries=2, pause=0)
        return ok, check

    def test_pass_restores_original(self):
        fake = FakeLab({1: b'original'})
        ran = []
        ok, check = self.run_flow(fake, lambda: ran.append(1))
        self.assertTrue(ok)
        self.assertEqual(ran, [1])
        self.assertEqual(fake.places[1], b'original')
        self.assertEqual(fake.calls, ['slot-read-backup', 'slot-read-write', 'slot-read-restore'])

    def test_exception_in_test_still_restores(self):
        fake = FakeLab({1: b'original'})

        def boom():
            raise RuntimeError('feed died')
        ok, check = self.run_flow(fake, boom)
        self.assertFalse(ok)
        self.assertEqual(fake.places[1], b'original')
        self.assertIn('slot-read-restore', fake.calls)

    def test_write_failure_still_restores(self):
        fake = FakeLab({1: b'original'}, fail={'slot-read-write': 1})
        ran = []
        ok, _ = self.run_flow(fake, lambda: ran.append(1))
        self.assertFalse(ok)
        self.assertEqual(ran, [])
        self.assertIn('slot-read-restore', fake.calls)

    def test_restore_retried_then_ok(self):
        fake = FakeLab({1: b'original'}, fail={'slot-read-restore': 1})
        ok, _ = self.run_flow(fake, lambda: None)
        self.assertTrue(ok)
        self.assertEqual(fake.calls.count('slot-read-restore'), 2)
        self.assertNotIn('MANUAL RESTORE', self.out.getvalue())

    def test_unverified_restore_prints_backup_path_and_instructions(self):
        fake = FakeLab({1: b'original'}, fail={'slot-read-restore': 5})
        ok, check = self.run_flow(fake, lambda: None)
        self.assertFalse(ok)
        text = self.out.getvalue()
        self.assertIn('MANUAL RESTORE NEEDED', text)
        self.assertIn(self.base + '.bin', text)
        self.assertIn('--only slot-read-restore', text)
        self.assertTrue(Path(self.base + '.bin').read_bytes() == b'original', 'backup survives')

    def test_backup_failure_writes_nothing(self):
        fake = FakeLab({1: b'original'}, fail={'slot-read-backup': 1})
        ok, _ = self.run_flow(fake, lambda: self.fail('the test must not run'))
        self.assertFalse(ok)
        self.assertEqual(fake.calls, ['slot-read-backup'])

    def test_dry_run_only_backs_up_and_verifies(self):
        fake = FakeLab({1: b'original'})
        ok, _ = self.run_flow(fake, None, dry_run=True)
        self.assertTrue(ok)
        self.assertEqual(fake.calls, ['slot-read-backup', 'slot-read-verify'])
        self.assertEqual(fake.places[1], b'original')

    def test_pick_slot(self):
        base = str(Path(self.dir) / 'pick')

        def step(case, env):
            Path(env['FOXGLOVE_SLOT_READ_BACKUP']).write_text(json.dumps([{'slot': 2}, {'slot': 5}]))
            return 0, ''
        self.assertEqual(slot_read.pick_slot(step, base, None), (2, None))
        self.assertEqual(slot_read.pick_slot(step, base, 5), (5, None))

        def empty(case, env):
            Path(env['FOXGLOVE_SLOT_READ_BACKUP']).write_text('[]')
            return 0, ''
        self.assertIsNone(slot_read.pick_slot(empty, base, None)[0])


if __name__ == '__main__':
    unittest.main()
