"""scripts/dev-identity.ps1 (#179): plan, apply and undo, with fake operations only.

Every test passes -OpsScript with fakes over a fake world: a temp folder of marker files for the
three certificate stores, the staged package's signer, the registered packages and elevation.
The probes read it, the changes write it and log their names, so no certificate store, package
or exe is touched. Apply must do only the missing steps (a rerun is a no-op), the machine store
must stop with the elevated command when not elevated, and undo must leave the world empty.
"""
import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / 'scripts' / 'dev-identity.ps1'
PUBLISHER = 'CN=Foxglove Dev, O=BanditF'
MY, USER_TP, MACHINE_TP = 'CurrentUser.My', 'CurrentUser.TrustedPeople', 'LocalMachine.TrustedPeople'
HUB = 'CFAE2CDA58F085868590E85504D25C61D2744D31'

FAKES = r'''
$W = $env:FAKE_WORLD
function Fake-Dir($store) { Join-Path $W ($store -replace '\\', '.') }
function Fake-Log($m) { Add-Content -Path $env:FAKE_LOG -Value $m }
function Fake-Fail($n) {
    if ($env:FAKE_FAIL -eq $n) { throw $(if ($env:FAKE_FAIL_MSG) { $env:FAKE_FAIL_MSG } else { 'fake failure' }) }
}
function Fake-Names($store) { @(Get-ChildItem (Fake-Dir $store) -ErrorAction SilentlyContinue | ForEach-Object Name) }
function Fake-Put($store, $t) { New-Item -ItemType Directory -Force (Fake-Dir $store) | Out-Null; Set-Content (Join-Path (Fake-Dir $store) $t) 'x' }

# Probes: read only, not logged.
$Ops.IsElevated = { Test-Path (Join-Path $W 'elevated') }
$Ops.FindCert = {
    param($hint)
    $all = Fake-Names 'CurrentUser\My'
    if ($hint -and $all -contains $hint) { $hint } elseif ($all.Count) { @($all | Sort-Object)[0] }
}
$Ops.InStore = { param($t, $store) [bool]$t -and (Test-Path (Join-Path (Fake-Dir $store) $t)) }
$Ops.SignedBy = { $f = Join-Path $W 'signer'; if (Test-Path $f) { (Get-Content $f -Raw).Trim() } }
$Ops.FindPackages = {
    @(Get-ChildItem (Join-Path $W 'packages') -ErrorAction SilentlyContinue | ForEach-Object {
        [pscustomobject]@{ PackageFullName = $_.Name; Publisher = (Get-Content $_.FullName -Raw).Trim(); InstallLocation = '' }
    })
}
$Ops.FindCerts = { @(foreach ($s in $Stores) { Fake-Names $s }) | Sort-Object -Unique }

# Changes: logged, can fail on FAKE_FAIL, and change the world.
$Ops.CheckExe = { Fake-Log 'CheckExe'; Fake-Fail 'CheckExe' }
$Ops.Confirm = { Fake-Log 'Confirm'; $env:FAKE_CONFIRM -eq '1' }
$Ops.CreateCert = { Fake-Log 'CreateCert'; Fake-Fail 'CreateCert'; Fake-Put 'CurrentUser\My' 'NEW1'; 'NEW1' }
$Ops.TrustCert = { param($t, $store) Fake-Log "TrustCert:$($t):$store"; Fake-Fail 'TrustCert'; Fake-Put $store $t }
$Ops.SignPackage = { param($t) Fake-Log "SignPackage:$t"; Fake-Fail 'SignPackage'; Set-Content (Join-Path $W 'signer') $t }
$Ops.Register = {
    Fake-Log 'Register'; Fake-Fail 'Register'
    New-Item -ItemType Directory -Force (Join-Path $W 'packages') | Out-Null
    Set-Content (Join-Path $W 'packages\BanditF.Foxglove_1.0.0.0_neutral__fake') $Publisher
}
$Ops.Unregister = { param($full) Fake-Log "Unregister:$full"; Remove-Item (Join-Path $W "packages\$full") }
$Ops.Unstage = { Fake-Log 'Unstage'; Remove-Item (Join-Path $W 'signer') -ErrorAction SilentlyContinue }
$Ops.Untrust = { param($t, $store) Fake-Log "Untrust:$($t):$store"; Remove-Item (Join-Path (Fake-Dir $store) $t) }
$Ops.RemoveCert = { param($t) Fake-Log "RemoveCert:$t"; Remove-Item (Join-Path (Fake-Dir 'CurrentUser\My') $t) }
'''


