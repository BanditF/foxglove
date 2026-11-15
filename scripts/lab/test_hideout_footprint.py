"""hideout_footprint.py's pure helpers: log_size, log_since, update_baseline."""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import footprint_compare as fc  # noqa: E402
import hideout_footprint as hf  # noqa: E402


class LogSize(unittest.TestCase):
    def test_existing_file_returns_st_size(self):
        with tempfile.NamedTemporaryFile(mode="wb", suffix=".log") as f:
            f.write(b"hello world")
            f.flush()
            self.assertEqual(hf.log_size(Path(f.name)), len(b"hello world"))

    def test_missing_file_returns_zero(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(hf.log_size(Path(d) / "no-such-file.tmp"), 0)


class LogSince(unittest.TestCase):
    def test_reads_text_from_offset(self):
        with tempfile.NamedTemporaryFile(mode="wb", suffix=".log", delete=False) as f:
            f.write(b"first line\nsecond line\nthird line")
            path = f.name
        offset = len(b"first line\n")
        self.assertEqual(hf.log_since(path, offset), "second line\nthird line")

    def test_missing_file_returns_empty_string(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(hf.log_since(Path(d) / "missing.log", 0), "")


class UpdateBaseline(unittest.TestCase):
    def test_sets_value_for_non_info_metric(self):
        baseline = {}
        results = {"s1": {"metrics": {"private_mb": 150.3}}}
        updated = hf.update_baseline(baseline, results)
        key = fc.baseline_key("private_mb")
        self.assertEqual(updated["sets"]["s1"][key]["value"], 150.3)

    def test_skips_info_metrics(self):
        baseline = {}
        results = {"s1": {"metrics": {"wakeups_per_s": 42}}}
        hf.update_baseline(baseline, results)
        self.assertEqual(baseline["sets"]["s1"], {})

    def test_preserves_pct_and_slack_on_existing_entry(self):
        key = fc.baseline_key("private_mb")
        baseline = {"sets": {"s1": {key: {"value": 100, "pct": 15, "slack": 2}}}}
        results = {"s1": {"metrics": {"private_mb": 110}}}
        updated = hf.update_baseline(baseline, results)
        entry = updated["sets"]["s1"][key]
        self.assertEqual(entry["value"], 110)
        self.assertEqual(entry["pct"], 15)
        self.assertEqual(entry["slack"], 2)

    def test_loop_wakes_shown_per_s_for_shown_state(self):
        baseline = {}
        results = {"s1": {"metrics": {"loop_wakes_per_s": 0.8}}}
        hf.update_baseline(baseline, results, state="shown")
        entry = baseline["sets"]["s1"][fc.baseline_key("loop_wakes_per_s", "shown")]
        self.assertEqual(entry["value"], 0.8)


if __name__ == "__main__":
    unittest.main()
