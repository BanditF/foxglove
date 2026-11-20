"""Dry run for the Hideout installer (H5.6): what crates/hideout/wix/hideout.wxs would write.

Reads the WiX source and prints the files, registry values, shortcuts and uninstall actions an
install would create. Touches nothing: it never runs the installer, the exe or WiX.
Usage: python scripts/hideout_installer_plan.py [--json]
"""
import json
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WXS = ROOT / 'crates' / 'hideout' / 'wix' / 'hideout.wxs'
NS = '{http://schemas.microsoft.com/wix/2006/wi}'
SEP = '\\'
# Folder ids the MSI resolves through Windows (the per-user ones this installer uses).
KNOWN = {
    'LocalAppDataFolder': '%LOCALAPPDATA%',
    'ProgramMenuFolder': SEP.join(['%APPDATA%', 'Microsoft', 'Windows', 'Start Menu', 'Programs']),
}


def plan(wxs=WXS):
    text = Path(wxs).read_text(encoding='utf-8')
    version = re.search(r'<\?define Version = "([^"]+)"', text).group(1)
    root = ET.fromstring(text.replace('$(var.Version)', version))
    product = root.find(NS + 'Product')
    paths = {}
    owner = {}

    def walk(el, base):
        for d in el.findall(NS + 'Directory'):
            ident = d.get('Id')
            name = d.get('Name')
            if ident in KNOWN:
                here = KNOWN[ident]
            elif base and name and ident != 'TARGETDIR':
                here = base + SEP + name
            else:
                here = base
            paths[ident] = here
            for comp in d.findall(NS + 'Component'):
                owner[id(comp)] = ident
            walk(d, here)

    walk(product, '')
    out = {'product': product.get('Name'), 'upgrade_code': product.get('UpgradeCode'),
           'version': product.get('Version'), 'files': [], 'registry': [], 'shortcuts': [],
           'uninstall_actions': [], 'finish_actions': [], 'removes': []}
    for comp in product.iter(NS + 'Component'):
        folder = paths.get(owner.get(id(comp)), '?')
        for f in comp.findall(NS + 'File'):
            out['files'].append(folder + SEP + f.get('Name'))
        for r in comp.findall(NS + 'RegistryValue'):
            out['registry'].append({'key': r.get('Root') + SEP + r.get('Key'), 'name': r.get('Name'),
                                    'type': r.get('Type'), 'value': r.get('Value')})
        for s in comp.findall(NS + 'Shortcut'):
            out['shortcuts'].append({'path': folder + SEP + s.get('Name') + '.lnk', 'target': s.get('Target')})
        for rf in comp.findall(NS + 'RemoveFolder'):
            out['removes'].append(paths.get(rf.get('Directory') or owner.get(id(comp)), '?'))
    for ca in product.findall(NS + 'CustomAction'):
        if ca.get('FileKey'):
            out['uninstall_actions'].append(ca.get('FileKey') + ' ' + ca.get('ExeCommand'))
        else:
            out['finish_actions'].append(ca.get('Id') + ' (only if "Open Hideout now" is ticked, as the user, no arguments)')
    return out


def render(p):
    lines = [f"Hideout installer dry run: {p['product']} {p['version']} (UpgradeCode {p['upgrade_code']})",
             'Nothing is written. An install would create:', 'Files:']
    lines += ['  ' + f for f in p['files']]
    lines.append('Registry (HKCU only):')
    lines += [f"  {r['key']} {r['name']} ({r['type']}) = {r['value']}" for r in p['registry']]
    lines.append('Shortcuts:')
    lines += [f"  {s['path']} -> {s['target']}" for s in p['shortcuts']]
    lines.append('On the finish page, as the user:')
    lines += ['  ' + a for a in p['finish_actions']]
    lines.append('On uninstall (not an upgrade), as the user, before the files go:')
    lines += ['  ' + a for a in p['uninstall_actions']]
    lines.append('Uninstall removes all of the above, and these folders if empty: ' + ', '.join(p['removes']))
    lines.append('Never created: a Startup-folder shortcut (Start with Windows stays off), a Run value.')
    return '\n'.join(lines)


if __name__ == '__main__':
    p = plan()
    print(json.dumps(p, indent=2) if '--json' in sys.argv else render(p))
