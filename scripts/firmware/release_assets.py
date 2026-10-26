"""The firmware release's assets (#378), from one CI run's UF2s.

The release ships the GA halves and the settings-reset UF2 under the release's own names, with a
SHA256SUMS over them:

    foxglove-<version>-left.uf2            the left half, GA
    foxglove-<version>-right.uf2           the right half, GA
    foxglove-<version>-settings-reset.uf2  RESCUE.md's last resort: erases the half's settings
                                           and Bluetooth pairings, then the half is flashed again
    SHA256SUMS                             sha256sum format: "<hex>  <name>"

    python scripts/firmware/release_assets.py --version 0.2.0 --src out --dest release

`--src` holds the CI's named UF2s (foxglove-fw-v<version>-<sha>-left-ga.uf2, ...-right-ga.uf2,
...-settings-reset.uf2). Exit status 1 if one is missing or there's more than one of a kind.
Standard library only.
"""
import argparse
import hashlib
import shutil
import sys
from pathlib import Path

# The CI's name suffix for each release asset.
KINDS = {'left': '-left-ga.uf2', 'right': '-right-ga.uf2', 'settings-reset': '-settings-reset.uf2'}


def assets(version, src, dest):
    """Copies the release's UF2s from `src` into `dest` and writes SHA256SUMS. Returns the
    names written, SHA256SUMS last; raises ValueError naming what's missing or doubled."""
    src, dest = Path(src), Path(dest)
    problems, picked = [], {}
    for kind, suffix in KINDS.items():
        found = sorted(p for p in src.glob('*.uf2') if p.name.endswith(suffix))
        if len(found) != 1:
            problems.append(f'{kind}: {len(found)} files ending {suffix} in {src}')
        else:
            picked[kind] = found[0]
    if problems:
        raise ValueError('; '.join(problems))
    dest.mkdir(parents=True, exist_ok=True)
    names, sums = [], []
    for kind, path in picked.items():
        name = f'foxglove-{version}-{kind}.uf2'
        shutil.copyfile(path, dest / name)
        sums.append(f'{hashlib.sha256((dest / name).read_bytes()).hexdigest()}  {name}\n')
        names.append(name)
    (dest / 'SHA256SUMS').write_text(''.join(sums), encoding='ascii', newline='\n')
    return names + ['SHA256SUMS']


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--version', required=True)
    ap.add_argument('--src', required=True)
    ap.add_argument('--dest', required=True)
    args = ap.parse_args(argv)
    try:
        for name in assets(args.version, args.src, args.dest):
            print(name)
    except ValueError as e:
        print(f'release assets: {e}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
