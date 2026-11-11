"""Every lab script's names resolve (#1078: restart_parity.py used REAL_STORE and art_put_back
without importing them, a NameError only on hardware). Static: symtable finds each implicit
global a function reads; it must be defined at module level or be a builtin. No script runs."""
import builtins
import pathlib
import symtable
import unittest

LAB = pathlib.Path(__file__).resolve().parent / 'lab'


def module_names(table):
    return {s.get_name() for s in table.get_symbols() if s.is_assigned() or s.is_imported()
            or s.is_namespace()}


def unresolved(path):
    table = symtable.symtable(path.read_text(encoding='utf-8'), str(path), 'exec')
    known = module_names(table) | set(dir(builtins)) | {'__file__', '__name__', '__doc__'}
    missing = set()

    def walk(t):
        for child in t.get_children():
            if child.get_type() in ('function', 'class'):
                for s in child.get_symbols():
                    if s.is_global() and s.is_referenced() and s.get_name() not in known:
                        missing.add(s.get_name())
            walk(child)

    walk(table)
    # Module-level reads too.
    for s in table.get_symbols():
        if s.is_referenced() and not (s.is_assigned() or s.is_imported() or s.is_namespace()) \
                and s.get_name() not in known:
            missing.add(s.get_name())
    return missing


class LabScriptNames(unittest.TestCase):
    def test_every_lab_script_resolves_its_names(self):
        scripts = sorted(LAB.glob('*.py'))
        self.assertTrue(scripts)
        for path in scripts:
            with self.subTest(script=path.name):
                self.assertEqual(unresolved(path), set(), f'{path.name}: undefined names')

    def test_the_check_catches_a_missing_import(self):
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d) / 'x.py'
            p.write_text('from os import path\n\ndef f():\n    return REAL_STORE, path\n', encoding='utf-8')
            self.assertEqual(unresolved(p), {'REAL_STORE'})


if __name__ == '__main__':
    unittest.main()
