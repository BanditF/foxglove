"""The hideout idle-footprint case's threshold and compare logic (#935). Window-free: nothing is
launched and nothing reads a process.

    python -m unittest scripts/test_hideout_footprint.py
"""
import json
import sys
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'
sys.path.insert(0, str(LAB))
import footprint_compare as fc  # noqa: E402

MB = 1024 * 1024
FIRST = {'private_b': 30 * MB, 'working_set_b': 50 * MB, 'cpu_100ns': 0, 'threads': {1: 900, 2: 100},
         'gdi': 90, 'user': 40, 'vram_b': None}
# 0.3 s CPU; 600 switches in 60 s over three threads (thread 3 was born in the window).
LAST = {**FIRST, 'cpu_100ns': 3_000_000, 'threads': {1: 1400, 2: 150, 3: 50}}
BASE = {'private_mb': {'value': 30, 'pct': 10}, 'cpu_pct': {'value': 0.5, 'pct': 0, 'slack': 0.1},
        'gdi_handles': {'value': 90}, 'vram_mb': {'value': 20, 'pct': 10}}


class Summary(unittest.TestCase):
    def test_rates_over_the_window(self):
        m = fc.summarize(FIRST, LAST, 60)
        self.assertEqual((m['cpu_pct'], m['wakeups_per_s']), (0.5, 10.0))
        self.assertEqual((m['private_mb'], m['working_set_mb'], m['gdi_handles'], m['user_handles']),
                         (30.0, 50.0, 90, 40))
        self.assertIsNone(m['vram_mb'])

    def test_vram_when_there_is_a_counter(self):
        self.assertEqual(fc.summarize(FIRST, {**LAST, 'vram_b': 8 * MB}, 60)['vram_mb'], 8.0)

    def test_an_empty_window_is_refused(self):
        with self.assertRaises(ValueError):
            fc.summarize(FIRST, LAST, 0)


class Threads(unittest.TestCase):
    def test_wakeups_sum_every_thread_not_the_process_counter(self):
        # The Process-class counter read 0 for the hub; only the threads carry the switches.
        first = {**FIRST, 'threads': {10: 0, 11: 0}}
        last = {**LAST, 'threads': {10: 90_000, 11: 600}}
        self.assertEqual(fc.summarize(first, last, 60)['wakeups_per_s'], 1510.0)

    def test_a_reused_thread_id_counts_from_zero(self):
        m = fc.summarize({**FIRST, 'threads': {1: 5000}}, {**LAST, 'threads': {1: 120}}, 60)
        self.assertEqual(m['wakeups_per_s'], 2.0)

    def test_top_three_threads_highest_first_with_names(self):
        first = {1: 0, 2: 0, 3: 0, 4: 0}
        last = {1: 60, 2: 90_000, 3: 600, 4: 6}
        top = fc.top_threads(first, last, 60, {2: 'main', 3: ''})
        self.assertEqual(top, [{'tid': 2, 'name': 'main', 'switches_per_s': 1500.0},
                               {'tid': 3, 'name': '', 'switches_per_s': 10.0},
                               {'tid': 1, 'name': '', 'switches_per_s': 1.0}])

    def test_no_thread_counts_give_no_top_threads(self):
        self.assertEqual(fc.top_threads(None, None, 60), [])


class WakeupsUnavailable(unittest.TestCase):
    def test_no_counter_gives_none_not_an_error(self):
        m = fc.summarize({**FIRST, 'threads': None}, {**LAST, 'threads': None}, 60)
        self.assertIsNone(m['wakeups_per_s'])
        self.assertEqual(m['cpu_pct'], 0.5)

    def test_context_switches_are_info_never_a_failure(self):
        # #1000: thread readies are OS and driver work; they are recorded, never compared.
        for got in (None, 12.0, 9999.0):
            rows = {r[0]: r for r in fc.compare({'wakeups_per_s': got}, {'wakeups_per_s': {'value': 1}})}
            self.assertEqual(rows['wakeups_per_s'], ('wakeups_per_s', got, None, 'INFO'))
        self.assertEqual(fc.verdict(fc.compare({'wakeups_per_s': 9999.0}, {})), 'PASS')


