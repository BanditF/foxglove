"""scripts/msi_tables.py against small MSI databases built in a temp folder (msi.dll, no WiX, no
install, no window). The fixtures mirror what WiX 3 compiles crates/hideout/wix/hideout.wxs to;
each test breaks one thing and expects exactly that check to fail.
"""
import copy
import sys
import tempfile
import unittest
from pathlib import Path

import msi_tables as m

WIN = sys.platform == 'win32'

SCHEMA = {
    'Property': ('`Property` CHAR(72) NOT NULL, `Value` LONGCHAR', 'Property'),
    'Upgrade': ('`UpgradeCode` CHAR(38) NOT NULL, `VersionMin` CHAR(20), `VersionMax` CHAR(20), `Language` CHAR(255), '
                '`Attributes` LONG NOT NULL, `Remove` CHAR(255), `ActionProperty` CHAR(72) NOT NULL',
                'UpgradeCode`, `VersionMin`, `VersionMax`, `Language`, `Attributes'),
    'LaunchCondition': ('`Condition` CHAR(255) NOT NULL, `Description` CHAR(255) NOT NULL', 'Condition'),
    'Registry': ('`Registry` CHAR(72) NOT NULL, `Root` SHORT NOT NULL, `Key` CHAR(255) NOT NULL, `Name` CHAR(255), '
                 '`Value` CHAR(255), `Component_` CHAR(72) NOT NULL', 'Registry'),
    'Shortcut': ('`Shortcut` CHAR(72) NOT NULL, `Directory_` CHAR(72) NOT NULL, `Name` CHAR(128) NOT NULL, '
                 '`Component_` CHAR(72) NOT NULL, `Target` CHAR(72) NOT NULL', 'Shortcut'),
    'File': ('`File` CHAR(72) NOT NULL, `Component_` CHAR(72) NOT NULL, `FileName` CHAR(255) NOT NULL', 'File'),
    'Component': ('`Component` CHAR(72) NOT NULL, `ComponentId` CHAR(38), `Directory_` CHAR(72) NOT NULL', 'Component'),
    'Directory': ('`Directory` CHAR(72) NOT NULL, `Directory_Parent` CHAR(72), `DefaultDir` CHAR(255) NOT NULL', 'Directory'),
    'CustomAction': ('`Action` CHAR(72) NOT NULL, `Type` SHORT NOT NULL, `Source` CHAR(72), `Target` CHAR(255)', 'Action'),
    'InstallExecuteSequence': ('`Action` CHAR(72) NOT NULL, `Condition` CHAR(255), `Sequence` SHORT', 'Action'),
    'RemoveFile': ('`FileKey` CHAR(72) NOT NULL, `Component_` CHAR(72) NOT NULL, `FileName` CHAR(255), '
                   '`DirProperty` CHAR(72) NOT NULL, `InstallMode` SHORT NOT NULL', 'FileKey'),
}

