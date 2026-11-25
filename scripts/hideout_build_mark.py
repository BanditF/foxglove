"""The installed Hideout's build marker (#1466): `hideout.exe` carries one line saying whether it
was built with the `lab-switches` feature (crates/hideout-registry/src/lab.rs, BUILD_MARK). The
installer build runs this on the exe it is about to pack and stops unless the exe is the build
without lab switches. It only reads the file: it never runs it.

    python scripts/hideout_build_mark.py <path to hideout.exe>

Exit 0 when the exe carries the `off` line and not the `on` line; 1 with the reason otherwise.
"""
import sys
from pathlib import Path

OFF = b'hideout-build: lab-switches=off'
ON = b'hideout-build: lab-switches=on'


def problem(data):
    """Why `data` (the exe's bytes) is not the installed build, or None."""
    if ON in data:
        return 'it was built with lab-switches on (build with --no-default-features)'
    if OFF not in data:
        return 'it carries no build marker (not a hideout.exe from this source, or too old)'
    return None


def main(argv):
    if len(argv) != 2:
        print(__doc__)
        return 1
    try:
        data = Path(argv[1]).read_bytes()
    except OSError as e:
        print(f'{argv[1]}: cannot be read: {e}')
        return 1
    why = problem(data)
    if why:
        print(f'{argv[1]}: {why}')
        return 1
    print(f'{argv[1]}: built without lab switches')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
