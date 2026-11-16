"""right_redraws.py's parse_redraws helper: extracts RIGHT_REDRAWS from foxglove-lab status output."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import right_redraws as rr  # noqa: E402


class ParseRedraws(unittest.TestCase):
    def test_valid_count_is_returned_as_int(self):
        self.assertEqual(rr.parse_redraws('RIGHT_REDRAWS: 5\n'), 5)

    def test_leading_trailing_text_does_not_affect_extraction(self):
        out = 'some header\nRIGHT_REDRAWS: 42\nmore text'
        self.assertEqual(rr.parse_redraws(out), 42)

    def test_no_match_returns_none(self):
        self.assertIsNone(rr.parse_redraws('nothing here'))

    def test_blank_string_returns_none(self):
        self.assertIsNone(rr.parse_redraws(''))

    def test_non_integer_value_is_not_matched(self):
        out = 'RIGHT_REDRAWS: abc\n'
        result = rr.parse_redraws(out)
        self.assertIsNone(result, "non-integer RIGHT_REDRAWS should return None")


if __name__ == '__main__':
    unittest.main()
