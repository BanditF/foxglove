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
FIRST = {'private_b': 30 * MB, 'working_set_b': 50 * MB, 'cpu_100ns': 0, 'ctx_switches': 1000,
         'gdi': 90, 'user': 40, 'vram_b': None}
LAST = {**FIRST, 'cpu_100ns': 3_000_000, 'ctx_switches': 1600}  # 0.3 s CPU and 600 switches in 60 s
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


class WakeupsUnavailable(unittest.TestCase):
    def test_no_counter_gives_none_not_an_error(self):
        m = fc.summarize({**FIRST, 'ctx_switches': None}, {**LAST, 'ctx_switches': None}, 60)
        self.assertIsNone(m['wakeups_per_s'])
        self.assertEqual(m['cpu_pct'], 0.5)

    def test_a_baselined_wakeups_metric_without_a_reading_is_skipped(self):
        rows = fc.compare({'wakeups_per_s': None}, {'wakeups_per_s': {'value': 30, 'pct': 50}})
        self.assertEqual(rows[3][3], 'SKIP')
        self.assertEqual(fc.verdict(rows), 'PASS')

    def test_a_metric_missing_from_the_baseline_is_not_compared(self):
        rows = dict((r[0], r[3]) for r in fc.compare({'wakeups_per_s': 9999}, {}))
        self.assertEqual(rows['wakeups_per_s'], 'SKIP')


class Thresholds(unittest.TestCase):
    def test_limit_is_pct_plus_slack(self):
        self.assertAlmostEqual(fc.limit({'value': 100, 'pct': 10, 'slack': 5}), 115)
        self.assertEqual(fc.limit({'value': 7}), 7)

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
            self.assertTrue(set(base['sets'][name]) <= set(fc.METRICS))
            for e in base['sets'][name].values():
                self.assertGreater(e['value'], 0)

    def test_record_is_per_batch(self):
        d = json.loads(fc.record('b1', {'none': {'verdict': 'PASS'}}))
        self.assertEqual((d['batch'], d['sets']['none']['verdict']), ('b1', 'PASS'))


if __name__ == '__main__':
    unittest.main()