# The Hideout package as WiX 3 compiles hideout.wxs (values written without quotes).
GOOD = {
    'Property': [
        ('UpgradeCode', '{0A0A820B-B3AE-427A-91A7-E7FF56509254}'), ('ProductVersion', '0.1.0'),
        ('ProductName', 'Hideout')],
    'Upgrade': [
        ('{0A0A820B-B3AE-427A-91A7-E7FF56509254}', '', '0.1.0', '', 257, '', 'WIX_UPGRADE_DETECTED'),
        ('{0A0A820B-B3AE-427A-91A7-E7FF56509254}', '0.1.0', '', '', 2, '', 'WIX_DOWNGRADE_DETECTED')],
    'LaunchCondition': [('NOT WIX_DOWNGRADE_DETECTED', 'A newer version of Hideout is already installed.')],
    'Registry': [
        ('r1', 1, 'Software\\Hideout\\Installer', 'App', '[INSTALLFOLDER]', 'Application'),
        ('r2', 1, 'Software\\Hideout\\Installer', 'Shortcut', '#1', 'StartMenu')],
    'Shortcut': [('StartMenuShortcut', 'ProgramMenuFolder', 'Hideout', 'StartMenu', '[INSTALLFOLDER]hideout.exe')],
    'File': [('HideoutExe', 'Application', 'hideout.exe'), ('LicenseHideout', 'Application', 'LICENSE-hideout')],
    'Component': [('Application', '{G1}', 'INSTALLFOLDER'), ('StartMenu', '{G2}', 'ProgramMenuFolder')],
    'Directory': [
        ('TARGETDIR', '', 'SourceDir'), ('LocalAppDataFolder', 'TARGETDIR', '.'), ('UserPrograms', 'LocalAppDataFolder', 'Programs'),
        ('INSTALLFOLDER', 'UserPrograms', 'Hideout'), ('ProgramMenuFolder', 'TARGETDIR', '.')],
    'CustomAction': [('RemoveAutostart', 3154 + 0, 'HideoutExe', '--autostart-remove')],
    'InstallExecuteSequence': [
        ('RemoveExistingProducts', '', 1500), ('RemoveFiles', '', 3500),
        ('RemoveAutostart', 'REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE', 3490)],
    'RemoveFile': [('RemoveInstallFolder', 'Application', '', 'INSTALLFOLDER', 2),
                   ('RemoveUserPrograms', 'Application', '', 'UserPrograms', 2)],
}
# exe from the installed file (2+16) + deferred (1024) + ignore return (64); impersonated: no 2048 bit.
GOOD['CustomAction'] = [('RemoveAutostart', 2 + 16 + 64 + 1024, 'HideoutExe', '--autostart-remove')]


def build(path, tables):
    with m.Database(path, 'create') as db:
        for name, rows in tables.items():
            cols, key = SCHEMA[name]
            db.execute(f'CREATE TABLE `{name}` ({cols} PRIMARY KEY `{key}`)')
            names = [c.split('`')[1] for c in cols.split(', ')]
            for row in rows:
                vals = ', '.join(str(v) if isinstance(v, int) else ("'%s'" % v if v != '' else 'NULL') for v in row)
                db.execute('INSERT INTO `%s` (%s) VALUES (%s)' % (
                    name, ', '.join(f'`{n}`' for n in names), vals))
        db.commit()


