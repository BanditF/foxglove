"""scripts/dev-identity.ps1 (#179): plan, apply and undo, with fake operations only.

Every test passes -OpsScript with fakes that record each operation's name in a log file, and a
temp state file, so no certificate store, package or exe is touched. The plan must record
nothing at all; undo must reverse exactly what apply recorded, newest first.
"""
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / 'scripts' / 'dev-identity.ps1'
OPS = ['CheckExe', 'CreateCert', 'TrustCert', 'SignPackage', 'Register', 'Verify',
       'Unregister', 'Unstage', 'Untrust', 'RemoveCert', 'FindCerts', 'Confirm']

FAKES = '''
foreach ($n in @(%s)) {
    $Ops[$n] = [scriptblock]::Create(@"
param(`$s)
Add-Content -Path `$env:FAKE_LOG -Value ('$n' + `$(if (`$s -and `$s.thumbprint) { ':' + `$s.thumbprint }))
if (`$env:FAKE_FAIL -eq '$n') { throw 'fake failure' }
if ('$n' -eq 'CreateCert') { `$s.thumbprint = 'FAKE1' }
if ('$n' -eq 'FindCerts') { @('OLD1', 'OLD2') }
if ('$n' -eq 'Confirm') { `$env:FAKE_CONFIRM -eq '1' }
"@)
}
''' % ', '.join("'%s'" % n for n in OPS)


@unittest.skipUnless(shutil.which('powershell'), 'no Windows PowerShell')
class DevIdentity(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.log = self.tmp / 'ops.log'
        self.state = self.tmp / 'state.json'
        self.ops = self.tmp / 'fake-ops.ps1'
        self.ops.write_text(FAKES, encoding='utf-8')

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_script(self, *args, fail=None, confirm=False):
        env = dict(os.environ, FAKE_LOG=str(self.log), FAKE_FAIL=fail or '',
                   FAKE_CONFIRM='1' if confirm else '0')
        r = subprocess.run(
            ['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(SCRIPT), *args,
             '-ExeDir', str(self.tmp / 'release'), '-StateFile', str(self.state),
             '-OpsScript', str(self.ops)],
            capture_output=True, text=True, timeout=120, env=env)
        return r

    def calls(self):
        if not self.log.exists():
            return []
        return [l.strip() for l in self.log.read_text(encoding='utf-8').splitlines() if l.strip()]

    def test_plan_prints_every_step_with_its_undo_and_does_nothing(self):
        r = self.run_script()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        out = r.stdout
        for needle in ('Cert:\\CurrentUser\\My', 'key not exportable', 'Cert:\\CurrentUser\\TrustedPeople',
                       'Add-AppxPackage', '-ExternalLocation', 'Get-AppxPackage', 'Background light control'):
            self.assertIn(needle, out)
        self.assertEqual(out.count('undo:'), 5, out)
        self.assertEqual(self.calls(), [], 'the plan runs no operation')
        self.assertFalse(self.state.exists())

    def test_apply_then_undo_reverses_each_step(self):
        r = self.run_script('apply', '-Yes')
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.calls(), ['CheckExe', 'CreateCert', 'TrustCert:FAKE1', 'SignPackage:FAKE1',
                                        'Register:FAKE1', 'Verify:FAKE1'])
        state = json.loads(self.state.read_text(encoding='utf-8-sig'))
        self.assertEqual(state['done'], ['cert', 'trust', 'sign', 'register'])
        self.assertEqual(state['thumbprint'], 'FAKE1')
        self.log.unlink()
        r = self.run_script('undo')
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.calls(), ['Unregister:FAKE1', 'Unstage:FAKE1', 'Untrust:FAKE1', 'RemoveCert:FAKE1'])
        self.assertFalse(self.state.exists(), 'undo leaves no state')

    def test_a_failure_part_way_is_undone_from_where_it_stopped(self):
        r = self.run_script('apply', '-Yes', fail='Register')
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn('dev-identity.ps1 undo', r.stdout)
        state = json.loads(self.state.read_text(encoding='utf-8-sig'))
        self.assertEqual(state['done'], ['cert', 'trust', 'sign'])
        self.log.unlink()
        r = self.run_script('undo')
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.calls(), ['Unstage:FAKE1', 'Untrust:FAKE1', 'RemoveCert:FAKE1'])

    def test_undo_without_a_record_removes_any_dev_identity(self):
        r = self.run_script('undo')
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.calls(), ['Unregister', 'Unstage', 'FindCerts', 'Untrust:OLD1', 'RemoveCert:OLD1',
                                        'Untrust:OLD2', 'RemoveCert:OLD2'])

    def test_apply_asks_first_and_stops_on_no(self):
        r = self.run_script('apply')
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertEqual(self.calls(), ['Confirm'])
        self.assertFalse(self.state.exists())

    def test_apply_refuses_over_an_applied_one(self):
        self.assertEqual(self.run_script('apply', '-Yes').returncode, 0)
        self.log.unlink()
        r = self.run_script('apply', '-Yes')
        self.assertNotEqual(r.returncode, 0)
        self.assertIn('run undo first', r.stdout + r.stderr)
        self.assertEqual(self.calls(), [])

    def test_a_failed_check_changes_nothing(self):
        r = self.run_script('apply', '-Yes', fail='CheckExe')
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.calls(), ['CheckExe'])


if __name__ == '__main__':
    unittest.main()
