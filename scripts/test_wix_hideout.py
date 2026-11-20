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


class InstallerDialogs(unittest.TestCase):
    """FR6: the dialog sequence, checked on the WiX source (nothing is built or run)."""

    def setUp(self):
        self.text = WXS.read_text(encoding='utf-8')
        self.root = ET.fromstring(self.text.replace('$(var.Version)', '0.1.0').replace('$(sys.SOURCEFILEDIR)', ''))
        self.ui = self.root.find('.//w:UI', NS)
        self.dlg = {d.get('Id'): d for d in self.ui.findall('w:Dialog', NS)}
        self.props = {p.get('Id'): p.get('Value') for p in self.root.iter('{%s}Property' % NS['w'])}

    def controls(self, dialog):
        return {c.get('Id'): c for c in self.dlg[dialog].findall('w:Control', NS)}

    def publishes(self, control):
        return [(p.get('Event'), p.get('Value'), (p.text or '').strip()) for p in control.findall('w:Publish', NS)]

    def all_text(self, dialog):
        out = []
        for c in self.dlg[dialog].findall('w:Control', NS):
            t = c.find('w:Text', NS)
            out.append(c.get('Text') or (t.text if t is not None and t.text else ''))
        return ' | '.join(out)

    def test_sequence_is_install_page_then_progress_then_finish(self):
        shows = [(s.get('Dialog'), s.get('Before'), s.get('OnExit')) for s in self.ui.iter('{%s}Show' % NS['w'])]
        self.assertEqual(shows, [('HideoutInstallDlg', 'ProgressDlg', None), ('ExitDialog', None, 'success')])
        for gone in ('WixUI_Minimal', 'WelcomeEulaDlg'):
            self.assertNotIn(gone, self.text)

    def test_install_page_has_an_accept_box_that_gates_install(self):
        c = self.controls('HideoutInstallDlg')
        boxes = [x for x in c.values() if x.get('Type') == 'CheckBox']
        self.assertEqual([b.get('Id') for b in boxes], ['LicenseAccepted'])  # Hub: keep the accept box
        self.assertEqual(boxes[0].get('Property'), 'LicenseAccepted')
        self.assertNotIn('LicenseAccepted', self.props)  # not pre-ticked
        self.assertEqual(c['Install'].get('Default'), 'yes')
        self.assertEqual(self.publishes(c['Install']), [('EndDialog', 'Return', 'LicenseAccepted = "1"')])
        conds = {(x.get('Action'), x.text.strip()) for x in c['Install'].findall('w:Condition', NS)}
        self.assertIn(('disable', 'NOT LicenseAccepted'), conds)
        # still two pages: install page, then the finish page (the license page is optional reading)
        self.assertEqual(len(self.ui.findall('w:InstallUISequence/w:Show', NS)), 2)
        self.assertEqual(self.publishes(c['ReadLicense']), [('NewDialog', 'HideoutLicenseDlg', '1')])
        txt = self.all_text('HideoutInstallDlg')
        for s in ('It installs for you only, in your user folder.', 'MIT license', 'Read the license'):
            self.assertIn(s, txt)
        self.assertEqual(self.dlg['HideoutInstallDlg'].get('Title'), 'Install Hideout')

    def test_license_page_shows_the_rtf_and_goes_back(self):
        c = self.controls('HideoutLicenseDlg')
        self.assertEqual(c['License'].get('Type'), 'ScrollableText')
        self.assertEqual(c['License'].find('w:Text', NS).get('SourceFile'), 'license.rtf')
        self.assertEqual(self.publishes(c['Back']), [('NewDialog', 'HideoutInstallDlg', '1')])

    def test_finish_page_opens_hideout_ticked_and_says_apps_stay(self):
        self.assertEqual(self.props['WIXUI_EXITDIALOGOPTIONALCHECKBOX'], '1')  # ticked
        self.assertEqual(self.props['WIXUI_EXITDIALOGOPTIONALCHECKBOXTEXT'], 'Open Hideout now')
        c = self.controls('ExitDialog')
        self.assertEqual(c['OptionalCheckBox'].get('Text'), 'Open Hideout now')
        self.assertEqual(c['OptionalCheckBox'].get('Property'), 'WIXUI_EXITDIALOGOPTIONALCHECKBOX')
        cond = c['OptionalCheckBox'].find('w:Condition', NS)
        self.assertEqual((cond.get('Action'), cond.text.strip()), ('hide', 'Installed'))  # re-run: no dead box
        # the launch condition and the box's visibility agree
        self.assertIn('NOT Installed', self.publishes(c['Finish'])[0][2])
        txt = self.all_text('ExitDialog')
        self.assertIn('Hideout is installed', txt)
        self.assertIn('Apps you already have stay as they are.', txt)
        self.assertIn('does not start when you sign in unless you turn that on', txt)
        pubs = self.publishes(c['Finish'])
        self.assertEqual(pubs[0][:2], ('DoAction', 'LaunchHideout'))
        self.assertIn('WIXUI_EXITDIALOGOPTIONALCHECKBOX = 1', pubs[0][2])
        self.assertEqual(pubs[1], ('EndDialog', 'Return', '1'))

    def test_open_now_starts_the_hub_as_the_user_with_no_arguments(self):
        ca = self.root.find(".//w:CustomAction[@Id='LaunchHideout']", NS)
        self.assertEqual(ca.get('DllEntry'), 'WixShellExec')
        self.assertEqual(ca.get('Impersonate'), 'yes')
        self.assertEqual(self.props['WixShellExecTarget'], '[#HideoutExe]')
        self.assertNotIn('WixShellExecCmdLine', self.props)  # never --tray or any argument
        body = re.sub(r'<!--.*?-->', '', self.text, flags=re.S)
        self.assertNotIn('--tray', body)
        # only the finish button launches it, never the install sequence
        self.assertEqual([c.get('Action') for c in self.root.iter('{%s}Custom' % NS['w'])], ['RemoveAutostart'])
        self.assertIn('WixUtilExtension', PS1.read_text(encoding='utf-8'))

    def test_no_run_value_or_startup_entry_anywhere(self):
        for r in self.root.iter('{%s}RegistryValue' % NS['w']):
            self.assertEqual(r.get('Key').split(SEP)[:2], ['Software', 'Hideout'], r.get('Key'))
        self.assertFalse(list(self.root.iter('{%s}RegistryKey' % NS['w'])))
        body = re.sub(r'<!--.*?-->', '', self.text, flags=re.S).lower()
        for bad in ('currentversion', SEP + 'run', 'runonce', 'startupfolder', 'startupapproved', 'hideout.lnk'):
            self.assertNotIn(bad, body, bad)
        self.assertEqual([k.get('Id') for k in self.root.iter('{%s}Shortcut' % NS['w'])], ['StartMenuShortcut'])


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
