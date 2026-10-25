"""The PreToolUse guard's rules on sample commands: no network, git, gh or open ports."""
import io
import json
import unittest
from unittest.mock import patch

from hooks import guard as g


def call(command, tool='PowerShell', **extra):
    return {'tool_name': tool, 'tool_input': {'command': command}, 'cwd': 'C:/repo', **extra}


def blocked(command, **extra):
    with patch.object(g, 'port_busy', lambda port: False), \
         patch.object(g, 'gh_json', lambda args, cwd: None), \
         patch.object(g, 'current_branch', lambda cwd: 'feature-x'):
        return g.evaluate(call(command, **extra))


class Flash(unittest.TestCase):
    def test_flash_needs_oncall(self):
        self.assertIn('On-call', blocked('python scripts/flash/foxglove_flash.py --firmware d --half both --yes'))
        self.assertIn('On-call', blocked('powershell -File flash-half.ps1 -Half left -Firmware x'))

    def test_dry_run_and_help_are_fine(self):
        self.assertIsNone(blocked('python scripts/flash/foxglove_flash.py --firmware d --dry-run'))
        self.assertIsNone(blocked('python scripts/flash/foxglove_flash.py --help'))

    def test_oncall_marker(self):
        self.assertIsNone(blocked("$env:FOXGLOVE_ROLE='oncall'; python scripts/flash/foxglove_flash.py --half left --yes"))
        self.assertIsNone(blocked('FOXGLOVE_ROLE=oncall python scripts/flash/foxglove_flash.py --half left', tool='Bash'))
        self.assertIsNone(blocked('python scripts/flash/foxglove_flash.py --half left', agent_type='on-call'))

    def test_reading_the_flasher_is_fine(self):
        self.assertIsNone(blocked('git log -- scripts/flash/test_flash.py'))


class App(unittest.TestCase):
    def test_launches_blocked(self):
        for cmd in [r'.\app\target\release\foxglove.exe',
                    r'& "C:\x\foxglove.exe" --tray',
                    'Start-Process foxglove',
                    r'Start-Process -FilePath C:\x\foxglove.exe',
                    r'python scripts/lab/pomodoro.py --app C:\x\foxglove.exe',
                    'cargo run --manifest-path app/Cargo.toml --release']:
            self.assertIn('On-call', blocked(cmd) or '', cmd)

    def test_not_launches(self):
        for cmd in ['Stop-Process -Name foxglove', 'Get-Process foxglove', r'Get-FileHash C:\x\foxglove.exe',
                    'cargo run -p foxglove-lab -- usb', 'python scripts/flash/foxglove_flash.py --dry-run']:
            reason = blocked(cmd)
            self.assertTrue(reason is None or 'Foxglove launches' not in reason, cmd)

    def test_ok_marker(self):
        self.assertIsNone(blocked(r'$env:FOXGLOVE_APP_OK=1; python scripts/lab/pomodoro.py --app C:\x\foxglove.exe'))


class Port(unittest.TestCase):
    def check(self, cmd, busy_ports):
        return g.check_port({}, cmd, busy=lambda p: p in busy_ports)

    def test_busy_port_blocked(self):
        self.assertIn('8765', self.check('python -m http.server 8765 --bind 127.0.0.1', {8765}))
        self.assertIn('9000', self.check('python scripts/lab/obs_mock.py --listen 127.0.0.1:9000 server', {9000}))

    def test_free_port_or_client_fine(self):
        self.assertIsNone(self.check('python -m http.server 8765', set()))
        self.assertIsNone(self.check('foxglove-plugin-broadcast-status --port 4455', {4455}))
        self.assertIsNone(self.check('FOXGLOVE_ALLOW_PORT=1 python -m http.server 8765', {8765}))


