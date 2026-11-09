"""Hub dependency licences (#932, contract §11): the Hideout hub is MIT and clean-room, so every
package in the dependency graph of the hub crates must carry a permissive licence from ALLOWED, and
no path dependency may point under app/ (GPL-3.0).

    python scripts/check_hub_deps.py            # runs cargo metadata on the crates/ workspace
    python scripts/check_hub_deps.py --metadata saved.json

The graph is cargo's resolve for the platform the hub ships on (PLATFORM, Windows x64; the hub's
GUI dependencies are Windows-only, and some of theirs exist only on Linux) with every feature,
from the hub crates (ROOTS) through
normal and build dependencies; the roots' own dev-dependencies count too. A licence is a cargo
SPDX expression: an OR passes when one side passes, an AND when both do; "A/B" is the old OR
spelling. A package with no `license` field fails. Exit 0 when clean, 1 with one line per problem.
"""
import argparse, json, re, subprocess, sys
from pathlib import Path, PurePath

REPO = Path(__file__).resolve().parent.parent
ROOTS = ('hideout-shell', 'hideout')
PLATFORM = 'x86_64-pc-windows-msvc'
# Permissive only. Copyleft of any strength (GPL, LGPL, AGPL, MPL, EPL, OFL...) is not here.
ALLOWED = {
    'MIT', 'MIT-0', 'Apache-2.0', 'Apache-2.0 WITH LLVM-exception', 'BSD-2-Clause',
    'BSD-3-Clause', 'ISC', 'Zlib', 'Unicode-3.0', 'Unicode-DFS-2016', 'Unlicense', '0BSD',
    'BSL-1.0', 'CC0-1.0',
}
# The hub's own crates are MIT, nothing else.
OWN_LICENCE = 'MIT'


def tokens(expr):
    return re.findall(r'\(|\)|[^\s()]+', expr.replace('/', ' OR '))


def licence_ok(expr, allowed=ALLOWED):
    """True when the SPDX expression `expr` is satisfiable from `allowed`."""
    toks = tokens(expr or '')
    pos = 0

    def peek():
        return toks[pos] if pos < len(toks) else None

    def take():
        nonlocal pos
        pos += 1
        return toks[pos - 1]

    def atom():
        t = take() if peek() is not None else None
        if t is None or t in ('OR', 'AND', 'WITH', ')'):
            raise ValueError(f'bad licence expression: {expr!r}')
        if t == '(':
            v = either()
            if peek() != ')':
                raise ValueError(f'unbalanced licence expression: {expr!r}')
            take()
            return v
        if peek() == 'WITH':
            take()
            t = f'{t} WITH {take()}'
        return t.rstrip('+') in allowed or t in allowed

    def both():
        v = atom()
        while peek() == 'AND':
            take()
            v = atom() and v
        return v

    def either():
        v = both()
        while peek() == 'OR':
            take()
            v = both() or v
        return v

    if not toks:
        return False
    v = either()
    if peek() is not None:
        raise ValueError(f'trailing tokens in licence expression: {expr!r}')
    return v


def under(path, folder):
    """True when `path` lies inside `folder` (both absolute; case-insensitive, as on Windows)."""
    p, f = PurePath(str(path).lower()), PurePath(str(folder).lower())
    return p == f or f in p.parents


def hub_graph(meta, roots=ROOTS):
    """The package ids reachable from the root packages, as {id: package}."""
    pkgs = {p['id']: p for p in meta['packages']}
    root_ids = [p['id'] for p in meta['packages'] if p['name'] in roots and p.get('source') is None]
    missing = set(roots) - {pkgs[i]['name'] for i in root_ids}
    if missing:
        raise ValueError(f'hub crates not in the workspace: {", ".join(sorted(missing))}')
    nodes = {n['id']: n for n in meta['resolve']['nodes']}
    seen, stack = {}, list(root_ids)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen[pid] = pkgs[pid]
        for dep in nodes.get(pid, {}).get('deps', []):
            kinds = {k.get('kind') for k in dep.get('dep_kinds', [{'kind': None}])}
            if pid in root_ids or kinds - {'dev'}:
                stack.append(dep['pkg'])
    return seen


def problems(meta, app_dir, roots=ROOTS):
    """One line per rule broken, sorted; empty when the hub's graph is clean."""
    out = []
    for pkg in hub_graph(meta, roots).values():
        name = f"{pkg['name']} {pkg['version']}"
        local = pkg.get('source') is None
        if local and under(pkg['manifest_path'], app_dir):
            out.append(f'{name}: a path dependency under app/ ({pkg["manifest_path"]})')
        for dep in pkg.get('dependencies', []):
            if dep.get('path') and under(dep['path'], app_dir):
                out.append(f'{name}: depends on {dep["name"]} by a path under app/ ({dep["path"]})')
        lic = pkg.get('license')
        if not lic:
            out.append(f'{name}: no licence field' + (f' (licence file {pkg["license_file"]})' if pkg.get('license_file') else ''))
            continue
        try:
            ok = (lic.strip() == OWN_LICENCE) if local else licence_ok(lic)
        except ValueError as e:
            out.append(f'{name}: {e}')
            continue
        if not ok:
            out.append(f'{name}: licence {lic!r} is not on the hub allowlist' if not local
                       else f'{name}: our own crate must be {OWN_LICENCE}, not {lic!r}')
    return sorted(set(out))


def cargo_metadata(platform=PLATFORM):
    res = subprocess.run(['cargo', 'metadata', '--format-version', '1', '--locked', '--all-features',
                          '--filter-platform', platform],
                         cwd=REPO, capture_output=True, text=True, encoding='utf-8')
    if res.returncode:
        raise RuntimeError(f'cargo metadata failed: {res.stderr.strip()[-500:]}')
    return json.loads(res.stdout)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--metadata', type=Path, help='a saved `cargo metadata` JSON instead of running cargo')
    ap.add_argument('--platform', default=PLATFORM, help=f'the target triple to resolve for (default {PLATFORM})')
    args = ap.parse_args(argv)
    meta = json.loads(args.metadata.read_text(encoding='utf-8')) if args.metadata else cargo_metadata(args.platform)
    found = problems(meta, Path(meta.get('workspace_root', REPO)) / 'app')
    for line in found:
        print(f'hub deps: {line}')
    n = len(hub_graph(meta))
    print(f'hub deps: {n} packages checked, {len(found)} problem(s)')
    return 1 if found else 0


if __name__ == '__main__':
    sys.exit(main())
