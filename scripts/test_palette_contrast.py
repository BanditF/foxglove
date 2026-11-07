"""Every bundled palette holds its contrast promises, and the harmony palettes keep Bandit's
orange as their anchor (docs/palette.md, "Harmony palettes" and "Bold tier")."""

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

    def test_bold_tier_keeps_its_promises(self):
        bold = [f for f in sorted(pc.PALETTES.glob("*.json")) if pc.resolve(f)["tier"] == "bold"]
        self.assertTrue(bold, "the bold tier has palettes")
        names = {json.loads(f.read_text(encoding="utf-8"))["name"] for f in pc.PALETTES.glob("*.json")}
        for f in bold:
            p = pc.resolve(f)
            self.assertEqual(pc.tier_problems(p), [], f.name)
            self.assertIn("accent2", p["roles"], f.name)
            # A "Bold" variant sits next to the palette it pushes further.
            if p["name"].endswith(" Bold"):
                self.assertIn(p["name"][: -len(" Bold")], names, f.name)
                self.assertTrue(f.name.endswith("-bold.json"), f.name)

    def test_unknown_tier_is_refused(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / "x.json"
            f.write_text(json.dumps({"format": "foxglove-palette", "version": 1, "name": "X",
                                     "tier": "loud", "roles": {"accent": "#f08a3c"},
                                     "stops": ["#f08a3c", "#000000"]}), encoding="utf-8")
            with self.assertRaises(ValueError):
                pc.resolve(f)

    def test_wash_is_an_srgb_mix(self):
        self.assertEqual(pc.mix("#000000", "#ffffff", 0.5), "#808080")
        self.assertEqual(pc.mix("#06172a", "#4c9ee0", 0.0), "#06172a")

    def test_readable_matches_the_rust_rule(self):
        # palette_file.rs: the derived `deep` and `accent_light` for Bandit's orange on paper.
        deep = pc.readable("#f08a3c", "#e9e6df", 3.0)
        self.assertGreaterEqual(pc.contrast(deep, "#e9e6df"), 3.0)
        self.assertEqual(pc.readable("#f08a3c", "#09090a", 3.0), "#f08a3c")


if __name__ == "__main__":
    unittest.main()
