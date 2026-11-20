import unittest

import check_window_tests as lint


class WindowTestLint(unittest.TestCase):
    def test_scanner_self_test(self):
        lint.self_test()

    def test_the_repo_has_no_unvetted_window_tests(self):
        allow = lint.allowed_set()
        bad = []
        for p in lint.tracked_rs():
            try:
                text = (lint.ROOT / p).read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            bad += [f"{p}:{h[0]} {h[3]}" for h in lint.scan(p, text, allow)]
        self.assertEqual(bad, [])


if __name__ == "__main__":
    unittest.main()
