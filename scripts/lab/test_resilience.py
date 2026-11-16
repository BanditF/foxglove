"""suite() and typing() from resilience.py: the lab binary, the lock state and the repo root are
faked, so no keyboard, app or repo is needed and every file lands in a temp dir.

    python -m unittest test_resilience -v
"""
import io
import json
import os
import subprocess
import sys
import tempfile
import tomllib
import unittest
import unittest.mock as mock
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)

import labkit  # noqa: E402


class LabRun:
    """The foxglove-lab subprocess, faked: writes the report.json that suite() reads back."""

    def __init__(self, report):
        self.report, self.calls = report, []

    def __call__(self, cmd, **kw):
        self.calls.append((cmd, kw))
        out = Path(cmd[cmd.index('--out') + 1])
        out.mkdir(parents=True, exist_ok=True)
        (out / 'report.json').write_text(json.dumps(self.report), encoding='utf-8')
        return subprocess.CompletedProcess(cmd, 0)


TMP_REPO = Path(tempfile.mkdtemp(prefix='resilience-test-repo-'))
PASS_REPORT = {'cases': [{'status': 'PASS', 'steps': [{'actual': 'lab ok'}]}]}

# resilience.py runs its row at import and ends in check.finish() (a sys.exit): run row 1 once,
# faking the lab binary, the lock state and the repo root; --lab names an existing file because
# argparse validates the tool_arg defaults and the lab build is not here.
_old_argv = sys.argv
sys.argv = ['resilience.py', '--row', '1', '--lab', sys.executable]
with mock.patch.object(subprocess, 'run', LabRun(PASS_REPORT)), \
        mock.patch.object(labkit, 'locked', lambda: True), \
        mock.patch.object(labkit, 'REPO', TMP_REPO), \
        mock.patch.object(sys, 'exit', lambda code=None: None), \
        mock.patch('sys.stdout', io.StringIO()) as _out:
    import resilience  # noqa: E402
sys.argv = _old_argv
IMPORT_OUT = _out.getvalue()


class Resilience(unittest.TestCase):
    def test_the_row_1_run_recorded_fresh_boot_pass_and_typing_blocked(self):
        self.assertEqual(resilience.check.results[:2], ['PASS', 'BLOCKED'])
        self.assertIn('PASS fresh boot: the right shows a built-in set', IMPORT_OUT)
        self.assertIn('BLOCKED typing on both halves: the PC is locked', IMPORT_OUT)
        self.assertIn('RESULT BLOCKED', IMPORT_OUT)
        self.assertTrue(TMP_REPO.is_relative_to(tempfile.gettempdir()))

    def test_the_fresh_boot_suite_is_valid_toml_under_the_temp_repo(self):
        suites = list((TMP_REPO / 'lab-reports').glob('resilience-fresh-boot-*.toml'))
        self.assertEqual(len(suites), 1, 'row 1 wrote its fresh-boot suite to the temp repo')
        text = suites[0].read_text(encoding='utf-8')
        self.assertIn('art_select', text)
        self.assertIn('expect_layer', text)
        case = tomllib.loads(text)['cases'][0]
        self.assertEqual(case['name'], 'fresh-boot')
        self.assertEqual(len(case['steps']), 9)  # the art select plus the eight layer-key steps

    def test_suite_writes_a_runnable_toml_and_reads_the_report_back(self):
        run = LabRun({'cases': [{'status': 'PASS', 'steps': [{'actual': 'first'}, {'actual': 'second'}]}]})
        with mock.patch.object(subprocess, 'run', run):
            self.assertEqual(resilience.suite('my case', ['{ a = 1 }', '{ b = 2 }']), ('PASS', ['first', 'second']))
        cmd, kw = run.calls[0]
        self.assertEqual(kw['cwd'], TMP_REPO)
        self.assertIn('--non-interactive', cmd)
        case = tomllib.loads(Path(cmd[cmd.index('--suite') + 1]).read_text(encoding='utf-8'))['cases'][0]
        self.assertEqual(case['name'], 'my case')
        self.assertEqual(case['steps'], [{'a': 1}, {'b': 2}])

    def test_suite_passes_a_fail_status_and_each_steps_actual_through(self):
        run = LabRun({'cases': [{'status': 'FAIL', 'steps': [{'actual': 'ok'}, {'actual': 'the frame is gone'}]}]})
        with mock.patch.object(subprocess, 'run', run):
            self.assertEqual(resilience.suite('kept-10s', ['{ panel_dump = { path = "x" } }']),
                             ('FAIL', ['ok', 'the frame is gone']))

    def test_typing_is_blocked_and_runs_nothing_when_the_pc_is_locked(self):
        run = LabRun(PASS_REPORT)
        out = io.StringIO()
        with mock.patch.object(subprocess, 'run', run), \
                mock.patch.object(resilience, 'locked', lambda: True), \
                mock.patch('sys.stdout', out):
            self.assertFalse(resilience.typing())
        self.assertEqual(run.calls, [])
        self.assertIn('BLOCKED typing on both halves: the PC is locked: the typing sink needs it unlocked',
                      out.getvalue())

    def test_typing_drives_the_typing_suite_when_the_pc_is_unlocked(self):
        run = LabRun({'cases': [{'status': 'PASS', 'steps': [{'actual': 'qpfj'}]}]})
        out = io.StringIO()
        with mock.patch.object(subprocess, 'run', run), \
                mock.patch.object(resilience, 'locked', lambda: False), \
                mock.patch('sys.stdout', out):
            self.assertIsNone(resilience.typing())  # the unlocked branch records, it returns nothing
        self.assertEqual(resilience.check.results[-1], 'PASS')
        cmd, kw = run.calls[0]
        self.assertEqual(kw['cwd'], TMP_REPO)
        text = Path(cmd[cmd.index('--suite') + 1]).read_text(encoding='utf-8')
        self.assertIn('type_into_sink', text)
        self.assertIn('qpfj', text)
        self.assertIn('PASS typing on both halves:', out.getvalue())


if __name__ == '__main__':
    unittest.main()
