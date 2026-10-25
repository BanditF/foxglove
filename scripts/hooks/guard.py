"""PreToolUse hook: turns the roster's house rules into checks before a shell command runs.

Claude Code runs it before every Bash or PowerShell tool call (.claude/settings.json) with the
call as JSON on stdin. Exit 2 blocks the call and shows the reason to the agent; anything else
lets it run. It guards against accidents, not against a determined agent: each rule names the
one deliberate way past it. Any error inside the guard lets the command run (fail open).

Rules:
  flash      only 7 · On-call runs a flasher (dry runs are fine for everyone).
  app        no foxglove.exe launch on the user's desktop unless On-call said so.
  port       no second listener on a port that already has one.
  base       a PR targets the branch its name says (release/0.x for backports, else main),
             and nobody pushes straight to main or release/*.
  worktree   git worktrees go in one folder beside the main checkout, never inside the repo.
  cargo      heavy cargo commands run inside a build slot (#347): check.ps1 or build-slot.ps1.

    python scripts/hooks/guard.py < call.json
"""
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys

# The marker On-call puts in front of a flash or app launch (its role file says how).
ONCALL = re.compile(r'FOXGLOVE_ROLE\s*=\s*["\']?oncall\b', re.I)
APP_OK = re.compile(r'FOXGLOVE_APP_OK\s*=\s*["\']?1\b')
BASE_OK = re.compile(r'FOXGLOVE_ALLOW_BASE\s*=\s*["\']?1\b')
LISTEN_OK = re.compile(r'FOXGLOVE_ALLOW_PORT\s*=\s*["\']?1\b')

# Command separators, so a rule can look at each command in a chain on its own.
SEPARATORS = re.compile(r'\s*(?:&&|\|\||[;|\n])\s*')


def commands(line):
    return [c for c in SEPARATORS.split(line) if c.strip()]


def is_oncall(call, line):
    return call.get('agent_type') == 'on-call' or bool(ONCALL.search(line))


def check_flash(call, line):
    for cmd in commands(line):
        flasher = re.search(r'foxglove_flash\.py', cmd) and '--dry-run' not in cmd and not re.search(r'(^|\s)(-h|--help)\b', cmd)
        if flasher or re.search(r'flash-half\.ps1', cmd):
            if not is_oncall(call, line):
                return ('Only 7 · On-call flashes the keyboard (AGENTS-ROSTER.md). '
                        'Send the flash request to On-call through the Hub; --dry-run is fine here.')
    return None


APP_LAUNCH = [
    re.compile(r'Start-Process\b[^;|&\n]*\bfoxglove(\.exe)?\b(?![-_.]\w)', re.I),
    # foxglove.exe at the head of a command, after & or a call operator.
    re.compile(r'^\s*(&\s*)?["\']?[^\s"\']*\bfoxglove\.exe\b', re.I),
    re.compile(r'--app[\s=]["\']?[^\s"\']*foxglove\.exe', re.I),
    re.compile(r'\bcargo\s+run\b[^;|&\n]*(--manifest-path\s+["\']?app[\\/]|(-p|--package|--bin)\s+foxglove(\s|$))', re.I),
]


def check_app(call, line):
    if is_oncall(call, line) or APP_OK.search(line):
        return None
    for cmd in commands(line):
        if any(p.search(cmd) for p in APP_LAUNCH):
            return ('No Foxglove launches on the user\'s desktop unless On-call says so. Ask On-call '
                    'through the Hub; once it agrees, prefix the command with FOXGLOVE_APP_OK=1.')
    return None


LISTENERS = [
    re.compile(r'http\.server\s+(?:--\S+\s+\S+\s+)*(\d{2,5})'),
    re.compile(r'(?:--port|-Port|--listen)[\s=](?:[\d.]+:)?(\d{2,5})\b', re.I),
]


def port_busy(port):
    with socket.socket() as s:
        s.settimeout(0.3)
        return s.connect_ex(('127.0.0.1', port)) == 0


def check_port(call, line, busy=port_busy):
    if LISTEN_OK.search(line):
        return None
    for cmd in commands(line):
        if not re.search(r'http\.server|serve|listen|server', cmd, re.I):
            continue
        for p in LISTENERS:
            m = p.search(cmd)
            if m and busy(int(m.group(1))):
                return (f'Port {m.group(1)} already has a listener, probably another session\'s. '
                        'Use that one, or stop it if it is yours (by its exact PID), or pick a free port.')
    return None


def expected_base(head):
    """The base a branch name implies: 'release' (release/0.x), 'main', or None (either is fine)."""
    if head.startswith(('backport/', 'release/')):
        return 'release'
    if head.startswith('forward/') or head.endswith('-main'):
        return 'main'
    # ble-status-0.1, docs/release-notes-0.1-backport. A docs branch named after the release
    # (docs/release-notes-0.1) documents it on main.
    if re.search(r'-0\.\d+$', head) and not head.startswith('docs/'):
        return 'release'
    if head.endswith('-backport'):
        return 'release'
    return None


def base_problem(head, base):
    want = expected_base(head)
    if want == 'release' and not base.startswith('release/'):
        return f'{head} is a release branch by its name but targets {base}; use --base release/0.1.'
    if want == 'main' and base != 'main':
        return f'{head} targets {base}, but its name says main.'
    if want is None and base.startswith('release/'):
        return (f'{head} targets {base}. Release fixes go on a backport/... or ...-0.1 branch; '
                'this name reads as main work.')
    return None


