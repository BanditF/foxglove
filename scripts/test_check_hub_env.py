"""scripts/check_hub_env.py (#1466): hub code reads the environment only behind the
`lab-switches` gate or at a listed site, and every switch name is one the installed build logs.

    python -m unittest scripts/test_check_hub_env.py
"""
import unittest

import check_hub_env as lint

LAB = '''
pub const LAB_ENV: &str = "HIDEOUT_LAB";
fn ignored_switches() {
    const VARS: [&str; 2] = [
        LAB_ENV,
        "HIDEOUT_HOME",
    ];
}
'''
F = 'crates/hideout/src/x.rs'


def problems(text, open_sites=None, rel=F):
    return lint.check({rel: text}, LAB, {} if open_sites is None else open_sites)


class HubEnvReads(unittest.TestCase):
    def test_the_repo_is_clean(self):
        files = lint.hub_files()
        self.assertGreater(len(files), 50)
        self.assertEqual(lint.check(files, files[lint.LAB_RS]), [])

    def test_an_ungated_read_fails(self):
        for read in ('std::env::var("HIDEOUT_HOME")', 'env::var_os(k)', 'std::env::vars_os()',
                     'std::env::vars ()'):
            p = problems(f'fn f() {{\n    let v = {read};\n}}\n')
            self.assertEqual(len(p), 1, read)
            self.assertIn(f'{F}:2: fn f reads the environment', p[0])

    def test_a_gated_function_passes(self):
        attr = ('/// Doc.\n#[cfg(feature = "lab-switches")]\npub(crate) fn f() {\n'
                '    std::env::var("HIDEOUT_HOME");\n}\n')
        macro = ('fn f() {\n    if cfg!(feature = "lab-switches") {\n'
                 '        std::env::var("HIDEOUT_HOME");\n    }\n}\n')
        inner = ('fn f() {\n    #[cfg(feature = "lab-switches")]\n'
                 '    for k in K { std::env::var_os(k); }\n}\n')
        for text in (attr, macro, inner):
            self.assertEqual(problems(text), [], text)

    def test_a_gate_on_another_function_does_not_count(self):
        text = ('#[cfg(feature = "lab-switches")]\nfn a() {}\n\n'
                'fn b() {\n    std::env::var_os(k);\n}\n')
        self.assertIn('fn b reads', problems(text)[0])
        off = '#[cfg(not(feature = "lab-switches"))]\nfn f() {\n    std::env::var_os(k);\n}\n'
        self.assertEqual(len(problems(off)), 1)
        after = 'fn f() {\n    std::env::var_os(k);\n    cfg!(feature = "lab-switches");\n}\n'
        self.assertEqual(len(problems(after)), 1)

    def test_system_variables_and_comments_pass(self):
        text = ('fn f() {\n    std::env::var("SystemRoot");\n    std::env::var_os("USERPROFILE");\n'
                '    // std::env::var("HIDEOUT_NEW")\n}\n/// `std::env::var_os` in the hub.\n')
        self.assertEqual(problems(text), [])

    def test_a_listed_site_passes_and_a_stale_one_fails(self):
        text = 'fn f() {\n    std::env::var_os(k);\n}\n'
        self.assertEqual(problems(text, {(F, 'f'): 'why'}), [])
        stale = problems('fn f() {}\n', {(F, 'f'): 'why'})
        self.assertEqual(len(stale), 1)
        self.assertIn('reads no environment now', stale[0])

    def test_test_code_is_skipped(self):
        text = ('fn f() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {\n'
                '        std::env::var("HIDEOUT_REVIEW_DIR");\n    }\n}\n')
        self.assertEqual(problems(text), [])
        read = 'fn t() {\n    std::env::var("HIDEOUT_REVIEW_DIR");\n}\n'
        for rel in ('crates/hideout/src/tests.rs', 'crates/hideout/src/a_tests.rs',
                    'crates/hideout/src/quick/tests/a.rs'):
            self.assertEqual(problems(read, rel=rel), [], rel)
        # A `#[cfg(test)]` on one item is not the test module: what follows is still checked.
        item = '#[cfg(test)]\nfn only_tests() {}\n\nfn f() {\n    std::env::var_os(k);\n}\n'
        self.assertEqual(len(problems(item)), 1)

    def test_a_switch_name_the_build_does_not_log_fails(self):
        gated = '#[cfg(feature = "lab-switches")]\nfn f() {\n    std::env::var("HIDEOUT_NEW");\n}\n'
        p = problems(gated)
        self.assertEqual(len(p), 1)
        self.assertIn('HIDEOUT_NEW is not in the ignored-switch list', p[0])
        ok = 'const A: [&str; 3] = ["HIDEOUT_LAB", "HIDEOUT_HOME", "HIDEOUT_PIPE"];\n'
        self.assertEqual(problems(ok), [])

    def test_a_missing_list_fails(self):
        p = lint.check({F: 'fn f() {}\n'}, 'nothing here', {})
        self.assertIn('VARS list of ignored switches was not found', p[0])


if __name__ == '__main__':
    unittest.main()
