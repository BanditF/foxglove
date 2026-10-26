"""This PC's values for the dev tooling, in one place (platform-fit audit, 2026-10-26).

The scripts used to bake in this PC's keyboard serials and paths. They now ask here. Each
value comes from the first of these that has it:
1. an environment variable (`FOXGLOVE_LEFT_SERIAL`, ...: see ENV below);
2. `tools/local.toml`, looked for in `FOXGLOVE_LOCAL_TOML`, then the checkout's own `tools/`,
   then the workspace's `tools/` (the folder holding the main checkout, e.g.
   `C:\\Users\\fuchs\\splitkeeb\\tools\\local.toml`);
3. the defaults below, which are this PC's values, so nothing changes here without the file.

`tools/local.example.toml` lists every key. From a shell:
    python scripts/local_config.py get worktrees.root
    python scripts/local_config.py show
"""
import os
from pathlib import Path
import subprocess
import sys
import tomllib

# This PC's values: the defaults when nothing else says otherwise.
DEFAULTS = {
    'keyboard': {
        'left_serial': '1E89EF085AA8A635',
        'right_serial': '35B4541F78DE9CF8',
    },
    'flash': {
        'rollback': r'C:\Users\fuchs\splitkeeb\firmware-358c3d0\firmware',
    },
    'workspace': {
        'root': r'C:\Users\fuchs\splitkeeb',
    },
    'worktrees': {
        'root': r'C:\Users\fuchs\splitkeeb\worktrees',
    },
}

# Environment variables that override a key.
ENV = {
    'FOXGLOVE_LEFT_SERIAL': ('keyboard', 'left_serial'),
    'FOXGLOVE_RIGHT_SERIAL': ('keyboard', 'right_serial'),
    'FOXGLOVE_ROLLBACK': ('flash', 'rollback'),
    'FOXGLOVE_WORKSPACE': ('workspace', 'root'),
    'FOXGLOVE_WORKTREES': ('worktrees', 'root'),
}

REPO = Path(__file__).resolve().parent.parent


def workspace_of(repo=REPO):
    """The folder holding the main checkout (git's common dir's parent's parent), or None."""
    try:
        out = subprocess.run(
            ['git', '-C', str(repo), 'rev-parse', '--path-format=absolute', '--git-common-dir'],
            capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0 or not out.stdout.strip():
        return None
    return Path(out.stdout.strip()).parent.parent


def candidates(env=os.environ, repo=REPO, workspace=None):
    """Where tools/local.toml may be, in order. Lazy: git is asked for the workspace only when
    the earlier places have no file."""
    if env.get('FOXGLOVE_LOCAL_TOML'):
        yield Path(env['FOXGLOVE_LOCAL_TOML'])
    yield repo / 'tools' / 'local.toml'
    ws = workspace if workspace is not None else workspace_of(repo)
    if ws:
        yield Path(ws) / 'tools' / 'local.toml'


def merge(base, over):
    out = {k: dict(v) for k, v in base.items()}
    for section, values in over.items():
        if isinstance(values, dict):
            out.setdefault(section, {}).update(values)
    return out


def load(env=os.environ, files=None):
    """The values: defaults, then the first tools/local.toml found, then the environment."""
    config = merge(DEFAULTS, {})
    for path in (files if files is not None else candidates(env)):
        if path.is_file():
            with open(path, 'rb') as f:
                config = merge(config, tomllib.load(f))
            break
    for var, (section, key) in ENV.items():
        if env.get(var):
            config.setdefault(section, {})[key] = env[var]
    return config


def get(dotted, env=os.environ, files=None):
    section, key = dotted.split('.', 1)
    return load(env, files)[section][key]


def serials(env=os.environ, files=None):
    """{serial: half}, as the flasher wants them. Refuses serials that aren't plain letters
    and digits, or the same for both halves (as foxglove-lab ignores them)."""
    k = load(env, files)['keyboard']
    left, right = str(k['left_serial']), str(k['right_serial'])
    for name, value in (('left_serial', left), ('right_serial', right)):
        if not value or not value.isascii() or not value.isalnum():
            raise ValueError(f'keyboard.{name} must be letters and digits, not {value!r}')
    if left.upper() == right.upper():
        raise ValueError('keyboard.left_serial and right_serial are the same')
    return {left: 'left', right: 'right'}


def main(argv):
    if len(argv) == 3 and argv[1] == 'get':
        print(get(argv[2]))
        return 0
    if len(argv) == 2 and argv[1] == 'show':
        for section, values in load().items():
            for key, value in values.items():
                print(f'{section}.{key} = {value}')
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv))
