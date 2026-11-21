"""Offline checks of a built Foxglove or Hideout MSI: reads its tables, installs nothing.

  python scripts/msi_tables.py <file.msi> --product hideout|foxglove

The MSI is opened read-only through msi.dll (the Windows Installer API) and only its database
tables are read: no install, no custom action, no window. It proves what the package DECLARES
(upgrade and downgrade rows, per-user scope, no autostart entry, what uninstall removes, no stray
files), not that Windows executes it correctly; review/lab-vm-msi-options.md option D.
Exit 0 = every check passed, 1 = a check failed (listed), 2 = could not read the MSI.
"""
import argparse
import ctypes
import re
import sys
from ctypes import wintypes

PRODUCTS = {
    'hideout': {
        'upgrade_code': '{0A0A820B-B3AE-427A-91A7-E7FF56509254}',
        'files': {'hideout.exe', 'LICENSE-hideout'},
        'registry_key': 'Software\\Hideout\\Installer',
        'autostart_removal': ('RemoveAutostart', '--autostart-remove'),
    },
    'foxglove': {
        'upgrade_code': '{40F2C6EE-F034-4FE5-91BC-65F1E74F7D24}',
        # The identity files exist only in the SparseIdentity variant.
        'files': {'foxglove.exe', 'LICENSE', 'foxglove-identity.msix', 'logo50.png', 'logo150.png', 'logo44.png'},
        'registry_key': 'Software\\Foxglove\\Installer',
        'autostart_removal': ('RemoveAutostart', '--uninstall-cleanup'),
    },
}

# Per-user installs write only under the user's profile and the user's Start menu.
ALLOWED_ROOTS = {'LocalAppDataFolder', 'ProgramMenuFolder'}
STARTUP_ROOTS = {'StartupFolder', 'CommonStartupFolder'}
HKCU = 1
# Upgrade.Attributes
UPG_ONLY_DETECT = 2
UPG_MAX_INCLUSIVE = 256
# CustomAction.Type bits
CA_EXE = 2
CA_FILE_SOURCE = 16
CA_CONTINUE = 64
CA_DEFERRED = 1024
CA_NO_IMPERSONATE = 2048
# RemoveFile.InstallMode
RM_ON_REMOVE = 2


class MsiError(Exception):
    pass


