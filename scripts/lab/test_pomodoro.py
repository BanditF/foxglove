"""Pure helpers of scripts/lab/pomodoro.py: remaining() (focus.remaining to seconds) and frames()
(distinct panel frames in a window). Nothing here launches the app or touches the board.

pomodoro.py runs its argparse, its temp config and the whole board check at import time, so the
helpers are taken from its source via AST, as test_api_tools.py does for api_tools.py.

    python -m unittest test_pomodoro -v   # from scripts/lab/
"""
import ast
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

SRC = Path(__file__).resolve().with_name('pomodoro.py')


def _load(name, **g):
    """The real code of `name` from pomodoro.py, exec'd with `g` standing in for its module state."""
    tree = ast.parse(SRC.read_text(encoding='utf-8'))
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.name == name:
            module = ast.Module(body=[node], type_ignores=[])
            ast.fix_missing_locations(module)
            ns = dict(g)
            exec(compile(module, str(SRC), 'exec'), ns)  # noqa: S102
            return ns[name]
    raise LookupError(f'{name}: not found in pomodoro.py')


def _boom():
    raise AssertionError('facts() ran: the CLI must not be called when the facts are given')


class _Clock:
    """A deterministic stand-in for the `time` module: sleep() advances the clock instead of waiting."""

    def __init__(self, on_sleep=None):
        self.now, self.on_sleep = 0.0, on_sleep

    def time(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds
        if self.on_sleep:
            self.on_sleep(self.now)


REMAINING = _load('remaining', facts=_boom)


class Remaining(unittest.TestCase):
    def test_a_numeric_fact_is_the_seconds_as_int(self):
        self.assertEqual(REMAINING({'focus.remaining': '1500'}), 1500)
        self.assertIsInstance(REMAINING({'focus.remaining': '30'}), int)

    def test_a_clock_string_is_not_seconds(self):
        self.assertIsNone(REMAINING({'focus.remaining': '25:00'}))

    def test_facts_without_a_numeric_remaining_are_none(self):
        self.assertIsNone(REMAINING({'focus.state': 'focus', 'focus.paused': 'false'}))
        self.assertIsNone(REMAINING({'focus.remaining': ''}))

    def test_no_facts_means_reading_them(self):
        # f is absent or falsy: remaining() falls back to facts() (the CLI), which must not run here.
        for f in (None, {}):
            with self.assertRaises(AssertionError):
                REMAINING(f)


class Frames(unittest.TestCase):
    def dump(self, directory, data):
        path = Path(directory) / 'panel.pbm'
        path.write_bytes(data)
        return path

    def test_a_missing_dump_is_no_frames(self):
        missing = Path(tempfile.gettempdir()) / 'no-such-pomodoro-dump.pbm'
        frames = _load('frames', time=_Clock(), dump=missing)
        self.assertEqual(frames(2.0), 0)

    def test_a_stable_dump_counts_as_one_frame(self):
        with tempfile.TemporaryDirectory() as d:
            frames = _load('frames', time=_Clock(), dump=self.dump(d, b'P4\n8 2\n\xff\xff'))
            self.assertEqual(frames(1.0), 1)

    def test_each_distinct_dump_counts_once(self):
        with tempfile.TemporaryDirectory() as d:
            dump = self.dump(d, b'P4\n8 2\n\x00\x00')
            clock = _Clock(on_sleep=lambda t: dump.write_bytes(b'P4\n8 2\n\xff\xff') if t >= 0.5 else None)
            self.assertEqual(_load('frames', time=clock, dump=dump)(1.0), 2)

    def test_a_zero_window_reads_nothing(self):
        reads = []
        with tempfile.TemporaryDirectory() as d:
            dump = self.dump(d, b'P4\n8 2\n\xff\xff')

            class ReadingDump:
                def read_bytes(self):
                    reads.append(True)
                    return dump.read_bytes()

            self.assertEqual(_load('frames', time=_Clock(), dump=ReadingDump())(0.0), 0)
        self.assertEqual(reads, [])


if __name__ == '__main__':
    unittest.main()
