from pathlib import Path
import unittest
from keymap import keymap_lint as k

REAL = Path(__file__).resolve().parents[2] / 'config' / 'eyelash_corne.keymap'


class KeymapTests(unittest.TestCase):
    def setUp(self):
        self.source = REAL.read_text()
        self.km = k.parse(self.source)

    def test_binding_counts(self):
        self.assertEqual(k.check_binding_counts(self.km), [])
        self.km.layers[0][1].pop()
        self.assertTrue(k.check_binding_counts(self.km))

    def test_layer_references(self):
        self.assertEqual(k.check_layer_references(self.km), [])
        for behavior in ('mo', 'tog', 'to', 'lt', 'sl'):
            km = k.parse(self.source + f'\n&{behavior} 99 A;')
            self.assertTrue(k.check_layer_references(km))
        self.km.conditions.append(((99,), 0))
        self.assertTrue(k.check_layer_references(self.km))
        self.km.conditions = [((1, 2), 99)]
        self.assertTrue(k.check_layer_references(self.km))

    def test_thumb_replay(self):
        self.assertEqual(k.check_thumb_replay(self.km), [])
        # Left-first GAME must keep the right thumb on SYMBOL through GAME+.
        self.km.layers[4][1][46] = '&mo 1'
        self.assertTrue(k.check_thumb_replay(self.km))

    def test_soft_off(self):
        self.assertEqual(k.check_soft_off(self.km), [])
        self.km.layers[0][1][1] = '&soft_off'
        self.assertTrue(k.check_soft_off(self.km))

    def test_soft_off_cannot_be_toggled_directly_from_base(self):
        # A hold path alone is insufficient if a second path bypasses it.
        self.km.layers[0][1][0] = '&tog 1'
        self.km.layers[1][1][0] = '&soft_off'
        self.assertTrue(k.check_soft_off(self.km))

    def test_vendor_combo_and_comments(self):
        combo = 'bad { key-positions = <1 15 29>; bindings = <&soft_off>; };'
        modified = self.source.replace('compatible = "zmk,combos";', 'compatible = "zmk,combos";' + combo)
        self.assertTrue(any('Q+S+Z' in s for s in k.lint(modified)))
        self.assertEqual(k.lint(self.source + '\n/* &mo 999 */ // &to 999\n'), [])

    def test_real_keymap(self):
        self.assertEqual(k.lint(self.source), [])


if __name__ == '__main__':
    unittest.main()