class Base(unittest.TestCase):
    def test_names(self):
        self.assertEqual(g.expected_base('backport/0.1-a'), 'release')
        self.assertEqual(g.expected_base('ble-status-0.1'), 'release')
        self.assertEqual(g.expected_base('docs/release-notes-0.1-backport'), 'release')
        self.assertEqual(g.expected_base('release/0.1-tag-run'), 'release')
        self.assertEqual(g.expected_base('forward/0.1-release-ci'), 'main')
        self.assertEqual(g.expected_base('ble-status-main'), 'main')
        self.assertIsNone(g.expected_base('docs/release-notes-0.1'))
        self.assertIsNone(g.expected_base('lab/art-keys'))

    def test_pairs_seen_in_the_repo(self):
        for head, base in [('backport/0.1-a', 'release/0.1'), ('replug-redraw-0.1', 'release/0.1'),
                           ('forward/0.1-msi-same-version', 'main'), ('docs/release-notes-0.1', 'main'),
                           ('lab/art-keys', 'lab/resilience'), ('test/release-checks-ci', 'main')]:
            self.assertIsNone(g.base_problem(head, base), (head, base))

    def test_wrong_targets(self):
        self.assertIsNotNone(g.base_problem('ble-status-0.1', 'main'))
        self.assertIsNotNone(g.base_problem('ble-status-main', 'release/0.1'))
        self.assertIsNotNone(g.base_problem('lab/new-suite', 'release/0.1'))

    def test_merge_checks_the_pr(self):
        pr = {'headRefName': 'single-instance-0.1', 'baseRefName': 'main'}
        reason = g.check_base({'cwd': '.'}, 'gh pr merge 351 --merge', gh=lambda a, c: pr)
        self.assertIn('release/0.1', reason)
        pr = {'headRefName': 'single-instance-0.1', 'baseRefName': 'release/0.1'}
        self.assertIsNone(g.check_base({'cwd': '.'}, 'gh pr merge 351 --merge', gh=lambda a, c: pr))

    def test_create_uses_current_branch(self):
        reason = g.check_base({'cwd': '.'}, 'gh pr create --title x --body y', branch=lambda c: 'fix-0.1')
        self.assertIn('release/0.1', reason)
        self.assertIsNone(g.check_base({'cwd': '.'}, 'gh pr create --base release/0.1 --title x', branch=lambda c: 'fix-0.1'))

    def test_direct_pushes(self):
        self.assertIsNotNone(blocked('git push origin main'))
        self.assertIsNotNone(blocked('git push origin HEAD:release/0.1'))
        self.assertIsNone(blocked('git push -u origin meta/hooks'))
        self.assertIsNone(blocked('git push origin release/0.1-tag-run'))


class Worktree(unittest.TestCase):
    ROOT = 'C:/kb/worktrees'

    def check(self, cmd, cwd='C:/kb/foxglove'):
        return g.check_worktree({'cwd': cwd}, cmd, root=lambda base: self.ROOT)

    def test_elsewhere_blocked(self):
        self.assertIn('worktrees', self.check(r'git worktree add -b x C:\Temp\scratchpad\x origin/main'))
        self.assertIn('worktrees', self.check('git worktree add .claude/worktrees/x main'))
        self.assertIn('worktrees', self.check('git -C C:/kb/foxglove worktree add ../fox-x main'))

    def test_in_the_folder_fine(self):
        self.assertIsNone(self.check('git worktree add -b meta/x ../worktrees/meta-x origin/main'))
        self.assertIsNone(self.check(r'git worktree add C:\kb\worktrees\y main'))
        self.assertIsNone(self.check('git -C C:/kb/foxglove worktree add ../worktrees/z main', cwd='C:/elsewhere'))
        self.assertIsNone(self.check('git worktree remove C:/Temp/x'))


class Cargo(unittest.TestCase):
    def test_raw_builds_blocked(self):
        for cmd in ['cargo build --release', 'cd app; cargo test', 'cargo clippy --workspace', 'cargo run -p foxglove-lab -- usb']:
            self.assertIn('build-slot.ps1', blocked(cmd) or '', cmd)

    def test_slotted_or_light_fine(self):
        for cmd in ['powershell -File scripts/build-slot.ps1 cargo build --release',
                    'powershell -ExecutionPolicy Bypass -File scripts/check.ps1',
                    'cargo fmt --all', 'cargo metadata --format-version 1', 'cargo --version', 'cargo tree -p digitalis']:
            self.assertIsNone(blocked(cmd), cmd)


class Main(unittest.TestCase):
    def run_main(self, payload):
        with patch('sys.stdin', io.StringIO(payload)), patch('sys.stderr', io.StringIO()) as err:
            return g.main(), err.getvalue()

    def test_block_exit_2(self):
        code, err = self.run_main(json.dumps(call('powershell -File flash-half.ps1 -Half left')))
        self.assertEqual(code, 2)
        self.assertIn('On-call', err)

    def test_other_tools_and_bad_input_pass(self):
        self.assertEqual(self.run_main(json.dumps({'tool_name': 'Read', 'tool_input': {}}))[0], 0)
        self.assertEqual(self.run_main('not json')[0], 0)


if __name__ == '__main__':
    unittest.main()
