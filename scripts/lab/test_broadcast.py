"""Pure helper functions from broadcast and its labkit dependencies, tested without GUI/network/keyboard.

- secret(): the environment variables and subprocess args it constructs for plugin_secret.ps1;
- same(): comparing two RGB_STATE readings (used for "lights as before" checks);
- hue_is(): the hue predicate with a tolerance (used throughout broadcast's state checks).
"""

import ast
import sys
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import labkit  # noqa: E402


def _load_secret():
    """Extract only the secret() function from broadcast.py via AST (avoids all module-level side effects)."""
    src = Path(__file__).resolve().parent / 'broadcast.py'
    tree = ast.parse(src.read_text(encoding='utf-8'))
    for node in ast.iter_child_nodes(tree):
        if isinstance(node, ast.FunctionDef) and node.name == 'secret':
            code = compile(ast.Module(body=[node], type_ignores=[]), '<broadcast>', 'exec')
            ns = {'subprocess': MagicMock(), 'os': __import__('os'),
                  'HERE': Path(__file__).resolve().parent,
                  'PLUGIN': 'org.foxglove.broadcast-status'}
            exec(code, ns)
            return ns['secret'], ns['subprocess']


class Secret(unittest.TestCase):
    def test_sets_the_foxglove_lab_secret_in_env(self):
        secret, mock_subproc = _load_secret()
        secret('get', 'my-pass')  # mock never raises
        self.assertEqual(mock_subproc.run.call_args_list[0].kwargs['env']['FOXGLOVE_LAB_SECRET'], 'my-pass')

    def test_clears_secret_when_value_is_none(self):
        secret, mock_subproc = _load_secret()
        secret('delete')  # value=None → ''
        self.assertEqual(mock_subproc.run.call_args_list[0].kwargs['env']['FOXGLOVE_LAB_SECRET'], '')

class Same(unittest.TestCase):
    """labkit.same(): two halves' RGB_STATE must match within on/hue/sat/effect."""

    def test_equal_halves_return_true(self):
        a = [{'on': True, 'hue': 0, 'sat': 100, 'brt': 80, 'effect': 0},
             {'on': True, 'hue': 0, 'sat': 100, 'brt': 80, 'effect': 0}]
        b = [{'on': True, 'hue': 0, 'sat': 100, 'brt': 80, 'effect': 0},
             {'on': True, 'hue': 0, 'sat': 100, 'brt': 80, 'effect': 0}]
        self.assertTrue(labkit.same(a, b))

    def test_different_hue_returns_false(self):
        a = [{'on': True, 'hue': 0, 'sat': 100, 'brt': 80, 'effect': 0},
             {'on': True, 'hue': 0, 'sat': 100, 'brt': 80, 'effect': 0}]
        b = [{'on': True, 'hue': 120, 'sat': 100, 'brt': 80, 'effect': 0},
             {'on': True, 'hue': 120, 'sat': 100, 'brt': 80, 'effect': 0}]
        self.assertFalse(labkit.same(a, b))


class HueIs(unittest.TestCase):
    """labkit.hue_is(): hue predicate with a tolerance."""

    def test_exact_hue_within_tolerance(self):
        pred = labkit.hue_is(120)  # green
        state = [{'on': True, 'hue': 120, 'sat': 100, 'brt': 80, 'effect': 0},
                 {'on': True, 'hue': 120, 'sat': 100, 'brt': 80, 'effect': 0}]
        self.assertTrue(pred(state))

    def test_off_led_is_false(self):
        pred = labkit.hue_is(0)
        state = [{'on': False, 'hue': 0, 'sat': 100, 'brt': 80, 'effect': 0},
                 {'on': True, 'hue': 0, 'sat': 100, 'brt': 80, 'effect': 0}]
        self.assertFalse(pred(state))


if __name__ == '__main__':
    unittest.main()