def gh_json(args, cwd):
    out = subprocess.run(['gh', *args], cwd=cwd, capture_output=True, text=True, timeout=20)
    return json.loads(out.stdout) if out.returncode == 0 and out.stdout.strip() else None


def current_branch(cwd):
    out = subprocess.run(['git', 'rev-parse', '--abbrev-ref', 'HEAD'], cwd=cwd, capture_output=True, text=True, timeout=10)
    return out.stdout.strip()


def check_base(call, line, gh=gh_json, branch=current_branch):
    if BASE_OK.search(line):
        return None
    cwd = call.get('cwd')
    for cmd in commands(line):
        push = re.search(r'\bgit\s+push\b(.*)', cmd)
        if push:
            refs = push.group(1)
            if re.search(r'(^|\s|:)(refs/heads/)?(main|release/[\w.]+)(\s|$)', refs) and not re.search(r'--delete|\s-d\b', refs):
                return 'No pushes straight to main or release/*: push a branch and open a PR (RESCUE.md).'
        merge = re.search(r'\bgh\s+pr\s+merge\b\s*(\d+)?', cmd)
        if merge:
            args = ['pr', 'view'] + ([merge.group(1)] if merge.group(1) else []) + ['--json', 'headRefName,baseRefName']
            pr = gh(args, cwd)
            if pr:
                problem = base_problem(pr['headRefName'], pr['baseRefName'])
                if problem:
                    return problem + ' Fix the base with gh pr edit --base, or prefix FOXGLOVE_ALLOW_BASE=1 if it is deliberate.'
        create = re.search(r'\bgh\s+pr\s+create\b(.*)', cmd)
        if create:
            opts = create.group(1)
            base = re.search(r'(?:--base|-B)[\s=]["\']?([\w./-]+)', opts)
            head = re.search(r'(?:--head|-H)[\s=]["\']?([\w./-]+)', opts)
            problem = base_problem(head.group(1) if head else branch(cwd), base.group(1) if base else 'main')
            if problem:
                return problem + ' Prefix FOXGLOVE_ALLOW_BASE=1 if it is deliberate.'
    return None


def worktree_root(cwd):
    """Where worktrees go: FOXGLOVE_WORKTREES, else `worktrees` beside the main checkout
    (C:/Users/fuchs/splitkeeb/worktrees). Never inside the repo: cargo walks up from a nested
    app/ into the main checkout's workspace and refuses to build."""
    if os.environ.get('FOXGLOVE_WORKTREES'):
        return Path(os.environ['FOXGLOVE_WORKTREES'])
    out = subprocess.run(['git', 'rev-parse', '--path-format=absolute', '--git-common-dir'],
                         cwd=cwd, capture_output=True, text=True, timeout=10)
    return Path(out.stdout.strip()).parent.parent / 'worktrees'


def inside(path, root):
    try:
        Path(os.path.abspath(path)).relative_to(os.path.abspath(root))
        return True
    except ValueError:
        return False


def check_worktree(call, line, root=worktree_root):
    for cmd in commands(line):
        m = re.search(r'\bgit\b(.*?)\bworktree\s+add\b(.*)', cmd)
        if not m:
            continue
        at = re.search(r'-C\s+("[^"]*"|\'[^\']*\'|\S+)', m.group(1))
        base = at.group(1).strip('"\'') if at else (call.get('cwd') or '.')
        # The first argument that isn't an option (or an option's value) is the path.
        words = re.findall(r'"[^"]*"|\'[^\']*\'|\S+', m.group(2))
        path, skip = None, False
        for w in words:
            if skip:
                skip = False
            elif w in ('-b', '-B', '--reason'):
                skip = True
            elif not w.startswith('-'):
                path = w.strip('"\'')
                break
        if not path:
            continue
        want = root(base)
        if not inside(os.path.join(base, path), want):
            return (f'Worktrees go in {want}\\<name>: there clean-worktrees.ps1 finds them, and the app '
                    'still builds (inside the repo, cargo picks up the main checkout\'s workspace).')
    return None


HEAVY = re.compile(r'\bcargo\s+(\+\S+\s+)?(build|b|test|t|clippy|check|c|run|r|doc|d|bench|install|rustc|nextest|wix)\b')


def check_cargo(call, line):
    if re.search(r'check\.ps1|build-slot\.ps1|release-test\.ps1', line):
        return None
    for cmd in commands(line):
        if HEAVY.search(cmd):
            return ('Cargo builds run in a build slot so at most 2 run at once (#347). Run it as: '
                    'powershell -File scripts\\build-slot.ps1 ' + HEAVY.search(cmd).group(0) + ' ...')
    return None


RULES = [check_flash, check_app, check_port, check_base, check_worktree, check_cargo]


def evaluate(call):
    if call.get('tool_name') not in ('Bash', 'PowerShell'):
        return None
    line = (call.get('tool_input') or {}).get('command') or ''
    for rule in RULES:
        reason = rule(call, line)
        if reason:
            return reason
    return None


def main():
    try:
        reason = evaluate(json.load(sys.stdin))
    except Exception as e:  # never block work because the guard itself broke
        print(f'guard.py: skipped ({e})', file=sys.stderr)
        return 0
    if reason:
        print(reason, file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    sys.exit(main())
