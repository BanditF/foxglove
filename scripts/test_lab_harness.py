"""labkit's harness fixes from the #618 exit-check run. Nothing here launches anything or makes input.

- UTF-8 out: a lab script printing "→" through labkit on a cp1252 stream (a piped Windows console)
  crashed with UnicodeEncodeError; labkit now writes UTF-8 whatever the stream was.
- The harness's own input: injections recorded by labkit (and foxglove-lab, same file format) are
  left out of the "user is working" gate, and the user's own input still blocks.
- Tool paths (#618 follow-up): release-test.ps1 passed `--lab target/release/foxglove-lab.exe` and
  overlay_tray.py crashed starting it (FileNotFoundError), so its last cases never ran. --lab,
  --app and the other tool flags are absolute now (relative ones from the repo root, whatever the
  cwd), a missing one stops the script with an error naming the flag, and the gate resolves its own.

    python -m unittest scripts/test_lab_harness.py
"""
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'
REPO = LAB.parents[1]


def labkit():
    os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
    sys.path.insert(0, str(LAB))
    try:
        import labkit as k
    finally:
        sys.path.pop(0)
    return k


class Utf8Out(unittest.TestCase):
    def test_a_check_prints_an_arrow_under_a_cp1252_stdout(self):
        # PYTHONIOENCODING=cp1252 gives the child the strict cp1252 stdout a piped console has.
        env = {**os.environ, 'PYTHONIOENCODING': 'cp1252'}
        env.setdefault('APPDATA', str(Path.home()))
        env.setdefault('USERNAME', 'test')
        code = 'import labkit; labkit.Checks()("arrow", True, "Foxglove: overlay window → restored")'
        r = subprocess.run([sys.executable, '-c', code], capture_output=True, env=env, cwd=LAB, timeout=60)
        self.assertEqual(r.returncode, 0, r.stderr.decode('utf-8', 'replace'))
        self.assertIn('PASS arrow: Foxglove: overlay window → restored', r.stdout.decode('utf-8'))

    def test_utf8_reconfigures_a_cp1252_stream(self):
        k = labkit()
        raw = io.BytesIO()
        stream = io.TextIOWrapper(raw, encoding='cp1252')
        print('a → b', file=k.utf8(stream), flush=True)
        self.assertEqual(raw.getvalue().decode('utf-8').strip(), 'a → b')

    def test_utf8_leaves_other_streams_alone(self):
        k = labkit()
        s = io.StringIO()
        self.assertIs(k.utf8(s), s)


class HarnessInput(unittest.TestCase):
    def setUp(self):
        self.k = labkit()
        self.dir = tempfile.TemporaryDirectory()
        self.path = Path(self.dir.name) / 'lab-injections.log'

    def tearDown(self):
        self.dir.cleanup()

    def test_input_just_after_an_injection_is_the_harness(self):
        # The user last typed at 1000; the lab clicked from 1100 to 1100.2; the PC saw input at 1101.
        self.k.record_injection(1100.0, 1100.2, 1000.0, 'tray click (lab hook)', self.path)
        idle, r = self.k.user_idle(now=1130.0, raw=1101.0, path=self.path)
        self.assertEqual(idle, 130.0)
        self.assertEqual(r[3], 'tray click (lab hook)')

    def test_the_users_own_input_still_blocks(self):
        self.k.record_injection(1100.0, 1100.2, 1000.0, 'tray click (lab hook)', self.path)
        # Input 2.5 s after the click is past the 2 s window: the user's.
        idle, r = self.k.user_idle(now=1130.0, raw=1102.7, path=self.path)
        self.assertAlmostEqual(idle, 27.3)
        self.assertIsNone(r)
        # Input before the injection (and before its 0.1 s slack) is the user's too.
        idle, r = self.k.user_idle(now=1130.0, raw=1099.5, path=self.path)
        self.assertAlmostEqual(idle, 30.5)
        self.assertIsNone(r)

    def test_back_to_back_injections_chain_to_the_users_last_input(self):
        # The second injection began while the PC's last input was the first's: the user's last
        # input recorded with it is the first's "before", so the chain ends at the user.
        self.k.record_injection(1100.0, 1100.2, 1000.0, 'position 11 tap', self.path)
        before, _ = self.k.user_last_input(1100.1, self.k.injections(self.path))
        self.k.record_injection(1105.0, 1105.1, before, 'position 12 tap', self.path)
        idle, r = self.k.user_idle(now=1200.0, raw=1105.5, path=self.path)
        self.assertEqual(idle, 200.0)

    def test_records_are_the_format_foxglove_lab_writes(self):
        # crates/foxglove-lab/src/injections.rs: "<start> <end> <user_last or -> <what>".
        self.path.write_text('100.000 100.250 40.500 position 11 tap\njunk\n1.000 2.000 - invoke 43\n',
                             encoding='utf-8')
        self.assertEqual(self.k.injections(self.path),
                         [(100.0, 100.25, 40.5, 'position 11 tap'), (1.0, 2.0, None, 'invoke 43')])
        # No user input known before an injection: idle since it began (the cautious reading).
        idle, _ = self.k.user_idle(now=10.0, raw=1.5, path=self.path)
        self.assertEqual(idle, 9.0)

    def test_injecting_records_the_block(self):
        with self.k.injecting('tray click (lab hook)', self.path):
            pass
        (start, end, _, what), = self.k.injections(self.path)
        self.assertLessEqual(start, end)
        self.assertEqual(what, 'tray click (lab hook)')