@unittest.skipUnless(WIN, 'msi.dll is Windows only')
class MsiTables(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.dir = Path(cls.tmp.name)
        cls.n = 0

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def read(self, tables):
        type(self).n += 1
        path = self.dir / f'fixture{self.n}.msi'
        build(path, tables)
        return m.read_package(path)

    def broken(self, edit, product='hideout'):
        t = copy.deepcopy(GOOD)
        edit(t)
        return m.check(self.read(t), product)

    def assertFails(self, edit, word):
        bad = self.broken(edit)
        self.assertTrue(any(word in b for b in bad), f'wanted a failure about {word!r}, got {bad}')

    def test_good_package_passes_and_is_read_only(self):
        pkg = self.read(GOOD)
        self.assertEqual(m.check(pkg, 'hideout'), [])
        path = self.dir / 'fixture1.msi'
        before = path.read_bytes()
        m.read_package(path)
        self.assertEqual(before, path.read_bytes(), 'reading must not change the MSI')

    def test_missing_file_is_an_error_not_a_pass(self):
        self.assertEqual(m.main([str(self.dir / 'nope.msi'), '--product', 'hideout']), 2)

    def test_upgrade_code_and_version(self):
        self.assertFails(lambda t: t['Property'].__setitem__(0, ('UpgradeCode', '{40F2C6EE-F034-4FE5-91BC-65F1E74F7D24}')), 'UpgradeCode')
        self.assertFails(lambda t: t['Property'].__setitem__(1, ('ProductVersion', '1.0')), 'ProductVersion')

    def test_upgrade_rows(self):
        self.assertFails(lambda t: t['Upgrade'].pop(0), 'replaces the same or an older')
        self.assertFails(lambda t: t['Upgrade'].pop(1), 'NEWER')
        self.assertFails(lambda t: t['LaunchCondition'].clear(), 'downgrade is not blocked')
        self.assertFails(lambda t: t['InstallExecuteSequence'].pop(0), 'RemoveExistingProducts')

    def test_per_user_scope(self):
        self.assertFails(lambda t: t['Property'].append(('ALLUSERS', '1')), 'per-user')
        self.assertFails(lambda t: t['Registry'].__setitem__(0, ('r1', 2, 'Software\\Hideout\\Installer', 'App', 'x', 'Application')), 'HKCU')
        def to_program_files(t):
            t['Directory'][1:3] = [('ProgramFilesFolder', 'TARGETDIR', '.'), ('UserPrograms', 'ProgramFilesFolder', 'Programs')]
        self.assertFails(to_program_files, 'not the user')

    def test_no_autostart(self):
        self.assertFails(lambda t: t['Registry'].append(('r3', 1, 'Software\\Microsoft\\Windows\\CurrentVersion\\Run', 'Hideout', 'x', 'Application')), 'Run key')
        def startup_shortcut(t):
            t['Directory'].append(('StartupFolder', 'TARGETDIR', '.'))
            t['Shortcut'].append(('S2', 'StartupFolder', 'Hideout', 'StartMenu', '[INSTALLFOLDER]hideout.exe'))
        self.assertFails(startup_shortcut, 'Startup folder')
        self.assertFails(lambda t: t['File'].append(('Lnk', 'Application', 'Hideout.lnk')), 'shortcut written as a file')

    def test_no_stray_files(self):
        self.assertFails(lambda t: t['File'].append(('X', 'Application', 'foxglove.exe')), 'unexpected file foxglove.exe')
        self.assertFails(lambda t: t['File'].pop(0), 'hideout.exe is not in the package')

    def test_uninstall_removes_what_install_added(self):
        self.assertFails(lambda t: t['RemoveFile'].pop(0), 'does not remove the folder')
        self.assertFails(lambda t: t['CustomAction'].clear(), 'custom action RemoveAutostart is missing')
        self.assertFails(lambda t: t['CustomAction'].__setitem__(0, ('RemoveAutostart', 2 + 16 + 1024, 'HideoutExe', '--autostart-remove')), 'ignore its exit code')
        self.assertFails(lambda t: t['CustomAction'].__setitem__(0, ('RemoveAutostart', 2 + 16 + 64 + 1024 + 2048, 'HideoutExe', '--autostart-remove')), 'as the system')
        self.assertFails(lambda t: t['CustomAction'].__setitem__(0, ('RemoveAutostart', 2 + 16 + 64, 'HideoutExe', '--autostart-remove')), 'not deferred')
        self.assertFails(lambda t: t['CustomAction'].__setitem__(0, ('RemoveAutostart', 2 + 16 + 64 + 1024, 'HideoutExe', '--other')), "want '--autostart-remove'")
        self.assertFails(lambda t: t['InstallExecuteSequence'].__setitem__(2, ('RemoveAutostart', 'REMOVE="ALL"', 3490)), 'not on an upgrade')
        self.assertFails(lambda t: t['InstallExecuteSequence'].__setitem__(2, ('RemoveAutostart', 'REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE', 3600)), 'after RemoveFiles')

    def test_product_specs_match_the_wxs(self):
        root = Path(__file__).resolve().parent.parent
        for product, wxs in (('hideout', root / 'crates/hideout/wix/hideout.wxs'), ('foxglove', root / 'app/wix/main.wxs')):
            text = wxs.read_text(encoding='utf-8')
            code = '{' + m.PRODUCTS[product]['upgrade_code'].strip('{}') + '}'
            self.assertIn(code.strip('{}'), text.upper(), f'{product}: UpgradeCode in the spec differs from the .wxs')
            self.assertIn(m.PRODUCTS[product]['autostart_removal'][1], text)
            self.assertIn(m.PRODUCTS[product]['registry_key'], text)


if __name__ == '__main__':
    unittest.main()
