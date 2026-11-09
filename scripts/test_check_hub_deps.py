"""check_hub_deps.py (#932) against fake `cargo metadata`, no cargo run."""
import json, tempfile, unittest
from pathlib import Path

import check_hub_deps as hd

WS = 'C:/ws'


def pkg(name, lic='MIT', source='registry+https://github.com/rust-lang/crates.io-index',
        manifest=None, deps=()):
    return {'id': f'{name}-id', 'name': name, 'version': '1.0.0', 'license': lic,
            'license_file': None, 'source': source,
            'manifest_path': manifest or f'C:/registry/{name}/Cargo.toml',
            'dependencies': [dict(d) for d in deps]}


def local(name, lic='MIT', folder='crates', deps=()):
    return pkg(name, lic, source=None, manifest=f'{WS}/{folder}/{name}/Cargo.toml', deps=deps)


def meta(packages, edges):
    """edges: {name: [(dep name, kind)]}; kind None is a normal dependency."""
    nodes = [{'id': f'{n}-id', 'deps': [{'pkg': f'{d}-id', 'dep_kinds': [{'kind': k}]} for d, k in es]}
             for n, es in edges.items()]
    return {'packages': packages, 'resolve': {'nodes': nodes}, 'workspace_root': WS}


def clean_graph(**extra):
    pkgs = [local('hideout-shell'), local('hideout'), pkg('eframe', 'MIT OR Apache-2.0'),
            pkg('unicode-ident', '(MIT OR Apache-2.0) AND Unicode-3.0')]
    edges = {'hideout': [('hideout-shell', None)], 'hideout-shell': [('eframe', None)],
             'eframe': [('unicode-ident', None)]}
    for name, (p, parent, kind) in extra.items():
        pkgs.append(p)
        edges.setdefault(parent, []).append((name, kind))
    return meta(pkgs, edges)


class LicenceExpressions(unittest.TestCase):
    def test_permissive_forms_pass(self):
        for expr in ('MIT', 'Apache-2.0', 'MIT/Apache-2.0', 'MIT OR Apache-2.0',
                     '(MIT OR Apache-2.0) AND Unicode-3.0', 'Apache-2.0 WITH LLVM-exception',
                     'Zlib OR Apache-2.0 OR MIT', 'BSD-3-Clause'):
            self.assertTrue(hd.licence_ok(expr), expr)

    def test_copyleft_fails(self):
        for expr in ('GPL-3.0-only', 'MPL-2.0', 'LGPL-2.1-or-later', 'MIT AND GPL-3.0',
                     '(MIT OR Apache-2.0) AND OFL-1.1', 'AGPL-3.0'):
            self.assertFalse(hd.licence_ok(expr), expr)

    def test_or_with_copyleft_passes_on_the_permissive_side(self):
        self.assertTrue(hd.licence_ok('GPL-3.0 OR MIT'))

    def test_malformed_is_an_error(self):
        for expr in ('(MIT OR Apache-2.0', 'MIT OR', 'MIT Apache-2.0'):
            with self.assertRaises(ValueError, msg=expr):
                hd.licence_ok(expr)
        self.assertFalse(hd.licence_ok(''))


class Graph(unittest.TestCase):
    def test_clean_graph_passes(self):
        self.assertEqual(hd.problems(clean_graph(), Path(WS) / 'app'), [])

    def test_copyleft_deep_in_the_graph_fails(self):
        m = clean_graph(**{'option-ext': (pkg('option-ext', 'MPL-2.0'), 'unicode-ident', None)})
        found = hd.problems(m, Path(WS) / 'app')
        self.assertEqual(len(found), 1)
        self.assertIn('option-ext', found[0])
        self.assertIn('MPL-2.0', found[0])

    def test_missing_licence_field_fails(self):
        p = pkg('odd', lic=None)
        p['license_file'] = 'LICENSE.txt'
        found = hd.problems(clean_graph(odd=(p, 'eframe', None)), Path(WS) / 'app')
        self.assertTrue(any('odd' in f and 'no licence field' in f for f in found), found)

    def test_path_dependency_under_app_fails(self):
        gpl = local('keypeek-bits', lic='GPL-3.0-only', folder='app')
        found = hd.problems(clean_graph(**{'keypeek-bits': (gpl, 'hideout-shell', None)}), Path(WS) / 'app')
        self.assertTrue(any('under app/' in f for f in found), found)
        self.assertTrue(any('must be MIT' in f for f in found), found)

    def test_declared_path_under_app_fails_even_if_unresolved(self):
        m = clean_graph()
        shell = next(p for p in m['packages'] if p['name'] == 'hideout-shell')
        shell['dependencies'].append({'name': 'shared', 'path': f'{WS}/APP/src/shared'})
        found = hd.problems(m, Path(WS) / 'app')
        self.assertTrue(any('depends on shared by a path under app/' in f for f in found), found)

    def test_own_crate_must_be_mit(self):
        m = clean_graph()
        next(p for p in m['packages'] if p['name'] == 'hideout')['license'] = 'MIT OR Apache-2.0'
        found = hd.problems(m, Path(WS) / 'app')
        self.assertTrue(any(f.startswith('hideout 1.0.0: our own crate') for f in found), found)

    def test_dev_dependencies_count_only_at_the_roots(self):
        bad_root_dev = clean_graph(**{'gpl-test': (pkg('gpl-test', 'GPL-3.0'), 'hideout-shell', 'dev')})
        self.assertEqual(len(hd.problems(bad_root_dev, Path(WS) / 'app')), 1)
        deep_dev = clean_graph(**{'gpl-test': (pkg('gpl-test', 'GPL-3.0'), 'eframe', 'dev')})
        self.assertEqual(hd.problems(deep_dev, Path(WS) / 'app'), [])

    def test_unreachable_packages_are_ignored(self):
        m = clean_graph()
        m['packages'].append(local('foxglove-app', lic='GPL-3.0-only', folder='app'))
        self.assertEqual(hd.problems(m, Path(WS) / 'app'), [])

    def test_missing_root_is_an_error(self):
        m = clean_graph()
        m['packages'] = [p for p in m['packages'] if p['name'] != 'hideout']
        with self.assertRaises(ValueError):
            hd.problems(m, Path(WS) / 'app')

    def test_main_exit_codes(self):
        with tempfile.TemporaryDirectory() as d:
            good, bad = Path(d) / 'good.json', Path(d) / 'bad.json'
            good.write_text(json.dumps(clean_graph()), encoding='utf-8')
            bad.write_text(json.dumps(clean_graph(**{'x': (pkg('x', 'GPL-2.0'), 'eframe', None)})), encoding='utf-8')
            self.assertEqual(hd.main(['--metadata', str(good)]), 0)
            self.assertEqual(hd.main(['--metadata', str(bad)]), 1)


if __name__ == '__main__':
    unittest.main()
