"""appdata_redirect (#894): the redirect rule on recorded paths, and the once-a-run notice with a
fake GetFinalPathNameByHandleW. Never reads where a real file went."""
import io
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import appdata_redirect as ar  # noqa: E402

ASKED = r'C:\Users\fuchs\AppData\Local\Foxglove\flash-history.log'
COPY = r'C:\Users\fuchs\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\Foxglove\flash-history.log'


class Rule(unittest.TestCase):
    def test_another_apps_local_cache_is_a_redirect(self):
        # Measured 11/07: the history only in the Claude app's copy.
        self.assertEqual(ar.redirected(ASKED, '\\\\?\\' + COPY), ('Claude_pzs8sxrjxfjjc', COPY))

    def test_the_same_file_or_another_place_is_not(self):
        self.assertIsNone(ar.redirected(ASKED, '\\\\?\\' + ASKED))
        self.assertIsNone(ar.redirected(ASKED, '\\\\?\\' + ASKED.upper()))
        self.assertIsNone(ar.redirected(ASKED, r'\\?\D:\Local\Foxglove\flash-history.log'))
        self.assertIsNone(ar.redirected(
            ASKED, r'\\?\C:\Users\fuchs\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\RoamingState\x.log'))
        self.assertIsNone(ar.redirected(
            ASKED, r'\\?\C:\Users\fuchs\AppData\Local\Packages\BanditF.Foxglove_abc\LocalCache\Local\x.log'))

    def test_the_rule_matches_the_rust_crate(self):
        # crates/redirect keeps the same name for Foxglove's own package.
        lib = Path(__file__).resolve().parents[1] / 'crates' / 'redirect' / 'src' / 'lib.rs'
        self.assertIn(f'pub const OWN_PACKAGE: &str = "{ar.OWN_PACKAGE}";', lib.read_text(encoding='utf-8'))


class Notice(unittest.TestCase):
    def test_a_redirect_is_said_once(self):
        out = io.StringIO()
        n = ar.Notice('the flash history', final=lambda p: '\\\\?\\' + COPY)
        self.assertEqual(n.after_write(ASKED, out), ('Claude_pzs8sxrjxfjjc', COPY))
        self.assertIsNone(n.after_write(ASKED, out))
        text = out.getvalue()
        self.assertEqual(text.count('\n'), 1)
        self.assertIn(f"the flash history is in another app's private copy: {COPY}", text)
        self.assertIn("inside Claude_pzs8sxrjxfjjc's package", text)

    def test_the_real_file_or_no_answer_says_nothing(self):
        for final in (lambda p: '\\\\?\\' + ASKED, lambda p: None):
            out = io.StringIO()
            self.assertIsNone(ar.Notice('x', final=final).after_write(ASKED, out))
            self.assertEqual(out.getvalue(), '')

    def test_a_missing_file_has_no_final_path(self):
        self.assertIsNone(ar.final_path(Path(__file__).with_name('no-such-file.log')))


if __name__ == '__main__':
    unittest.main()
