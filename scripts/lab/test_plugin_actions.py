"""The pure helpers of plugin_actions (#212): the focus timer's facts and the lab's verdicts.

    python -m unittest test_plugin_actions -v   # from scripts/lab/
"""
import ctypes
import io
import os
import subprocess
import sys
import tempfile
import time
import unittest
import unittest.mock as mock
from pathlib import Path

LAB = Path(__file__).resolve().parent
sys.path.insert(0, str(LAB))


def load(plugin):
    """Imports plugin_actions with the board check faked (timer counting, F13 passing); (module, [exits])."""
    argv = ['plugin_actions.py', '--app', sys.executable, '--cli', sys.executable,
            '--lab', sys.executable, '--plugin', str(plugin)]
    state = {'taps': 0, 'reads': 0, 'exits': []}
    def run(args, **kw):
        if args and args[0] == 'powershell':
            out = ''
        elif '--suite' in args:
            state['taps'] += 1
            out = 'PASS f13-tap\n'
        elif state['taps'] % 2:  # a tap paused the timer: the remaining holds still
            out = 'focus.paused = true\nfocus.remaining = 40\n'
        else:
            state['reads'] += 1
            out = f'focus.paused = false\nfocus.remaining = {max(10, 100 - 10 * state["reads"])}\n'
        return subprocess.CompletedProcess(args, 0, stdout=out, stderr='')

    with mock.patch.object(sys, 'argv', argv), mock.patch.object(sys, 'stdout', io.StringIO()), \
            mock.patch.object(sys, 'exit', side_effect=lambda code=None: state['exits'].append(code)), \
            mock.patch.object(subprocess, 'run', side_effect=run), \
            mock.patch.object(subprocess, 'Popen', return_value=mock.MagicMock()), \
            mock.patch.object(time, 'sleep'), \
            mock.patch.object(ctypes, 'windll', mock.MagicMock(**{'user32.OpenInputDesktop.return_value': 1}), create=True):  # no windll on Linux CI
        import plugin_actions
    return plugin_actions, state['exits']


class PluginActions(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory()
        appdata = Path(cls._tmp.name) / 'appdata'
        cls.plugin = Path(cls._tmp.name) / 'plugin'
        cls.plugin.mkdir()
        (cls.plugin / 'plugin.toml').write_text('schema = 1\n', encoding='utf-8')
        old = os.environ.get('APPDATA')
        os.environ['APPDATA'] = str(appdata)  # labkit reads it at import; the user's files stay put
        try:
            cls.pa, cls.exits = load(cls.plugin)
            cls.checks_at_load = list(cls.pa.results)
        finally:
            if old is None: del os.environ['APPDATA']
            else: os.environ['APPDATA'] = old

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def test_remaining_reads_the_focus_fact(self):
        self.assertEqual(self.pa.remaining({'focus.remaining': '123'}), 123)
        self.assertEqual(self.pa.remaining({'focus.remaining': '-5'}), -5)
        self.assertIsNone(self.pa.remaining({}))
        self.assertIsNone(self.pa.remaining({'focus.remaining': '3.5'}))
        self.assertIsNone(self.pa.remaining({'focus.remaining': 'n/a'}))

    def test_facts_reads_the_timer_from_the_cli(self):
        done = subprocess.CompletedProcess(['cli'], 0, stdout='focus.paused = false\nfocus.remaining = 30\n', stderr='')
        with mock.patch.object(subprocess, 'run', return_value=done) as r:
            self.assertEqual(self.pa.facts(), {'focus.paused': 'false', 'focus.remaining': '30'})
        self.assertEqual(r.call_args[0][0][-2:], ['facts', 'focus.*'])

    def test_check_prints_the_verdict_and_records_it(self):
        out = io.StringIO()
        with mock.patch.object(sys, 'stdout', out):
            self.pa.check('the timer runs', True, 'focus.remaining 90 -> 80')
            self.pa.check('the key fires', False, 'no hotkey')
        self.assertEqual(self.pa.results[-2:], [True, False])
        self.assertEqual(out.getvalue(), 'PASS the timer runs: focus.remaining 90 -> 80\nFAIL the key fires: no hotkey\n')

    def test_counting_sees_the_timer_run_and_hold(self):
        outs = iter(['focus.remaining = 90\n', 'focus.remaining = 80\n', 'focus.remaining = 80\n', 'focus.remaining = 80\n'])
        run = lambda *a, **q: subprocess.CompletedProcess(a, 0, stdout=next(outs), stderr='')
        with mock.patch.object(subprocess, 'run', side_effect=run), mock.patch.object(time, 'sleep'):
            self.assertEqual([self.pa.counting(0), self.pa.counting(0)], [(True, (90, 80)), (False, (80, 80))])

    def test_wait_facts_returns_the_facts_or_gives_up_at_the_deadline(self):
        clock = {'t': 1000.0}
        with mock.patch.object(subprocess, 'run',
                               return_value=subprocess.CompletedProcess(['cli'], 0, stdout='focus.paused = false\n', stderr='')), \
                mock.patch.object(time, 'time', side_effect=lambda: clock['t']), \
                mock.patch.object(time, 'sleep', side_effect=lambda s: clock.update(t=clock['t'] + s)):
            ok, f = self.pa.wait_facts(lambda f: f.get('focus.paused') == 'true', 3)
        self.assertEqual((ok, f), (False, {'focus.paused': 'false'}))

    def test_tap_f13_reads_the_lab_verdict(self):
        done = subprocess.CompletedProcess(['lab'], 0, stdout='running suite\nPASS f13-tap\n', stderr='')
        with mock.patch.object(subprocess, 'run', return_value=done) as r:
            self.assertEqual(self.pa.tap_f13(), (True, ['PASS f13-tap']))
        self.assertIn('app-actions.toml', ' '.join(map(str, r.call_args[0][0])))
        with mock.patch.object(subprocess, 'run', return_value=subprocess.CompletedProcess(['lab'], 0, stdout='FAIL f13-tap\n', stderr='')):
            self.assertEqual(self.pa.tap_f13(), (False, ['FAIL f13-tap']))

    def test_the_board_check_passes_end_to_end_under_the_fakes(self):
        self.assertEqual(self.checks_at_load, [True] * 7)
        self.assertEqual(self.exits, [0])


if __name__ == '__main__':
    unittest.main()
