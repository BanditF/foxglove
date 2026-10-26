"""Size budget (code governor, option A): functions over 120 lines and files over 1,500 lines in
Foxglove's Rust (crates/ and app/src/foxglove/; KeyPeek's upstream files are left out).

It warns and lists the offenders; it never fails the gate, so the current ones don't block
anything. New code should stay under the budget; split a function that grows past it.

    python scripts/size_budget.py [--fn-lines 120] [--file-lines 1500]

Function length is a brace count from the `fn` line to its closing brace, after stripping
comments, strings and char literals, so it's an estimate, not a parser. Standard library only.
"""
import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TREES = ['crates', 'app/src/foxglove']
FN = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+(\w+)')
# Strings (plain and raw), char literals and comments; lifetimes ('a) are left alone.
NOISE = re.compile(r'r(#*)".*?"\1|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'|//.*$')


def strip(line, in_block):
    """`line` without comments and literals, and whether a /* */ comment is still open."""
    out = ''
    while line:
        if in_block:
            end = line.find('*/')
            if end < 0:
                return out, True
            line, in_block = line[end + 2:], False
            continue
        start = line.find('/*')
        head, line = (line, '') if start < 0 else (line[:start], line[start + 2:])
        out += NOISE.sub('""', head)
        in_block = start >= 0
    return out, in_block


def functions(text):
    """(name, first line, length in lines) for every fn with a body."""
    found, stack, depth, in_block = [], [], 0, False
    pending = None  # a fn signature waiting for its opening brace
    for number, raw in enumerate(text.splitlines(), 1):
        line, in_block = strip(raw, in_block)
        m = FN.match(line)
        if m:
            pending = (m.group(1), number)
        for ch in line:
            if ch == '{':
                depth += 1
                if pending:
                    stack.append((pending[0], pending[1], depth))
                    pending = None
            elif ch == '}':
                if stack and stack[-1][2] == depth:
                    name, first, _ = stack.pop()
                    found.append((name, first, number - first + 1))
                depth -= 1
            elif ch == ';' and pending:
                pending = None  # a declaration without a body (trait method, extern)
    return found


def scan(root=ROOT, fn_lines=120, file_lines=1500):
    """The files and functions over budget: ([(path, lines)], [(path, line, name, lines)])."""
    files, fns = [], []
    for tree in TREES:
        for path in sorted((root / tree).rglob('*.rs')):
            if 'target' in path.relative_to(root).parts:
                continue
            text = path.read_text(encoding='utf-8', errors='replace')
            rel = path.relative_to(root).as_posix()
            n = text.count('\n') + 1
            if n > file_lines:
                files.append((rel, n))
            fns += [(rel, first, name, length) for name, first, length in functions(text) if length > fn_lines]
    return files, fns


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--fn-lines', type=int, default=120)
    ap.add_argument('--file-lines', type=int, default=1500)
    args = ap.parse_args(argv)
    files, fns = scan(fn_lines=args.fn_lines, file_lines=args.file_lines)
    for rel, n in sorted(files, key=lambda f: -f[1]):
        print(f'warning: {rel}: {n} lines (budget {args.file_lines})')
    for rel, first, name, length in sorted(fns, key=lambda f: -f[3]):
        print(f'warning: {rel}:{first}: fn {name} is {length} lines (budget {args.fn_lines})')
    print(f'size budget: {len(files)} files over {args.file_lines} lines, '
          f'{len(fns)} functions over {args.fn_lines} lines (warnings only)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
