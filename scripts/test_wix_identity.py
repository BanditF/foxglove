"""The MSI's sparse identity variant (#179), checked without WiX: app/wix/main.wxs.

Everything for identity sits between <?ifdef SparseIdentity?> and <?endif?>, so the plain MSI
is unchanged; the logos the package manifest names ship next to the exe; the uninstall step
runs as the user, deferred, and never on a major upgrade (which keeps the registration).
"""
import re
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WXS = ROOT / 'app' / 'wix' / 'main.wxs'
APPX = ROOT / 'app' / 'packaging' / 'identity' / 'AppxManifest.xml'
NS = {'w': 'http://schemas.microsoft.com/wix/2006/wi'}


def ifdef_blocks(text):
    return re.findall(r'<\?ifdef SparseIdentity\?>(.*?)<\?endif\?>', text, re.S)


class WixIdentity(unittest.TestCase):
    def setUp(self):
        self.text = WXS.read_text(encoding='utf-8')
        self.root = ET.fromstring(self.text)
        self.inside = '\n'.join(ifdef_blocks(self.text))

    def test_identity_parts_are_only_in_the_variant(self):
        outside = re.sub(r'<\?ifdef SparseIdentity\?>.*?<\?endif\?>', '', self.text, flags=re.S)
        for name in ('IdentityPackage', 'IdentityLogos', 'UnregisterIdentity', 'SparseIdentity)'):
            self.assertIn(name, self.inside, name)
            self.assertNotIn(name, outside, f'{name} outside the SparseIdentity variant')

    def test_every_logo_the_manifest_names_ships(self):
        appx = APPX.read_text(encoding='utf-8')
        logos = set(re.findall(r'Assets\\([A-Za-z0-9]+\.png)', appx))
        self.assertTrue(logos)
        assets = self.root.find(".//w:Directory[@Id='IdentityAssets']", NS)
        self.assertIsNotNone(assets)
        self.assertEqual(assets.get('Name'), 'Assets')
        shipped = {f.get('Name') for f in assets.iter('{%s}File' % NS['w'])}
        self.assertEqual(shipped, logos)
        refs = {r.get('Id') for r in self.root.iter('{%s}ComponentRef' % NS['w'])}
        self.assertTrue({'IdentityPackage', 'IdentityLogos'} <= refs)

    def test_the_uninstall_step_runs_as_the_user_and_skips_upgrades(self):
        action = self.root.find(".//w:CustomAction[@Id='UnregisterIdentity']", NS)
        self.assertEqual(action.get('ExeCommand'), '--unregister-identity')
        self.assertEqual(action.get('Impersonate'), 'yes', 'per user (#444), never elevated')
        self.assertEqual(action.get('Execute'), 'deferred')
        self.assertEqual(action.get('Return'), 'ignore', 'a failed unregister never blocks uninstall')
        custom = [c for c in self.root.iter('{%s}Custom' % NS['w']) if c.get('Action') == 'UnregisterIdentity']
        self.assertEqual(len(custom), 1)
        self.assertEqual(custom[0].text.strip(), 'REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE')
        self.assertEqual(custom[0].get('Before'), 'RemoveFiles')

    def test_the_msi_stays_per_user(self):
        package = self.root.find('.//w:Package', NS)
        self.assertEqual(package.get('InstallScope'), 'perUser')


if __name__ == '__main__':
    unittest.main()
