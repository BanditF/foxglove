"""GA builds must carry no test hook (#81); dev builds must carry the expected ones.

Every test hook compiled into the firmware leaves a marker string, FOXGLOVE_TEST_HOOK=<name>, in a
kept section (FOXGLOVE_TEST_HOOK_MARK in half_marker.h). This reads the UF2 payloads (markers can
cross block boundaries, so not the raw file) and checks them.

    python scripts/firmware/check_hooks.py --ga out/*-ga.uf2 \\
        --dev out/*-left-dev.uf2=position,bootloader,guard_test --dev out/*-right-dev.uf2=position,bootloader

Exit status 1 with a message per problem. Standard library only.
"""
import argparse
import re
import struct
import sys

MAGIC = (0x0A324655, 0x9E5D5157)
MARKER = re.compile(rb'FOXGLOVE_TEST_HOOK=([a-z_]+)')


def payload(data):
    """The UF2 blocks' payload bytes, in file order."""
    out = bytearray()
    for i in range(0, len(data) - 511, 512):
        block = data[i:i + 512]
        m0, m1, _flags, _addr, size = struct.unpack_from('<5I', block)
        if (m0, m1) != MAGIC:
            raise ValueError(f'block {i // 512}: not a UF2 block')
        out += block[32:32 + size]
    return bytes(out)


def hooks(data):
    return {m.decode() for m in MARKER.findall(payload(data))}


def check(ga, dev):
    """ga: [path]; dev: [(path, {expected names})]. Returns a list of problems."""
    problems = []
    for path in ga:
        found = hooks(open(path, 'rb').read())
        if found:
            problems.append(f'{path}: GA build carries test hooks: {", ".join(sorted(found))}')
    for path, expected in dev:
        found = hooks(open(path, 'rb').read())
        missing = expected - found
        if missing:
            problems.append(f'{path}: dev build lacks test hooks: {", ".join(sorted(missing))}')
    return problems


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    p.add_argument('--ga', nargs='*', default=[], help='UF2s that must carry no test hook')
    p.add_argument('--dev', action='append', default=[],
                   help='PATH=name,name: a dev UF2 and the hooks it must carry')
    args = p.parse_args(argv)
    dev = []
    for item in args.dev:
        path, _, names = item.partition('=')
        dev.append((path, {n for n in names.split(',') if n}))
    problems = check(args.ga, dev)
    for problem in problems:
        print(problem, file=sys.stderr)
    if not problems:
        print(f'test hooks ok: {len(args.ga)} GA build(s) clean, {len(dev)} dev build(s) complete')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
