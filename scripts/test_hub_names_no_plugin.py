"""The Hideout hub names no plugin (H2.8, #975): crates/hideout-* draw every module's tile and
page from the contract alone. Each plugin's ids, crate names and product words are read from the
plugin crates themselves, so a new plugin is covered without editing this test."""

import pathlib
import re
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def plugin_words():
    words = set()
    for crate in sorted((ROOT / "crates").glob("plugin-*")):
        if crate.name in ("plugin-sdk", "plugin-api", "plugin-host", "plugin-trust"):
            continue
        words.add(crate.name)
        words.add(crate.name.removeprefix("plugin-"))
        manifest = crate / "plugin.toml"
        if manifest.exists():
            data = tomllib.loads(manifest.read_text(encoding="utf-8"))
            words.add(data["id"])
            words.add(data["id"].rsplit(".", 1)[-1])
    # Plain words like "focus" or "sandbox-probe" parts are too common to ban; ids and crate
    # names are what a hub that knew a plugin would contain.
    return sorted(w.lower() for w in words if len(w) >= 4)


class HubNamesNoPlugin(unittest.TestCase):
    def test_hub_crates_name_no_plugin(self):
        words = plugin_words()
        self.assertIn("mic-mute", words)
        pattern = re.compile("|".join(re.escape(w) for w in words), re.IGNORECASE)
        hits = []
        for crate in sorted((ROOT / "crates").glob("hideout*")):
            for path in crate.rglob("*"):
                if path.is_file() and path.suffix in (".rs", ".toml", ".md", ".json"):
                    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                        m = pattern.search(line)
                        if m:
                            hits.append(f"{path.relative_to(ROOT)}:{n}: {m.group(0)}")
        self.assertEqual(hits, [], "hub code names a plugin:\n" + "\n".join(hits))


if __name__ == "__main__":
    unittest.main()
