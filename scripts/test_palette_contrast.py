"""Every bundled palette holds its contrast promises, and the harmony palettes keep Bandit's
orange as their anchor (docs/palette.md, "Harmony palettes")."""

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import palette_contrast as pc  # noqa: E402


class PaletteContrast(unittest.TestCase):
    def test_every_palette_holds_its_contrast(self):
        files = sorted(pc.PALETTES.glob("*.json"))
        self.assertTrue(any(f.name == "bandit.json" for f in files))
        for f in files:
            for fg, g, ratio, minimum, ok in pc.check(pc.resolve(f)):
                self.assertTrue(ok, f"{f.name}: {fg} on {g} is {ratio:.2f}, needs {minimum}")

    def test_harmony_palettes_keep_the_bandit_anchor(self):
        bandit = json.loads((pc.PALETTES / "bandit.json").read_text(encoding="utf-8"))["roles"]
        for f in sorted(pc.PALETTES.glob("bandit-*.json")):
            data = json.loads(f.read_text(encoding="utf-8"))
            r = data["roles"]
            self.assertTrue(data["name"].startswith("Bandit "), f.name)
            self.assertNotEqual(data["name"], "Bandit", "saved looks name Bandit's own palette")
            for k in ("accent", "highlight", "accent_light"):
                self.assertEqual(r[k], bandit[k], f"{f.name}: {k}")
            self.assertIn("accent2", r, f.name)
            self.assertEqual(r["app_ground"], r["base"], f.name)
            self.assertIn(pc.BANDIT_ACCENT, data["stops"] + [r["accent"]], f.name)
            self.assertTrue(2 <= len(data["stops"]) <= 8, f.name)

    def test_readable_matches_the_rust_rule(self):
        # palette_file.rs: the derived `deep` and `accent_light` for Bandit's orange on paper.
        deep = pc.readable("#f08a3c", "#e9e6df", 3.0)
        self.assertGreaterEqual(pc.contrast(deep, "#e9e6df"), 3.0)
        self.assertEqual(pc.readable("#f08a3c", "#09090a", 3.0), "#f08a3c")


if __name__ == "__main__":
    unittest.main()
