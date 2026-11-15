"""Pure helpers in api_tools: log_lines (filtering a log file) and user_pc_lighting (TOML round-trip).

No GUI, network, keyboard or files outside a temp dir are needed.

    python -m unittest test_api_tools -v   # from scripts/lab/
"""
import ast
import tempfile
import tomllib
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent

# Extract only the pure helpers from api_tools.py via AST: the module itself runs argparse and
# drives the app at import time, so it cannot simply be imported.
_WANTED = {"user_pc_lighting", "log_lines"}
_tree = ast.parse((LAB / "api_tools.py").read_text(encoding="utf-8"))
_funcs = [n for n in _tree.body if isinstance(n, ast.FunctionDef) and n.name in _WANTED]
assert {f.name for f in _funcs} == _WANTED
_namespace = {"tomllib": tomllib, "Path": Path}
exec(compile(ast.Module(body=_funcs, type_ignores=[]), "api_tools.py", "exec"), _namespace)


class LogLines(unittest.TestCase):

    def test_matches_are_returned(self):
        with tempfile.TemporaryDirectory() as d:
            log = Path(d) / "logs" / "foxglove.log"
            log.parent.mkdir(parents=True, exist_ok=True)
            log.write_text("hello zone colours of green\nnope\nbye zone colours of blue\n", encoding="utf-8")
            result = _namespace["log_lines"](Path(d), "zone colours of")
            self.assertEqual(result, ["hello zone colours of green", "bye zone colours of blue"])

    def test_no_match_returns_empty_list(self):
        with tempfile.TemporaryDirectory() as d:
            log = Path(d) / "logs" / "foxglove.log"
            log.parent.mkdir(parents=True, exist_ok=True)
            log.write_text("nothing here\n", encoding="utf-8")
            result = _namespace["log_lines"](Path(d), "missing piece")
            self.assertEqual(result, [])

    def test_missing_log_returns_empty_list(self):
        with tempfile.TemporaryDirectory() as d:
            result = _namespace["log_lines"](Path(d), "anything")
            self.assertEqual(result, [])


class UserPcLighting(unittest.TestCase):

    def setUp(self):
        # Point REAL_STORE at our own temp dir so we control the TOML content.
        self._tmpdir = tempfile.TemporaryDirectory()
        _namespace["REAL_STORE"] = Path(self._tmpdir.name) / "fake.toml"

    def tearDown(self):
        self._tmpdir.cleanup()

    def test_one_device_with_colors_and_match_keyboard(self):
        toml_in = '[pc_lighting.devices."keyboard"]\nmatch_keyboard = true\n[pc_lighting.devices."keyboard".colors]\n"1" = "#ff0000"\n'
        (_namespace["REAL_STORE"]).write_text(toml_in, encoding="utf-8")
        devices, out = _namespace["user_pc_lighting"]()
        parsed_out = tomllib.loads(out)["pc_lighting"]["devices"]
        expected = tomllib.loads(toml_in)["pc_lighting"]["devices"]
        self.assertEqual(parsed_out, expected)



if __name__ == "__main__":
    unittest.main()
