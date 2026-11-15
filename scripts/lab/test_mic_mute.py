"""Tests for mic_mute's pure helpers (ink). Nothing here launches anything or touches hardware."""

import ast
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))


def _extract_ink():
    """Parse mic_mute.py and extract ink() without running module-level side effects (argparse)."""
    src = Path(__file__).with_name('mic_mute.py').read_text(encoding='utf-8')
    tree = ast.parse(src)
    ns = {}
    for node in ast.iter_child_nodes(tree):
        if isinstance(node, ast.FunctionDef) and node.name == 'ink':
            code = compile(ast.Module([node], []), '<mic_mute>', 'exec')
            exec(code, ns)  # noqa: S102
            break
    return ns['ink']


# ink() reads from a global `dump` (Path). We provide it via the globals dict.
INK = _extract_ink()


class InkTests(unittest.TestCase):
    """Tests for mic_mute.ink(): share of lit pixels in a PBM frame."""

    def _write_pbm(self, data: bytes) -> Path:
        p = tempfile.mktemp(suffix='.pbm')
        Path(p).write_bytes(data)
        self.addCleanup(Path(p).unlink, missing_ok=True)
        return Path(p)

    def test_black_image_has_no_ink(self):
        # P4 is a 1-bit bitmap; ink() skips only the first two header lines (magic, size).
        # 16x2 pixels = 2 bytes per row, 4 data bytes, all zero, so no ink.
        raw = b'P4\n16 2\n\x00\x00\x00\x00'
        INK.__globals__['dump'] = self._write_pbm(raw)
        self.assertEqual(INK(), 0.0)

    def test_white_image_is_full_ink(self):
        # P4 is a 1-bit bitmap: 8x2 pixels = 1 byte per row, so 2 data bytes, every bit set = 100% ink
        raw = b'P4\n8 2\n\xff\xff'
        INK.__globals__['dump'] = self._write_pbm(raw)
        self.assertEqual(INK(), 1.0)

    def test_half_white_half_black(self):
        # P4 is a 1-bit bitmap, 8x1; the byte 0xf0 is 11110000, so half the pixels are lit
        raw = b'P4\n8 1\n\xf0'
        INK.__globals__['dump'] = self._write_pbm(raw)
        self.assertEqual(INK(), 0.5)


if __name__ == '__main__':
    unittest.main()
