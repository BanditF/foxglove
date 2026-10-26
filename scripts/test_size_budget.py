import tempfile
import unittest
from pathlib import Path

import size_budget as sb


class SizeBudget(unittest.TestCase):
    def test_counts_a_function_from_fn_to_its_closing_brace(self):
        text = 'fn a() {\n    let x = 1;\n}\n\npub(crate) async fn b(\n    y: u8,\n) -> u8 {\n    y\n}\n'
        self.assertEqual(sb.functions(text), [('a', 1, 3), ('b', 5, 5)])

    def test_braces_in_strings_comments_and_chars_dont_count(self):
        text = ('fn a() {\n'
                '    let s = "}}}";  // }\n'
                '    let r = r#"{"#;\n'
                '    let c = \'}\';\n'
                '    /* { */\n'
                '    let l: &\'static str = "";\n'
                '}\n')
        self.assertEqual(sb.functions(text), [('a', 1, 7)])

    def test_nested_functions_and_bodyless_declarations(self):
        text = 'trait T {\n    fn x(&self);\n}\nfn outer() {\n    fn inner() {\n    }\n}\n'
        self.assertEqual(sorted(sb.functions(text)), [('inner', 5, 2), ('outer', 4, 4)])

    def test_scan_lists_offenders_without_failing(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'crates' / 'x' / 'src').mkdir(parents=True)
            (root / 'app' / 'src' / 'foxglove').mkdir(parents=True)
            body = 'fn long() {\n' + '    let a = 1;\n' * 130 + '}\nfn short() {\n}\n'
            (root / 'crates' / 'x' / 'src' / 'lib.rs').write_text(body + '\n' * 1500)
            files, fns = sb.scan(root)
            self.assertEqual([f[0] for f in files], ['crates/x/src/lib.rs'])
            self.assertEqual([(f[2], f[3]) for f in fns], [('long', 132)])
        self.assertEqual(sb.main([]), 0, 'warnings only: the gate never fails on the budget')


if __name__ == '__main__':
    unittest.main()
