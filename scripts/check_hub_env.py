"""Hub environment reads (#1466): the installed Hideout is built without the `lab-switches`
feature and must not take orders from environment variables. This check fails when hub code reads
the environment somewhere that is neither gated on the feature nor listed here with its reason.

    python scripts/check_hub_env.py

It looks at every `.rs` file under the hub crates' `src` folders (CRATES), outside test code, for
`env::var`, `env::var_os`, `env::vars` and `env::vars_os`. A read passes when:

- it names a Windows variable from SYSTEM on its own line (`SystemRoot`, `USERPROFILE`, ...);
- its function is gated: `#[cfg(feature = "lab-switches")]` on the function, or
  `cfg!(feature = "lab-switches")` in the function above the read;
- its (file, function) is in OPEN, with why the read is safe in the installed build.

It also fails when a `HIDEOUT_*` name in hub code is neither a switch the installed build logs as
ignored (the VARS list in crates/hideout-registry/src/lab.rs) nor in NOT_SWITCHES, so a new
switch can't be added without the log line that says the build ignores it. An OPEN entry that
matches nothing fails too. Exit 0 when clean, 1 with one line per problem.

Test code is a file named `tests.rs` or `*_tests.rs`, a `tests` folder, and everything from a
`#[cfg(test)]` line followed by `mod <name> {` to the end of the file (test modules come last).
"""
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CRATES = 'crates'
PREFIX = 'hideout'
LAB_RS = 'crates/hideout-registry/src/lab.rs'

# Windows' own variables: where system folders are and who is signed in. Not switches.
SYSTEM = {'SystemRoot', 'WINDIR', 'USERNAME', 'USERPROFILE', 'COMPUTERNAME'}

# (file, function): why this ungated read is safe in the installed build.
OPEN = {
    ('crates/hideout-registry/src/lab.rs', 'log_ignored_switches'):
        'names the ignored switches in the log; never their values, never acts on them',
    ('crates/hideout-registry/src/dev_folder.rs', 'env_path'):
        'system folders (TEMP, ProgramFiles...) a dev folder may not be in: it only refuses',
    ('crates/hideout-shell/src/paths.rs', 'from_env'):
        'the lookup for resolve(): home_override and local_app_data are gated on the feature',
    ('crates/hideout-shell/src/shell.rs', 'start_capture'):
        'capture::enabled needs an overridden folder, which the installed build never has',
    ('crates/hideout-supervisor/src/sandbox.rs', 'env_block'):
        "hands a contained module a short fixed list of the hub's variables; reads no switch",
    ('crates/hideout/src/find_installed.rs', 'env'):
        'system folder variables for finding installed programs; no switch',
}

# HIDEOUT_* names that are not switches: what the hub hands a module it starts.
NOT_SWITCHES = {'HIDEOUT_PIPE', 'HIDEOUT_TOKEN', 'HIDEOUT_INSTANCE', 'HIDEOUT_HUB_PID'}

READ = re.compile(r'\benv::vars?(_os)?\s*\(')
FN = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)')
NAME = re.compile(r'"(HIDEOUT_[A-Z0-9_]+)"')
GATE_ATTR = re.compile(r'#\[cfg\(\s*feature\s*=\s*"lab-switches"\s*\)\]')
GATE_MACRO = re.compile(r'cfg!\(\s*feature\s*=\s*"lab-switches"\s*\)')
TEST_MOD = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{')


def is_test_file(rel):
    p = Path(rel)
    return p.name == 'tests.rs' or p.name.endswith('_tests.rs') or 'tests' in p.parts[:-1]


def production(lines):
    """The lines before the file's test module: up to a `#[cfg(test)]` that a `mod x {` follows."""
    for i, line in enumerate(lines):
        if line.strip() == '#[cfg(test)]':
            for nxt in lines[i + 1:]:
                if nxt.strip().startswith('#[') or not nxt.strip():
                    continue
                if TEST_MOD.match(nxt):
                    return lines[:i]
                break
    return lines


def code(line):
    """The line without a trailing `//` comment (a doc line is all comment)."""
    return line.split('//', 1)[0]


def reads(lines):
    """(line number, function name, gated, system) for each environment read in `lines`."""
    out = []
    for i, line in enumerate(lines):
        if not READ.search(code(line)):
            continue
        f = next((j for j in range(i, -1, -1) if FN.match(lines[j])), None)
        name = FN.match(lines[f]).group(1) if f is not None else ''
        gated = False
        if f is not None:
            j = f - 1
            while j >= 0 and lines[j].strip().startswith(('#[', '///')):
                gated = gated or bool(GATE_ATTR.search(lines[j]))
                j -= 1
            gated = gated or any(
                GATE_MACRO.search(code(x)) or GATE_ATTR.search(code(x)) for x in lines[f:i])
        system = any(f'"{s}"' in code(line) for s in SYSTEM)
        out.append((i + 1, name, gated, system))
    return out


def logged_switches(lab_text):
    """The names in lab.rs's VARS list (LAB_ENV by its value); None when the list is not found."""
    m = re.search(r'const VARS: \[&str; \d+\] = \[(.*?)\];', lab_text, re.S)
    if not m:
        return None
    names = set(re.findall(r'"([A-Z0-9_]+)"', m.group(1)))
    env = re.search(r'pub const LAB_ENV: &str = "([A-Z0-9_]+)";', lab_text)
    if 'LAB_ENV' in m.group(1) and env:
        names.add(env.group(1))
    return names


def check(files, lab_text, open_sites=None):
    """Problems for `files` ({relative path: text}), one line each."""
    open_sites = OPEN if open_sites is None else open_sites
    problems, used = [], set()
    logged = logged_switches(lab_text)
    if logged is None:
        problems.append(f'{LAB_RS}: the VARS list of ignored switches was not found')
        logged = set()
    for rel in sorted(files):
        if is_test_file(rel):
            continue
        lines = production(files[rel].splitlines())
        for n, fn, gated, system in reads(lines):
            if (rel, fn) in open_sites:
                used.add((rel, fn))
            elif not (gated or system):
                problems.append(
                    f'{rel}:{n}: fn {fn} reads the environment outside the lab-switches gate; '
                    'gate it, or list it in OPEN (scripts/check_hub_env.py) with why it is safe')
        for n, line in enumerate(lines, 1):
            for name in NAME.findall(code(line)):
                if name not in logged and name not in NOT_SWITCHES:
                    problems.append(
                        f'{rel}:{n}: {name} is not in the ignored-switch list (VARS in {LAB_RS}); '
                        'add it there so the installed build logs it as ignored')
    for site in sorted(set(open_sites) - used):
        problems.append(f'OPEN lists {site[0]} fn {site[1]}, which reads no environment now')
    return problems


def hub_files(repo=REPO):
    out = {}
    for crate in sorted((repo / CRATES).iterdir()):
        if crate.name.startswith(PREFIX) and (crate / 'src').is_dir():
            for p in sorted((crate / 'src').rglob('*.rs')):
                out[p.relative_to(repo).as_posix()] = p.read_text(encoding='utf-8')
    return out


def main():
    files = hub_files()
    problems = check(files, files.get(LAB_RS, ''))
    for p in problems:
        print(p)
    if not problems:
        print(f'hub environment reads: {len(files)} files, all gated or listed')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