@unittest.skipUnless(shutil.which('powershell'), 'no Windows PowerShell')
class DevIdentity(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.world = self.tmp / 'world'
        self.world.mkdir()
        self.log = self.tmp / 'ops.log'
        self.state = self.tmp / 'state.json'
        self.ops = self.tmp / 'fake-ops.ps1'
        self.ops.write_text(FAKES, encoding='utf-8')

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    # The fake world.
    def put(self, store, thumb):
        (self.world / store).mkdir(exist_ok=True)
        (self.world / store / thumb).write_text('x', encoding='utf-8')

    def has(self, store, thumb):
        return (self.world / store / thumb).exists()

    def register(self, full='BanditF.Foxglove_1.0.0.0_neutral__byhand', publisher=PUBLISHER):
        (self.world / 'packages').mkdir(exist_ok=True)
        (self.world / 'packages' / full).write_text(publisher, encoding='utf-8')

    def packages(self):
        d = self.world / 'packages'
        return sorted(p.name for p in d.iterdir()) if d.exists() else []

    def certs_anywhere(self):
        return [(s, t.name) for s in (MY, USER_TP, MACHINE_TP) if (self.world / s).exists()
                for t in (self.world / s).iterdir()]

    def elevated(self):
        (self.world / 'elevated').write_text('x', encoding='utf-8')

    def hub_state(self):
        """The sitting's state after the Hub's manual steps: the script's cert, user and machine
        trust, the staged package signed with it, and a registration by hand."""
        self.put(MY, HUB)
        self.put(USER_TP, HUB)
        self.put(MACHINE_TP, HUB)
        (self.world / 'signer').write_text(HUB, encoding='utf-8')
        self.register()

    def pc_state(self):
        """The user's PC on 11/07, exactly: the sitting's apply recorded done = cert, trust, sign
        (old format); the user added CFAE2CDA... to LocalMachine/TrustedPeople in an admin
        PowerShell and registered BanditF.Foxglove 1.0.1.0 by hand, which the record doesn't know."""
        self.put(MY, HUB)
        self.put(USER_TP, HUB)
        self.put(MACHINE_TP, HUB)
        (self.world / 'signer').write_text(HUB, encoding='utf-8')
        self.register('BanditF.Foxglove_1.0.1.0_neutral__8wekyb3d8bbwe')
        self.state.write_text(json.dumps({'thumbprint': HUB, 'publisher': PUBLISHER,
                                          'done': ['cert', 'trust', 'sign']}), encoding='utf-8')

    def state_json(self):
        return json.loads(self.state.read_text(encoding='utf-8-sig'))

    def run_script(self, *args, fail=None, fail_msg=None, confirm=False):
        env = dict(os.environ, FAKE_LOG=str(self.log), FAKE_WORLD=str(self.world), FAKE_FAIL=fail or '',
                   FAKE_FAIL_MSG=fail_msg or '', FAKE_CONFIRM='1' if confirm else '0')
        return subprocess.run(
            ['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(SCRIPT), *args,
             '-ExeDir', str(self.tmp / 'release'), '-StateFile', str(self.state),
             '-OpsScript', str(self.ops)],
            capture_output=True, text=True, timeout=120, env=env)

    def calls(self):
        if not self.log.exists():
            return []
        out = [l.strip() for l in self.log.read_text(encoding='utf-8').splitlines() if l.strip()]
        self.log.unlink()
        return out

    def ok(self, r):
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    # Plan.
    def test_plan_prints_every_step_with_its_undo_and_changes_nothing(self):
        r = self.run_script()
        self.ok(r)
        for needle in ('Cert:\\CurrentUser\\My', 'key not exportable', 'Cert:\\CurrentUser\\TrustedPeople',
                       'Add-AppxPackage', '-ExternalLocation', 'Get-AppxPackage', 'Background light control',
                       'Trust store: Cert:\\CurrentUser\\TrustedPeople only', '-MachineTrust'):
            self.assertIn(needle, r.stdout)
        self.assertEqual(r.stdout.count('undo:'), 4, r.stdout)
        self.assertEqual(len(re.findall(r'\d\. \w+ +\[to do\]', r.stdout)), 4, r.stdout)
        self.assertEqual(self.calls(), [], 'the plan runs only probes')
        self.assertFalse(self.state.exists())

    def test_plan_with_machine_trust_names_the_machine_store_and_admin(self):
        r = self.run_script('-MachineTrust')
        self.ok(r)
        self.assertIn('Trust store: Cert:\\CurrentUser\\TrustedPeople and Cert:\\LocalMachine\\TrustedPeople', r.stdout)
        self.assertIn('needs an elevated PowerShell', r.stdout)
        self.assertEqual(r.stdout.count('undo:'), 5, r.stdout)
        self.assertEqual(self.calls(), [])

    def test_plan_shows_what_is_done(self):
        self.hub_state()
        r = self.run_script('-MachineTrust')
        self.ok(r)
        self.assertEqual(r.stdout.count('[done]'), 5, r.stdout)
        self.assertIn(HUB, r.stdout)
        self.assertIn('BanditF.Foxglove_1.0.0.0_neutral__byhand', r.stdout)

    # Apply.
    def test_apply_from_nothing_then_rerun_is_a_no_op(self):
        r = self.run_script('apply', '-Yes')
        self.ok(r)
        self.assertEqual(self.calls(), ['CheckExe', 'CreateCert', f'TrustCert:NEW1:CurrentUser\\TrustedPeople',
                                        'SignPackage:NEW1', 'Register'])
        self.assertIn('Applied.', r.stdout)
        self.assertIn('NEW1', self.state.read_text(encoding='utf-8-sig'))
        r = self.run_script('apply', '-Yes')
        self.ok(r)
        self.assertIn('Nothing to do', r.stdout)
        self.assertEqual(self.calls(), [])

    def test_machine_trust_not_elevated_prints_the_command_and_changes_nothing(self):
        r = self.run_script('apply', '-MachineTrust', '-Yes')
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn('Needs administrator', r.stdout)
        self.assertIn('Run as administrator', r.stdout)
        self.assertIn('Nothing was changed', r.stdout)
        self.assertIn(f'-File "{SCRIPT}" apply -MachineTrust', r.stdout)
        self.assertEqual(self.calls(), [])
        self.assertEqual(self.certs_anywhere(), [])
        self.assertFalse(self.state.exists())

    def test_machine_trust_elevated_trusts_both_stores(self):
        self.elevated()
        r = self.run_script('apply', '-MachineTrust', '-Yes')
        self.ok(r)
        self.assertEqual(self.calls(), ['CheckExe', 'CreateCert', 'TrustCert:NEW1:CurrentUser\\TrustedPeople',
                                        'TrustCert:NEW1:LocalMachine\\TrustedPeople', 'SignPackage:NEW1', 'Register'])

    def test_the_hubs_manual_steps_make_a_rerun_a_no_op(self):
        self.hub_state()
        for args in (('apply', '-MachineTrust'), ('apply',)):  # not elevated, no -Yes: nothing to ask
            r = self.run_script(*args)
            self.ok(r)
            self.assertIn('Nothing to do', r.stdout)
            self.assertEqual(self.calls(), [], args)

    def test_the_pcs_state_rerun_is_a_no_op_that_upgrades_the_record(self):
        self.pc_state()
        r = self.run_script('apply', '-MachineTrust')  # not elevated, no -Yes
        self.ok(r)
        self.assertIn('Nothing to do', r.stdout)
        self.assertIn('BanditF.Foxglove_1.0.1.0', r.stdout)
        self.assertEqual(self.calls(), [], 'no store or package change, no question')
        self.assertEqual(self.state_json()['done'], ['cert', 'trust', 'machine', 'sign', 'register'])
        self.assertEqual(self.state_json()['thumbprint'], HUB)
        before = self.state.read_bytes()
        self.ok(self.run_script('apply'))  # and again, without the switch: the record stays
        self.assertEqual(self.calls(), [])
        self.assertEqual(self.state.read_bytes(), before)

    def test_the_pcs_state_full_undo_removes_the_hand_made_pieces_too(self):
        self.pc_state()
        r = self.run_script('undo')  # not elevated: the machine copy needs admin
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn(HUB, r.stdout)
        self.assertEqual(self.calls(), [])
        self.elevated()
        r = self.run_script('undo')
        self.ok(r)
        self.assertEqual(self.calls(), [
            'Unregister:BanditF.Foxglove_1.0.1.0_neutral__8wekyb3d8bbwe', 'Unstage',
            f'Untrust:{HUB}:LocalMachine\\TrustedPeople', f'Untrust:{HUB}:CurrentUser\\TrustedPeople',
            f'RemoveCert:{HUB}'])
        self.assertEqual(self.certs_anywhere(), [])
        self.assertEqual(self.packages(), [])
        self.assertFalse(self.state.exists())

    def test_resume_after_0x800B0109_reuses_the_cert_and_does_only_what_is_missing(self):
        # The sitting: cert, user trust and sign done, registration refused.
        r = self.run_script('apply', '-Yes', fail='Register',
                            fail_msg='Deployment failed with HRESULT: 0x800B0109, A certificate chain processed, '
                                     'but terminated in a root certificate which is not trusted')
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn('0x800B0109', r.stdout)
        self.assertIn('machine-wide', r.stdout)
        self.assertIn('apply -MachineTrust', r.stdout)
        self.calls()
        r = self.run_script('apply', '-MachineTrust', '-Yes')  # not elevated yet
        self.assertEqual(r.returncode, 1)
        self.assertEqual(self.calls(), [])
        self.elevated()
        r = self.run_script('apply', '-MachineTrust', '-Yes')
        self.ok(r)
        self.assertEqual(self.calls(), ['CheckExe', 'TrustCert:NEW1:LocalMachine\\TrustedPeople', 'Register'])

    def test_an_existing_matching_cert_is_reused_never_duplicated(self):
        self.put(MY, HUB)
        self.put(USER_TP, HUB)
        r = self.run_script('apply', '-Yes')
        self.ok(r)
        self.assertEqual(self.calls(), ['CheckExe', f'SignPackage:{HUB}', 'Register'])
        self.assertEqual(sorted(t.name for t in (self.world / MY).iterdir()), [HUB])

    def test_another_failure_says_rerun_resumes(self):
        r = self.run_script('apply', '-Yes', fail='SignPackage')
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn('run apply again (it resumes)', r.stdout)
        self.assertNotIn("Windows doesn't trust", r.stdout)
        self.calls()
        r = self.run_script('apply', '-Yes')
        self.ok(r)
        self.assertEqual(self.calls(), ['CheckExe', 'SignPackage:NEW1', 'Register'])

    def test_apply_asks_first_and_stops_on_no(self):
        r = self.run_script('apply')
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(self.calls(), ['Confirm'])
        self.assertFalse(self.state.exists())

    def test_a_failed_check_changes_nothing(self):
        r = self.run_script('apply', '-Yes', fail='CheckExe')
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.calls(), ['CheckExe'])
        self.assertEqual(self.certs_anywhere(), [])

    def test_another_publishers_package_stops_apply(self):
        self.register('BanditF.Foxglove_1.0.0.0_neutral__other', 'CN=Foxglove Placeholder')
        r = self.run_script('apply', '-Yes')
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn('dev-identity.ps1 undo', r.stdout)
        self.assertEqual(self.calls(), [])

    # Undo.
    def test_full_undo_leaves_no_cert_and_no_package(self):
        self.hub_state()
        self.put(MY, 'OLD2')  # a second, older dev cert
        self.elevated()
        r = self.run_script('undo')
        self.ok(r)
        self.assertEqual(self.calls(), [
            'Unregister:BanditF.Foxglove_1.0.0.0_neutral__byhand', 'Unstage',
            f'Untrust:{HUB}:LocalMachine\\TrustedPeople', f'Untrust:{HUB}:CurrentUser\\TrustedPeople',
            f'RemoveCert:{HUB}', 'RemoveCert:OLD2'])
        self.assertEqual(self.certs_anywhere(), [])
        self.assertEqual(self.packages(), [])
        self.assertFalse((self.world / 'signer').exists())

    def test_undo_with_a_machine_copy_needs_admin_and_changes_nothing(self):
        self.hub_state()
        r = self.run_script('undo')
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn('Needs administrator', r.stdout)
        self.assertIn(f'-File "{SCRIPT}" undo', r.stdout)
        self.assertEqual(self.calls(), [])
        self.assertTrue(self.has(MACHINE_TP, HUB))
        self.assertEqual(len(self.packages()), 1)

    def test_undo_after_a_user_only_apply_needs_no_admin(self):
        self.ok(self.run_script('apply', '-Yes'))
        self.calls()
        r = self.run_script('undo')
        self.ok(r)
        self.assertEqual(self.calls(), ['Unregister:BanditF.Foxglove_1.0.0.0_neutral__fake', 'Unstage',
                                        'Untrust:NEW1:CurrentUser\\TrustedPeople', 'RemoveCert:NEW1'])
        self.assertEqual(self.certs_anywhere(), [])
        self.assertFalse(self.state.exists(), 'undo leaves no state')

    def test_undo_after_a_failure_part_way(self):
        self.run_script('apply', '-Yes', fail='Register')
        self.calls()
        r = self.run_script('undo')
        self.ok(r)
        self.assertEqual(self.calls(), ['Unstage', 'Untrust:NEW1:CurrentUser\\TrustedPeople', 'RemoveCert:NEW1'])
        self.assertEqual(self.certs_anywhere(), [])

    def test_undo_removes_a_package_registered_by_hand_without_a_state_file(self):
        self.register()
        r = self.run_script('undo')
        self.ok(r)
        self.assertEqual(self.calls(), ['Unregister:BanditF.Foxglove_1.0.0.0_neutral__byhand', 'Unstage'])
        self.assertEqual(self.packages(), [])

    def test_undo_twice_is_safe(self):
        self.hub_state()
        self.elevated()
        self.ok(self.run_script('undo'))
        self.calls()
        r = self.run_script('undo')
        self.ok(r)
        self.assertEqual(self.calls(), ['Unstage'])


if __name__ == '__main__':
    unittest.main()