class LoopWakes(unittest.TestCase):
    LINES = (
        '2026-11-10T08:00:15Z INFO  hideout_shell::wake: event loop: 40 wakes in 15 s (repaint 30, timer 1, '
        'window 9, accesskit 0, device 0, poll 0, other 0)\n'
        '2026-11-10T08:00:30Z INFO  hideout_shell::wake: event loop: 2 wakes in 15 s (repaint 0, timer 1, '
        'window 0, accesskit 0, device 0, poll 0, other 1)\n'
        '2026-11-10T08:00:31Z INFO  hideout: something else\n'
        '2026-11-10T08:00:45Z INFO  hideout_shell::wake: event loop: 1 wakes in 15 s (repaint 0, timer 1, '
        'window 0, accesskit 0, device 0, poll 0, other 0)\n')

    def test_the_rate_comes_from_the_hubs_own_lines_without_the_first(self):
        # The first report's period began before the window (settling, the close to the tray).
        rate, causes = fc.loop_wakes(self.LINES)
        self.assertEqual(rate, 0.1)  # 3 wakes in 30 s
        self.assertEqual(causes, {'repaint': 0, 'timer': 2, 'window': 0, 'accesskit': 0, 'device': 0,
                                  'poll': 0, 'other': 1})

    def test_the_other_split_after_the_causes_changes_nothing(self):
        # #1018: the report names the messages of the `other` wakes after the causes.
        line = ('event loop: 30 wakes in 15 s (repaint 0, timer 1, window 0, accesskit 0, device 0, poll 0, '
                "other 29); other (0x0113 WM_TIMER#6007 tray 20, 0x003D WM_GETOBJECT main sent 5, "
                "0xC2FA TaskbarButtonCreated 'Winit Thread Event Target' 2, more 1, unseen 3)\n")
        rate, causes = fc.loop_wakes(line + line)
        self.assertEqual(rate, 2.0)
        self.assertEqual(causes['other'], 29)
        self.assertEqual(causes['timer'], 1)
        self.assertEqual(len(causes), 7)

    def test_no_whole_period_gives_no_reading(self):
        self.assertEqual(fc.loop_wakes(''), (None, {}))
        self.assertEqual(fc.loop_wakes(self.LINES.splitlines()[0]), (None, {}))

    def test_a_line_without_causes_still_counts(self):
        rate, causes = fc.loop_wakes('event loop: 9 wakes in 10 s\nevent loop: 30 wakes in 20 s\n')
        self.assertEqual((rate, causes), (1.5, {}))

    def test_hidden_is_at_most_one_per_second_whatever_the_value(self):
        base = {'loop_wakes_per_s': {'value': 0.08, 'max': 1}, 'loop_wakes_shown_per_s': {'value': 0.1, 'slack': 5}}
        status = lambda got, state: {r[0]: r[3] for r in fc.compare({'loop_wakes_per_s': got}, base, state)}[
            'loop_wakes_per_s']
        self.assertEqual(status(1.0, 'hidden'), 'PASS')
        self.assertEqual(status(1.97, 'hidden'), 'FAIL')  # d333bd52: 1182 wakes in 10 min
        # A shown window has its own limit.
        self.assertEqual(status(1.97, 'shown'), 'PASS')
        self.assertEqual(status(5.2, 'shown'), 'FAIL')

    def test_a_missing_loop_reading_blocks(self):
        rows = fc.compare({'loop_wakes_per_s': None}, {'loop_wakes_per_s': {'value': 0.08, 'max': 1}})
        self.assertEqual(fc.verdict(rows), 'BLOCKED')

    def test_the_baseline_key_is_per_state(self):
        self.assertEqual(fc.baseline_key('loop_wakes_per_s'), 'loop_wakes_per_s')
        self.assertEqual(fc.baseline_key('loop_wakes_per_s', 'shown'), 'loop_wakes_shown_per_s')
        self.assertEqual(fc.baseline_key('private_mb', 'shown'), 'private_mb')

    def test_a_metric_missing_from_the_baseline_is_not_compared(self):
        rows = dict((r[0], r[3]) for r in fc.compare({'loop_wakes_per_s': 9999}, {}))
        self.assertEqual(rows['loop_wakes_per_s'], 'SKIP')


