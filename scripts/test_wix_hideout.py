"""The Hideout installer (H5.6), checked without WiX and without running anything:
crates/hideout/wix/hideout.wxs, scripts/hideout_installer_plan.py (the dry run) and
scripts/build-hideout-installer.ps1.
"""
import re
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

import hideout_installer_plan as plan_mod

ROOT = Path(__file__).resolve().parent.parent
WXS = ROOT / 'crates' / 'hideout' / 'wix' / 'hideout.wxs'
FOXGLOVE_WXS = ROOT / 'app' / 'wix' / 'main.wxs'
PS1 = ROOT / 'scripts' / 'build-hideout-installer.ps1'
NS = {'w': 'http://schemas.microsoft.com/wix/2006/wi'}
FOXGLOVE_UPGRADE = '40F2C6EE-F034-4FE5-91BC-65F1E74F7D24'
SEP = '\\'


class WixHideout(unittest.TestCase):
    def setUp(self):
        self.text = WXS.read_text(encoding='utf-8')
        self.root = ET.fromstring(self.text.replace('$(var.Version)', '0.1.0'))
        self.product = self.root.find('w:Product', NS)

    def test_new_product_with_its_own_upgrade_code(self):
        self.assertEqual(self.product.get('Name'), 'Hideout')
        code = self.product.get('UpgradeCode')
        self.assertRegex(code, r'^[0-9A-F]{8}(-[0-9A-F]{4}){3}-[0-9A-F]{12}$')
        self.assertNotEqual(code.upper(), FOXGLOVE_UPGRADE)
        self.assertNotIn(FOXGLOVE_UPGRADE, self.text.upper())
        self.assertEqual(code, '0A0A820B-B3AE-427A-91A7-E7FF56509254', 'pinned: changing it breaks upgrades')
        fox = FOXGLOVE_WXS.read_text(encoding='utf-8')
        self.assertIn(FOXGLOVE_UPGRADE, fox)
        guids = re.findall(r'Guid="([0-9A-F-]{36})"', self.text)
        for g in guids:
            self.assertNotIn(g, fox, 'component GUIDs must not be shared with Foxglove')

    def test_per_user_no_admin(self):
        package = self.product.find('w:Package', NS)
        self.assertEqual(package.get('InstallScope'), 'perUser')
        self.assertEqual(package.get('InstallPrivileges'), 'limited')
        roots = {r.get('Root') for r in self.root.iter('{%s}RegistryValue' % NS['w'])}
        self.assertEqual(roots, {'HKCU'})
        self.assertNotIn('ProgramFiles', self.text)
        self.assertIn('WixUI_Minimal', self.text)

    def test_downgrade_refused_and_unc_refused(self):
        up = self.product.find('w:MajorUpgrade', NS)
        self.assertTrue(up.get('DowngradeErrorMessage'))
        conds = ''.join(c.text or '' for c in self.product.findall('w:Condition', NS))
        self.assertIn('LocalAppDataFolder', conds)

    def test_installer_record_for_the_hub(self):
        # find_installed / startup_win read HKCU\Software\Hideout\Installer value App (+ Exe, Version).
        values = {r.get('Name'): r for r in self.root.iter('{%s}RegistryValue' % NS['w'])
                  if r.get('Key') == SEP.join(['Software', 'Hideout', 'Installer'])}
        self.assertEqual(values['App'].get('Value'), '[INSTALLFOLDER]')
        self.assertEqual(values['App'].get('Type'), 'string')
        self.assertEqual(values['App'].get('KeyPath'), 'yes')
        self.assertEqual(values['Exe'].get('Value'), '[INSTALLFOLDER]hideout.exe')
        self.assertIn('Version', values)

    def test_install_folder_is_programs_hideout(self):
        p = plan_mod.plan()
        self.assertIn(SEP.join(['%LOCALAPPDATA%', 'Programs', 'Hideout', 'hideout.exe']), p['files'])
        self.assertIn('[INSTALLFOLDER]', p['registry'][0]['value'])

    def test_only_hideout_files_ship(self):
        p = plan_mod.plan()
        names = sorted(f.rsplit(SEP, 1)[1] for f in p['files'])
        self.assertEqual(names, ['LICENSE-hideout', 'hideout.exe'])
        # Foxglove etc. appear only in comments (naming what is never touched).
        body = re.sub(r'<!--.*?-->', '', self.text, flags=re.S)
        for word in ('firmware', '.uf2', 'art-src', 'foxglove.exe'):
            self.assertNotIn(word, body.lower(), word)
        self.assertNotIn('Foxglove', body)

    def test_license_is_mit_and_hideout_only(self):
        lic = (WXS.parent / 'LICENSE-hideout').read_text(encoding='utf-8')
        self.assertTrue(lic.startswith('MIT License'))
        self.assertNotIn('ZMK', lic)
        rtf = (WXS.parent / 'license.rtf').read_text(encoding='ascii')
        self.assertIn('MIT License', rtf)
        self.assertNotIn('GNU GENERAL PUBLIC', rtf)
        self.assertIn('license.rtf', self.text)

    def test_no_autostart_from_the_installer(self):
        body = re.sub(r'<!--.*?-->', '', self.text, flags=re.S)
        for bad in ('StartupFolder', 'Startup', 'CurrentVersion', r'\Run', 'Hideout.lnk', '--tray', 'StartupApproved'):
            self.assertNotIn(bad, body, bad)
        for s in self.root.iter('{%s}Shortcut' % NS['w']):
            self.assertEqual(s.get('Id'), 'StartMenuShortcut')
        p = plan_mod.plan()
        self.assertEqual(len(p['shortcuts']), 1)
        self.assertIn('Start Menu', p['shortcuts'][0]['path'])
        self.assertTrue(all('Startup' not in s['path'] for s in p['shortcuts']))

    def test_uninstall_runs_autostart_remove_as_user_not_on_upgrade(self):
        action = self.root.find(".//w:CustomAction[@Id='RemoveAutostart']", NS)
        self.assertEqual(action.get('ExeCommand'), '--autostart-remove')
        self.assertEqual(action.get('Impersonate'), 'yes')
        self.assertEqual(action.get('Execute'), 'deferred')
        self.assertEqual(action.get('Return'), 'ignore')
        custom = self.root.find('.//w:Custom', NS)
        self.assertEqual(custom.get('Before'), 'RemoveFiles')
        self.assertEqual(custom.text.strip(), 'REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE')

    def test_uninstall_removes_only_its_own(self):
        removes = [r.get('Directory') or 'INSTALLFOLDER' for r in self.root.iter('{%s}RemoveFolder' % NS['w'])]
        self.assertEqual(sorted(removes), ['INSTALLFOLDER', 'UserPrograms'])  # empty folders only
        for r in self.root.iter('{%s}RemoveFile' % NS['w']):
            self.fail('no wildcard file removal')
        self.assertNotIn('RemoveRegistry', self.text)