def _lib():
    if sys.platform != 'win32':
        raise MsiError('msi.dll is Windows only')
    msi = ctypes.WinDLL('msi', use_last_error=True)
    msi.MsiOpenDatabaseW.argtypes = [wintypes.LPCWSTR, ctypes.c_void_p, ctypes.POINTER(wintypes.ULONG)]
    msi.MsiDatabaseOpenViewW.argtypes = [wintypes.ULONG, wintypes.LPCWSTR, ctypes.POINTER(wintypes.ULONG)]
    msi.MsiViewExecute.argtypes = [wintypes.ULONG, wintypes.ULONG]
    msi.MsiViewFetch.argtypes = [wintypes.ULONG, ctypes.POINTER(wintypes.ULONG)]
    msi.MsiViewGetColumnInfo.argtypes = [wintypes.ULONG, wintypes.UINT, ctypes.POINTER(wintypes.ULONG)]
    msi.MsiRecordGetFieldCount.argtypes = [wintypes.ULONG]
    msi.MsiRecordGetFieldCount.restype = wintypes.UINT
    msi.MsiRecordGetStringW.argtypes = [wintypes.ULONG, wintypes.UINT, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
    msi.MsiRecordSetStringW.argtypes = [wintypes.ULONG, wintypes.UINT, wintypes.LPCWSTR]
    msi.MsiDatabaseCommit.argtypes = [wintypes.ULONG]
    msi.MsiCloseHandle.argtypes = [wintypes.ULONG]
    return msi


def _string(msi, rec, i):
    n = wintypes.DWORD(0)
    r = msi.MsiRecordGetStringW(rec, i, None, ctypes.byref(n))
    if r not in (0, 234):
        raise MsiError(f'MsiRecordGetString {r}')
    buf = ctypes.create_unicode_buffer(n.value + 1)
    n = wintypes.DWORD(n.value + 1)
    r = msi.MsiRecordGetStringW(rec, i, buf, ctypes.byref(n))
    if r:
        raise MsiError(f'MsiRecordGetString {r}')
    return buf.value


class Database:
    """Read-only by default; mode='create' is for the test fixtures only."""

    def __init__(self, path, mode='read'):
        self.msi = _lib()
        self.h = wintypes.ULONG(0)
        persist = {'read': 0, 'create': 4}[mode]
        r = self.msi.MsiOpenDatabaseW(str(path), ctypes.c_void_p(persist), ctypes.byref(self.h))
        if r:
            raise MsiError(f"cannot open '{path}' (msi error {r})")

    def close(self):
        self.msi.MsiCloseHandle(self.h)

    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.close()

    def execute(self, sql):
        view = wintypes.ULONG(0)
        r = self.msi.MsiDatabaseOpenViewW(self.h, sql, ctypes.byref(view))
        if r:
            raise MsiError(f'bad SQL (msi error {r}): {sql}')
        try:
            r = self.msi.MsiViewExecute(view, 0)
            if r:
                raise MsiError(f'execute failed (msi error {r}): {sql}')
            if not sql.lstrip().upper().startswith('SELECT'):
                return None, []
            cols = None
            names = wintypes.ULONG(0)
            if self.msi.MsiViewGetColumnInfo(view, 0, ctypes.byref(names)) == 0:
                n = self.msi.MsiRecordGetFieldCount(names)
                cols = [_string(self.msi, names, i) for i in range(1, n + 1)]
                self.msi.MsiCloseHandle(names)
            rows = []
            while True:
                rec = wintypes.ULONG(0)
                r = self.msi.MsiViewFetch(view, ctypes.byref(rec))
                if r == 259:
                    break
                if r:
                    raise MsiError(f'fetch failed (msi error {r})')
                n = self.msi.MsiRecordGetFieldCount(rec)
                rows.append([_string(self.msi, rec, i) for i in range(1, n + 1)])
                self.msi.MsiCloseHandle(rec)
            return cols, rows
        finally:
            self.msi.MsiCloseHandle(view)

    def commit(self):
        self.msi.MsiDatabaseCommit(self.h)

    def tables(self):
        return {r[0] for r in self.execute('SELECT `Name` FROM `_Tables`')[1]}

    def table(self, name):
        """List of dicts (column -> text), [] when the package has no such table."""
        if name not in self.tables():
            return []
        cols, rows = self.execute(f'SELECT * FROM `{name}`')
        return [dict(zip(cols, r)) for r in rows]


def read_package(path):
    with Database(path) as db:
        names = ('Property', 'Upgrade', 'LaunchCondition', 'Registry', 'Shortcut', 'File', 'Component',
                 'Directory', 'CustomAction', 'InstallExecuteSequence', 'RemoveFile')
        return {n: db.table(n) for n in names}


def _int(s):
    try:
        return int(s)
    except (TypeError, ValueError):
        return 0


def _long_name(name):
    """Directory and File names are 'short|long' when they differ."""
    return (name or '').split('|')[-1]


def check(pkg, product):
    """Returns the list of failed checks (empty = the package declares what it should)."""
    spec = PRODUCTS[product]
    bad = []
    props = {r['Property']: r['Value'] for r in pkg['Property']}

    # --- identity and version
    if props.get('UpgradeCode', '').upper() != spec['upgrade_code']:
        bad.append(f"UpgradeCode is {props.get('UpgradeCode')!r}, want {spec['upgrade_code']}")
    version = props.get('ProductVersion', '')
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        bad.append(f'ProductVersion {version!r} is not MAJOR.MINOR.PATCH')

    # --- upgrade and downgrade
    mine = [r for r in pkg['Upgrade'] if r['UpgradeCode'].upper() == spec['upgrade_code']]
    migrate = [r for r in mine if not _int(r['Attributes']) & UPG_ONLY_DETECT and r['VersionMax'] == version
               and _int(r['Attributes']) & UPG_MAX_INCLUSIVE]
    if not migrate:
        bad.append('no Upgrade row that replaces the same or an older version (VersionMax = this version, inclusive)')
    elif not all(r['ActionProperty'] for r in migrate):
        bad.append('the upgrade row has no ActionProperty')
    # WiX's downgrade row detects versions above this one: VersionMin = this version, exclusive.
    block = [r for r in mine if _int(r['Attributes']) & UPG_ONLY_DETECT and r['VersionMin'] == version
             and r['ActionProperty']]
    if not block:
        bad.append('no Upgrade row that detects a NEWER installed version (downgrade block)')
    else:
        for r in block:
            cond = [c for c in pkg['LaunchCondition'] if f"NOT {r['ActionProperty']}" in c['Condition']]
            if not cond or not all(c['Description'] for c in cond):
                bad.append(f"downgrade is not blocked: no LaunchCondition 'NOT {r['ActionProperty']}' with a message")
    if not any(r['Action'] == 'RemoveExistingProducts' for r in pkg['InstallExecuteSequence']):
        bad.append('RemoveExistingProducts is not scheduled: an upgrade would leave the old version installed')

    # --- per-user scope
    if props.get('ALLUSERS', ''):
        bad.append(f"ALLUSERS is {props['ALLUSERS']!r}: the package is not per-user")
    dirs = {d['Directory']: d['Directory_Parent'] for d in pkg['Directory']}

    def chain_of(d):
        chain = []
        while d and d not in chain:
            chain.append(d)
            d = dirs.get(d, '')
        return chain

    comp_dir = {c['Component']: c['Directory_'] for c in pkg['Component']}
    for c, d in comp_dir.items():
        chain = chain_of(d)
        known = [n for n in chain if n.endswith('Folder') and n not in ('INSTALLFOLDER', 'TARGETDIR')]
        if not known or known[-1] not in ALLOWED_ROOTS:
            bad.append(f"component {c} installs under {known[-1] if known else d}, not the user's profile or Start menu")
    for r in pkg['Registry']:
        if _int(r['Root']) != HKCU:
            bad.append(f"registry {r['Key']}\\{r['Name']} is under root {r['Root']}, want HKCU (1)")

    # --- no autostart
    for r in pkg['Registry']:
        segs = [s.lower() for s in re.split(r'[\\/]', r['Key'])]
        if 'run' in segs or 'runonce' in segs:
            bad.append(f"registry key {r['Key']} is a Run key: the installer must not turn on Start with Windows")
        if not r['Key'].startswith(spec['registry_key']) and not r['Key'].lower().startswith('software\\' + product):
            bad.append(f"registry key {r['Key']} is outside {spec['registry_key']}")
    for s in pkg['Shortcut']:
        if STARTUP_ROOTS & set(chain_of(s['Directory_'])):
            bad.append(f"shortcut {s['Shortcut']} is in a Startup folder")
        elif s['Directory_'] != 'ProgramMenuFolder':
            bad.append(f"shortcut {s['Shortcut']} is in {s['Directory_']}, want the Start menu")
    for f in pkg['File']:
        if _long_name(f['FileName']).lower().endswith('.lnk'):
            bad.append(f"file {f['FileName']} is a shortcut written as a file")

    # --- no stray files
    for f in pkg['File']:
        name = _long_name(f['FileName'])
        if name not in spec['files']:
            bad.append(f'unexpected file {name}')
    if not any(_long_name(f['FileName']) == f'{product}.exe' for f in pkg['File']):
        bad.append(f'{product}.exe is not in the package')

    # --- uninstall removes what install added
    remove_dirs = [r for r in pkg['RemoveFile'] if not r['FileName'] and _int(r['InstallMode']) & RM_ON_REMOVE]
    file_components = {f['Component_'] for f in pkg['File']}
    for c in file_components:
        d = comp_dir.get(c)
        if not any(r['Component_'] == c and r['DirProperty'] == d for r in remove_dirs):
            bad.append(f'uninstall does not remove the folder {d} of component {c}')
    for r in pkg['Registry']:
        # Registry values of a component are removed with it; a value that is not tied to a component cannot be.
        if not r['Component_']:
            bad.append(f"registry {r['Key']}\\{r['Name']} belongs to no component")
    action, arg = spec['autostart_removal']
    ca = [c for c in pkg['CustomAction'] if c['Action'] == action]
    if not ca:
        bad.append(f'custom action {action} is missing')
    else:
        t = _int(ca[0]['Type'])
        if ca[0]['Target'] != arg:
            bad.append(f"{action} runs {ca[0]['Target']!r}, want {arg!r}")
        if not t & CA_EXE or not t & CA_FILE_SOURCE:
            bad.append(f'{action} does not run the installed exe')
        if not t & CA_DEFERRED:
            bad.append(f'{action} is not deferred')
        if t & CA_NO_IMPERSONATE:
            bad.append(f'{action} runs as the system, not as the user')
        if not t & CA_CONTINUE:
            bad.append(f'{action} does not ignore its exit code: a failure would block the uninstall')
    seq = {r['Action']: r for r in pkg['InstallExecuteSequence']}
    if action not in seq:
        bad.append(f'{action} is not scheduled')
    else:
        cond = seq[action]['Condition']
        if 'REMOVE="ALL"' not in cond or 'NOT UPGRADINGPRODUCTCODE' not in cond:
            bad.append(f'{action} condition is {cond!r}: it must run only on uninstall, not on an upgrade')
        rf = seq.get('RemoveFiles')
        if rf and _int(seq[action]['Sequence']) >= _int(rf['Sequence']):
            bad.append(f'{action} runs after RemoveFiles, when the exe is gone')
    return bad


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('msi')
    ap.add_argument('--product', required=True, choices=sorted(PRODUCTS))
    args = ap.parse_args(argv)
    try:
        pkg = read_package(args.msi)
    except MsiError as e:
        print(f'msi_tables: {e}')
        return 2
    bad = check(pkg, args.product)
    for b in bad:
        print(f'FAIL: {b}')
    if bad:
        return 1
    print(f'msi_tables: {args.msi} ({args.product}): all checks passed')
    return 0


if __name__ == '__main__':
    sys.exit(main())