class ToolPaths(unittest.TestCase):
    def setUp(self):
        self.k = labkit()

    def test_a_relative_path_is_taken_from_the_repo_root_whatever_the_cwd(self):
        old = os.getcwd()
        os.chdir(tempfile.gettempdir())
        try:
            p = self.k.tool('scripts/lab/labkit.py')
        finally:
            os.chdir(old)
        self.assertTrue(p.is_absolute())
        self.assertEqual(p, REPO / 'scripts' / 'lab' / 'labkit.py')

    def test_an_absolute_path_stays_and_a_folder_needs_dir(self):
        self.assertEqual(self.k.tool(sys.executable), Path(os.path.normpath(sys.executable)))
        self.assertEqual(self.k.tool('scripts', '--target', 'dir'), REPO / 'scripts')
        with self.assertRaises(ValueError):
            self.k.tool('scripts', '--lab')  # a folder isn't the file asked for

    def test_a_missing_tool_names_the_flag_and_where_it_looked(self):
        with self.assertRaises(ValueError) as e:
            self.k.tool('target/release/no-such-lab.exe', '--lab')
        msg = str(e.exception)
        self.assertIn('--lab target/release/no-such-lab.exe', msg)
        self.assertIn(str(REPO / 'target' / 'release' / 'no-such-lab.exe'), msg)

    def test_target_is_absolute(self):
        self.assertTrue(self.k.TARGET.is_absolute())

    def test_a_relative_cargo_target_dir_is_from_the_repo_root(self):
        env = {**os.environ, 'CARGO_TARGET_DIR': 'some-target'}
        env.setdefault('APPDATA', str(Path.home()))
        r = subprocess.run([sys.executable, '-c', 'import labkit; print(labkit.TARGET)'], capture_output=True,
                           text=True, env=env, cwd=LAB, timeout=60)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(Path(r.stdout.strip()), REPO / 'some-target')

    def script(self, *args):
        env = {k: v for k, v in os.environ.items() if k != 'FOXGLOVE_APP_OK'}
        env.setdefault('APPDATA', str(Path.home()))
        env.setdefault('USERNAME', 'test')
        # cwd is not the repo root on purpose: relative paths must still mean the repo's.
        return subprocess.run([sys.executable, str(LAB / 'overlay_tray.py'), *args], capture_output=True,
                              text=True, env=env, cwd=tempfile.gettempdir(), timeout=60)

    def test_a_lab_script_refuses_a_missing_lab_before_anything_runs(self):
        r = self.script('--app', sys.executable, '--lab', 'target/release/no-such-lab.exe')
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('--lab target/release/no-such-lab.exe: no such file', r.stderr)
        self.assertNotIn('RESULT', r.stdout)

    def test_a_lab_script_refuses_a_missing_app(self):
        r = self.script('--app', 'C:/nowhere/foxglove.exe', '--lab', sys.executable)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('--app C:/nowhere/foxglove.exe', r.stderr)

    def test_a_relative_lab_that_exists_is_accepted(self):
        # Parsed and resolved; the script then stops at the On-call gate (nothing launches).
        r = self.script('--app', sys.executable, '--lab', 'scripts/lab/labkit.py')
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('RESULT BLOCKED', r.stdout)

    def test_every_lab_script_parses_its_tool_flags_through_labkit(self):
        for f in sorted(LAB.glob('*.py')):
            text = f.read_text(encoding='utf-8')
            for flag in ('--app', '--lab', '--cli', '--target', '--plugin'):
                for line in text.splitlines():
                    if f"add_argument('{flag}'" in line:
                        self.assertIn(f"tool_arg('{flag}'", line, f'{f.name}: {flag}')


class ReleaseGateScript(unittest.TestCase):
    """scripts/release-test.ps1 and its suites, read as text (the gate needs the keyboard)."""

    def setUp(self):
        self.text = (REPO / 'scripts' / 'release-test.ps1').read_text(encoding='utf-8')

    def test_tool_paths_are_resolved_from_the_repo_root(self):
        self.assertNotIn("$lab = 'target/", self.text)
        self.assertIn('$lab = Resolve-RepoPath', self.text)
        self.assertIn('$Firmware = Resolve-RepoPath', self.text)
        self.assertIn('$App = Resolve-RepoPath', self.text)

    def test_resilience_runs_before_the_app_stages_after_the_app_exits(self):
        resilience = self.text.index("Write-Host '== Suite resilience")
        suite = self.text.index('Write-Host "== Suite $Suite"')
        app_cases = self.text.index("Write-Host '== App lab cases")
        self.assertLess(resilience, suite)
        self.assertLess(suite, app_cases)
        self.assertIn('Wait-Process -Timeout', self.text)
        self.assertNotIn('Start-Sleep -Seconds 2', self.text)

    def toml(self, name):
        import tomllib
        return tomllib.loads((REPO / 'tests' / 'hil' / name).read_text(encoding='utf-8'))

    def test_the_activity_case_is_the_resilience_suites_first(self):
        case = self.toml('resilience.toml')['cases'][0]
        self.assertEqual(case['name'], 'right-follows-left-activity')
        # Its 1 s wake limit stays.
        self.assertIn({'expect_rgb': {'right_age_max_s': 2, 'right_on': True, 'within_ms': 1000}}, case['steps'])

    def test_the_idle_cpu_case_comes_first_and_settles_two_minutes(self):
        self.assertEqual(self.toml('release.toml')['include'][0], 'app-idle.toml')
        case = self.toml('app-idle.toml')['cases'][0]
        self.assertEqual(case['name'], 'app-idle-cpu')
        step = case['steps'][0]['idle_cpu']
        self.assertEqual((step['settle_s'], step['seconds'], step['max_cpu_ms']), (120, 20, 60))

if __name__ == '__main__':
    unittest.main()