class DryRun(unittest.TestCase):
    def test_lists_what_it_would_write(self):
        out = plan_mod.render(plan_mod.plan())
        for line in ('Programs' + SEP + 'Hideout' + SEP + 'hideout.exe',
                     'HKCU' + SEP + 'Software' + SEP + 'Hideout' + SEP + 'Installer App (string) = [INSTALLFOLDER]',
                     'Hideout.lnk -> [INSTALLFOLDER]hideout.exe',
                     'HideoutExe --autostart-remove',
                     'Nothing is written'):
            self.assertIn(line, out)
        self.assertIn('0.1.0', out)

    def test_script_dry_run_and_build_never_install(self):
        ps = PS1.read_text(encoding='utf-8')
        self.assertIn('[switch]$DryRun', ps)
        self.assertIn('hideout_installer_plan.py', ps)
        code = re.sub(r'(?m)^\s*#.*$', '', ps)
        for bad in ('msiexec', 'Start-Process', 'Invoke-Item', 'signtool', 'git tag', 'gh release', 'Invoke-WebRequest'):
            self.assertNotIn(bad, code, bad)
        self.assertNotIn('& $exe', code)
        self.assertIn('hideout-installer', ps)  # output under the target dir (ignored by git)
        self.assertIn('hideout-x64.msi', ps)


if __name__ == '__main__':
    unittest.main()
