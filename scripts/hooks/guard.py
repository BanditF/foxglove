"""PreToolUse hook: turns the roster's house rules into checks before a shell command runs.

Claude Code runs it before every Bash or PowerShell tool call (.claude/settings.json) with the
call as JSON on stdin. Exit 2 blocks the call and shows the reason to the agent; anything else
lets it run. It guards against accidents, not against a determined agent: each rule names the
one deliberate way past it. Any error inside the guard lets the command run (fail open).

Rules look only at what a command runs (its head), never at text it carries: a commit message,
PR body, heredoc or grep pattern that mentions cargo or the flasher passes.
  flash      only 7 · On-call runs a flasher or `foxglove-lab bootloader` (dry runs and --help
             are fine for everyone). The marker is the only proof: an on-call subagent isn't it.
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

# What may come before the program a command runs: env assignments, `timeout N`, `cmd /c`,
# PowerShell's call operator.
HEAD = r'^\s*(?:(?:\$env:)?[A-Za-z_]\w*=\S*\s+|timeout\s+(?:-\S+\s+)*\d+\S*\s+|cmd(?:\.exe)?\s+/c\s+|&\s*)*'
PROG = r'["\']?(?:[^\s"\';|]*[\\/])?'          # an optional path before the program name
PYTHON = HEAD + PROG + r'(?:python[\d.]*|py)(?:\.exe)?["\']?\s+(?:-\S+\s+)*'
POWERSHELL = HEAD + PROG + r'(?:powershell|pwsh)(?:\.exe)?["\']?\s+(?:-(?!File\b)\S+\s+(?:[^-\s]\S*\s+)?)*-File\s+'


def strip_bodies(line):
    """Drops heredoc and here-string bodies: they are text, not commands."""
    line = re.sub(r"<<-?\s*(['\"]?)(\w+)\1[^\n]*\n.*?\n\s*\2[ \t]*(?=\n|$)", '<<BODY', line, flags=re.S)
    return re.sub(r"@(['\"])\r?\n.*?\r?\n\1@", "''", line, flags=re.S)


def commands(line):
    """Splits a shell line on ; | && || and newlines outside quotes."""
    out, cur, quote, i = [], [], None, 0
    line = strip_bodies(line)
    while i < len(line):
        ch = line[i]
        if quote:
            cur.append(ch)
            if ch == quote:
                quote = None
        elif ch in '"\'':
            quote = ch
            cur.append(ch)
        elif line.startswith(('&&', '||'), i):
            out.append(''.join(cur)); cur = []; i += 1
        elif ch in ';|\n':
            out.append(''.join(cur)); cur = []
        else:
            cur.append(ch)
        i += 1
    out.append(''.join(cur))
    return [c for c in out if c.strip()]


def words(text):
    return [w.strip('"\'') for w in re.findall(r'"[^"]*"|\'[^\']*\'|\S+', text)]


def is_oncall(call, line):
    # The marker only: an on-call *subagent* started from another session is not the standing
    # On-call session, and two flashers on one bootloader drive risk a wrong-half write.
    return bool(ONCALL.search(line))


def check_flash(call, line):
    for cmd in commands(line):
        flasher = re.match(PYTHON + r'["\']?\S*foxglove_flash\.py\b(.*)', cmd)
        if flasher and re.search(r'(^|\s)(--dry-run|-h|--help)(\s|$)', flasher.group(1)):
            flasher = None
        half = re.match(r'(?:' + POWERSHELL + r'|' + HEAD + r')' + PROG + r'flash-half\.ps1\b', cmd)
        # Bootloader entry (D7 08) is half a flash: the half stops typing until it gets a UF2.
        boot = re.match(r'(?:' + HEAD + PROG + r'foxglove-lab(?:\.exe)?["\']?|' + HEAD + PROG +
                        r'cargo(?:\.exe)?["\']?\s+run\b.*?-p\s+foxglove-lab\b.*?--)\s.*\bbootloader\b', cmd)
        if (flasher or half or boot) and not is_oncall(call, line):
            return ('Only 7 · On-call flashes the keyboard (AGENTS-ROSTER.md). '
                    'Send the flash request to On-call through the Hub; --dry-run is fine here.')
    return None


APP_LAUNCH = [
    re.compile(HEAD + r'Start-Process\b[^;|\n]*\bfoxglove(\.exe)?\b(?![-_.]\w)', re.I),
    re.compile(HEAD + PROG + r'foxglove\.exe\b', re.I),
    re.compile(PYTHON + r'.*--app[\s=]["\']?[^\s"\']*foxglove\.exe', re.I),
    re.compile(HEAD + PROG + r'cargo(?:\.exe)?["\']?\s+run\b.*(--manifest-path\s+["\']?app[\\/]|(-p|--package|--bin)\s+foxglove(\s|$))', re.I),
]


def check_app(call, line):
    if is_oncall(call, line) or APP_OK.search(line):
        return None
    for cmd in commands(line):
        if any(p.match(cmd) for p in APP_LAUNCH):
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


def check_port(call, line, busy=None):
    busy = busy or port_busy
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


PROTECTED = re.compile(r'\+?(?:[^:\s]*:)?(?:refs/heads/)?(main|release/[\w.]+)')
GIT = HEAD + PROG + r'git(?:\.exe)?["\']?\s+(?:-C\s+("[^"]*"|\'[^\']*\'|\S+)\s+)?'


def check_base(call, line, gh=None, branch=None):
    gh, branch = gh or gh_json, branch or current_branch
    if BASE_OK.search(line):
        return None
    for cmd in commands(line):
        push = re.match(GIT + r'push\b(.*)', cmd)
        if push:
            cwd = (push.group(1) or '').strip('"\'') or call.get('cwd')
            refs = [w for w in words(push.group(2)) if not w.startswith('-')]
            # refs[0] is the remote; a bare push goes to the current branch's upstream.
            targets = refs[1:] or [branch(cwd)]
            if any(PROTECTED.fullmatch(r) for r in targets):
                return 'No pushes straight to main or release/*: push a branch and open a PR (RESCUE.md).'
        merge = re.match(r'(?:' + HEAD + PROG + r')gh(?:\.exe)?["\']?\s+pr\s+merge\b(.*)', cmd)
        if merge:
            number = re.search(r'(?:^|\s)["\']?(\d+|https://\S+/pull/\d+)["\']?(?=\s|$)', merge.group(1))
            pr = gh(['pr', 'view'] + ([number.group(1)] if number else []) + ['--json', 'headRefName,baseRefName'], call.get('cwd'))
            if pr:
                problem = base_problem(pr['headRefName'], pr['baseRefName'])
                if problem:
                    return problem + ' Fix the base with gh pr edit --base, or prefix FOXGLOVE_ALLOW_BASE=1 if it is deliberate.'
        create = re.match(r'(?:' + HEAD + PROG + r')gh(?:\.exe)?["\']?\s+pr\s+create\b(.*)', cmd)
        if create:
            opts = create.group(1)
            base = re.search(r'(?:--base|-B)[\s=]["\']?([\w./-]+)', opts)
            head = re.search(r'(?:--head|-H)[\s=]["\']?([\w./-]+)', opts)
            problem = base_problem(head.group(1) if head else branch(call.get('cwd')), base.group(1) if base else 'main')
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


def windows_path(path):
    """Git Bash spells C:\\x as /c/x."""
    return re.sub(r'^/([a-zA-Z])/', r'\1:/', str(path))


def inside(path, root):
    try:
        Path(os.path.abspath(windows_path(path))).relative_to(os.path.abspath(windows_path(root)))
        return True
    except ValueError:
        return False


def check_worktree(call, line, root=None):
    root = root or worktree_root
    for cmd in commands(line):
        m = re.match(GIT + r'worktree\s+add\b(.*)', cmd)
        if not m:
            continue
        base = windows_path((m.group(1) or '').strip('"\'') or call.get('cwd') or '.')
        # The first argument that isn't an option (or an option's value) is the path.
        path, skip = None, False
        for w in words(m.group(2)):
            if skip:
                skip = False
            elif w in ('-b', '-B', '--reason', '--orphan'):
                skip = True
            elif not w.startswith('-'):
                path = windows_path(w)
                break
        if not path:
            continue
        want = root(base)
        if not inside(os.path.join(base, path), want):
            return (f'Worktrees go in {want}\\<name>: there clean-worktrees.ps1 finds them, and the app '
                    'still builds (inside the repo, cargo picks up the main checkout\'s workspace).')
    return None


HEAVY = re.compile(HEAD + PROG + r'cargo(?:\.exe)?["\']?\s+(?:\+\S+\s+)?(build|b|test|t|clippy|check|c|run|r|doc|d|bench|install|rustc|nextest|wix)\b')


def check_cargo(call, line):
    for cmd in commands(line):
        m = HEAVY.match(cmd)
        if m:
            return ('Cargo builds run in a build slot so at most 2 run at once (#347). Run it as: '
                    f'powershell -File scripts\\build-slot.ps1 cargo {m.group(1)} ...')
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
    if hasattr(sys.stderr, 'reconfigure'):  # the agent reads the reason as UTF-8
        sys.stderr.reconfigure(encoding='utf-8')
    try:
        reason = evaluate(json.loads(sys.stdin.buffer.read().decode('utf-8-sig')))
    except Exception as e:  # never block work because the guard itself broke
        print(f'guard.py: skipped ({e})', file=sys.stderr)
        return 0
    if reason:
        print(reason, file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    sys.exit(main())