class Thresholds(unittest.TestCase):
    def test_limit_is_pct_plus_slack(self):
        self.assertAlmostEqual(fc.limit({'value': 100, 'pct': 10, 'slack': 5}), 115)
        self.assertEqual(fc.limit({'value': 7}), 7)

    def test_a_max_is_a_fixed_limit(self):
        self.assertEqual(fc.limit({'value': 0.08, 'max': 1, 'pct': 50, 'slack': 3}), 1)

    def test_at_the_limit_passes_past_it_fails(self):
        rows = {r[0]: r for r in fc.compare({'private_mb': 33.0, 'cpu_pct': 0.61}, BASE)}
        self.assertEqual(rows['private_mb'][3], 'PASS')  # 30 * 1.1 = 33
        self.assertEqual(rows['cpu_pct'][3], 'FAIL')  # 0.5 + 0.1 = 0.6
        m = {'private_mb': 33.0, 'cpu_pct': 0.61, 'gdi_handles': 90}
        self.assertEqual(fc.verdict(fc.compare(m, BASE)), 'FAIL')

    def test_all_within_passes(self):
        m = {'private_mb': 31, 'cpu_pct': 0.4, 'gdi_handles': 90}
        self.assertEqual(fc.verdict(fc.compare(m, BASE)), 'PASS')

    def test_no_baseline_is_skipped_not_failed(self):
        rows = {r[0]: r[3] for r in fc.compare({'working_set_mb': 9999}, BASE)}
        self.assertEqual(rows['working_set_mb'], 'SKIP')

    def test_vram_without_a_counter_is_skipped_other_missing_blocks(self):
        rows = {r[0]: r[3] for r in fc.compare({'private_mb': 30, 'vram_mb': None}, BASE)}
        self.assertEqual(rows['vram_mb'], 'SKIP')
        self.assertEqual(rows['gdi_handles'], 'MISSING')
        self.assertEqual(fc.verdict(fc.compare({'private_mb': 30, 'cpu_pct': 0.1}, BASE)), 'BLOCKED')

    def test_a_fail_outranks_a_missing_reading(self):
        self.assertEqual(fc.verdict(fc.compare({'private_mb': 99}, BASE)), 'FAIL')


class Files(unittest.TestCase):
    def test_sets_always_have_none_and_the_default_is_empty_for_now(self):
        path = LAB.parents[1] / 'tests' / 'hil' / 'hideout-footprint-sets.json'
        self.assertEqual(fc.load_sets(path.read_text(encoding='utf-8')), {'none': [], 'default': []})

    def test_sets_are_data_driven(self):
        self.assertEqual(fc.load_sets('{"sets": {"default": ["a/b", "c"]}}'),
                         {'none': [], 'default': ['a/b', 'c']})

    def test_bad_sets_are_refused(self):
        for bad in ('{"sets": {"none": ["x"]}}', '{"sets": {"default": "x"}}', '{"sets": {"default": [1]}}',
                    '{"sets": []}'):
            with self.assertRaises(ValueError, msg=bad):
                fc.load_sets(bad)

    def test_committed_baseline_covers_every_set_and_known_metrics(self):
        path = LAB.parents[1] / 'tests' / 'hil' / 'hideout-footprint-baseline.json'
        base = json.loads(path.read_text(encoding='utf-8'))
        for name in ('none', 'default'):
            self.assertIn(name, base['sets'])
            self.assertTrue(set(base['sets'][name]) <= set(fc.BASELINE_KEYS))
            for key, e in base['sets'][name].items():
                # An idle hub can measure 0.0 % CPU (case 7 on 1aa0393); the rest must be real.
                if key == 'cpu_pct':
                    self.assertGreaterEqual(e['value'], 0)
                else:
                    self.assertGreater(e['value'], 0)
            # The gate's wake limits (#1000, Hub 11/10): at most 2 loop wakes/s hidden (no
            # flyout) and 5/s shown.
            self.assertEqual(fc.limit(base['sets'][name]['loop_wakes_per_s']), 2)
            self.assertEqual(fc.limit(base['sets'][name]['loop_wakes_shown_per_s']), 5)
            self.assertIn('loop_wakes_shown_per_s', base['sets'][name])

    def test_record_is_per_batch(self):
        d = json.loads(fc.record('b1', {'none': {'verdict': 'PASS'}}))
        self.assertEqual((d['batch'], d['sets']['none']['verdict']), ('b1', 'PASS'))


if __name__ == '__main__':
    unittest.main()
