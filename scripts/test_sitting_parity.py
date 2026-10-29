"""Sitting parity (#618 theme C, #617): every step of the next sitting's script maps to an agent
check (a local API method, a CLI or foxglove-lab command, a HIL case, a simulator test, a script
or a log line) or is tagged needs_eyes with a reason. The table is tests/sitting-parity.toml.

Fails when a step of docs/release-notes/0.2.md "Known untested items", a #90 row (the table's
[queue] snapshot) or a step of the resumed sitting's re-cut (docs/test-lab/sitting-0.2-h-i-r.md,
batches H, I, R, Z) has no entry, and when an entry names something the code doesn't have. In the
re-cut, a user step (`H1`) must say why it needs eyes or hands, and an agent step (`HA1`) must
name its checks and nothing for the user.

    python -m unittest scripts/test_sitting_parity.py
    python scripts/test_sitting_parity.py --list     (the needs-eyes steps, for a PR body)
"""
import re
import sys
import tomllib
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
TABLE = REPO / 'tests' / 'sitting-parity.toml'
NOTES = REPO / 'docs' / 'release-notes' / '0.2.md'
RECUT = REPO / 'docs' / 'test-lab' / 'sitting-0.2-h-i-r.md'
CHECKS = ('api', 'cli', 'lab', 'hil', 'sim', 'script', 'log')
KEYS = {'match', 'row', 'batch', 'needs_eyes', 'note', *CHECKS}
WHERE = ('match', 'row', 'batch')


def norm(text):
    return ' '.join(text.split())


def sitting_steps(notes):
    """The checklist items under "## Known untested items", each as one line of text."""
    section = notes.split('## Known untested items', 1)[1]
    section = re.split(r'\n## |\n\[#\d+\]:', section, maxsplit=1)[0]
    items, current = [], None
    for line in section.splitlines():
        if re.match(r'- \[[ xX]\] ', line):
            current = [line[6:]]
            items.append(current)
        elif current is not None and line.startswith('  ') and line.strip():
            current.append(line.strip())
        else:
            current = None
    return [norm(' '.join(i)) for i in items]


def recut_steps(text):
    """The re-cut's steps: {id: text} from its "- `H1` …" lines (ids like H1, HA1, RA2, ZA1)."""
    return {m[1]: m[2] for m in re.finditer(r'^- `([A-Z]A?\d+)` (.+)$', text, re.M)}


def is_agent(step_id):
    return bool(re.fullmatch(r'[A-Z]A\d+', step_id))


def name_of(s):
    return s.get('match') or (f'#90 row {s["row"]}' if 'row' in s else f'sitting step {s.get("batch")}')


def api_methods():
    text = (REPO / 'crates/plugin-api/src/methods.rs').read_text(encoding='utf-8')
    return set(re.findall(r'name: "([a-z_.]+)",\s*kind: Kind::Call', text))


def cli_commands():
    text = (REPO / 'crates/foxglove-cli/src/main.rs').read_text(encoding='utf-8')
    return set(re.findall(r'^\s+"([a-z]+)" =>', text, re.M))


def lab_commands():
    text = (REPO / 'crates/foxglove-lab/src/main.rs').read_text(encoding='utf-8')
    body = text.split('enum Command {', 1)[1].split('\n}', 1)[0]
    names = re.findall(r'^    ([A-Z][A-Za-z]+)\b', body, re.M)
    return {re.sub(r'(?<!^)([A-Z])', r'-\1', n).lower() for n in names}


def hil_cases():
    cases = set()
    for f in (REPO / 'tests/hil').glob('*.toml'):
        for c in tomllib.loads(f.read_text(encoding='utf-8')).get('cases', []):
            cases.add(f'{f.stem}/{c["name"]}')
    return cases


def log_lines():
    """Every source line with a `"Foxglove` string: where the app's log lines are written."""
    lines = []
    for root in ('app/src', 'crates'):
        for f in (REPO / root).rglob('*.rs'):
            lines += [l for l in f.read_text(encoding='utf-8').splitlines() if '"Foxglove' in l]
    return lines


def load():
    return tomllib.loads(TABLE.read_text(encoding='utf-8'))


class SittingParity(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.table = load()
        cls.steps = cls.table['step']
        cls.sitting = sitting_steps(NOTES.read_text(encoding='utf-8'))
        cls.recut = recut_steps(RECUT.read_text(encoding='utf-8'))

    def test_the_script_is_found(self):
        self.assertGreater(len(self.sitting), 40, 'the "Known untested items" checklist')

    def test_every_sitting_step_has_exactly_one_entry(self):
        for item in self.sitting:
            hits = [s['match'] for s in self.steps if 'match' in s and norm(s['match']) in item]
            self.assertEqual(len(hits), 1, f'{len(hits)} entries for: {item[:100]}')

    def test_every_entry_matches_one_sitting_step(self):
        for s in (s for s in self.steps if 'match' in s):
            hits = [i for i in self.sitting if norm(s['match']) in i]
            self.assertEqual(len(hits), 1, f'{s["match"]!r} matches {len(hits)} steps (stale?)')

    def test_every_queue_row_has_one_entry(self):
        rows = [s['row'] for s in self.steps if 'row' in s]
        self.assertEqual(sorted(rows), sorted(self.table['queue']), '#90 rows against entries')

    def test_the_recut_is_found(self):
        ids = set(self.recut)
        self.assertLessEqual({'H1', 'H2', 'I2', 'R1', 'HA1', 'IA1', 'RA1', 'ZA1'}, ids)
        self.assertGreaterEqual(len(ids), 20, 'the re-cut sitting steps')

    def test_every_recut_step_has_exactly_one_entry(self):
        for sid in self.recut:
            hits = [s for s in self.steps if s.get('batch') == sid]
            self.assertEqual(len(hits), 1, f'{len(hits)} entries for sitting step {sid}')
        for s in (s for s in self.steps if 'batch' in s):
            self.assertIn(s['batch'], self.recut, f'sitting step {s["batch"]}: not in {RECUT.name} (stale?)')

    def test_user_steps_need_eyes_and_agent_steps_need_none(self):
        for s in (s for s in self.steps if 'batch' in s):
            checks = [c for c in CHECKS if s.get(c)]
            if is_agent(s['batch']):
                self.assertTrue(checks, f'agent step {s["batch"]}: names no check')
                self.assertNotIn('needs_eyes', s, f'agent step {s["batch"]}: nothing for the user')
            else:
                self.assertGreater(len(s.get('needs_eyes', '').strip()), 10,
                                   f'user step {s["batch"]}: why only eyes or hands can do it')

    def test_the_recut_counts_its_user_steps(self):
        """The re-cut states its user-step count; it must be the steps it lists (fallbacks aside)."""
        text = RECUT.read_text(encoding='utf-8')
        said = int(re.search(r'\*\*User steps: (\d+), was \d+\*\*', text)[1])
        fallback = {'I3', 'I4'}
        users = [i for i in self.recut if not is_agent(i) and i not in fallback]
        self.assertEqual(len(users), said, users)

    def test_every_entry_is_mapped(self):
        for s in self.steps:
            name = name_of(s)
            self.assertLessEqual(set(s), KEYS, f'unknown keys in {name}')
            self.assertEqual(sum(k in s for k in WHERE), 1, f'{name}: one of match, row or batch')
            checks = [c for c in CHECKS if s.get(c)]
            eyes = s.get('needs_eyes', '').strip()
            self.assertTrue(checks or eyes, f'{name}: no agent check and no needs_eyes reason')
            if 'needs_eyes' in s:
                self.assertGreater(len(eyes), 10, f'{name}: needs_eyes needs a reason')

    def test_every_named_check_exists(self):
        have = {
            'api': api_methods(), 'cli': cli_commands(), 'lab': lab_commands(),
            'hil': hil_cases(),
            'sim': {p.name for p in (REPO / 'tests/zmk-sim').iterdir() if p.is_dir()},
        }
        logs = log_lines()
        for s in self.steps:
            name = name_of(s)
            for kind, known in have.items():
                for x in s.get(kind, []):
                    self.assertIn(x, known, f'{name}: no {kind} {x!r}')
            for x in s.get('script', []):
                self.assertTrue((REPO / x).is_file(), f'{name}: no script {x}')
            for x in s.get('log', []):
                self.assertTrue(any(x in l for l in logs), f'{name}: no log line with {x!r}')

    def test_the_617_controls_are_mapped(self):
        """The case-zone and sync controls this table was made for have their calls and logs."""
        case = next(s for s in self.steps if s.get('match', '').startswith('Each zone changes'))
        self.assertLessEqual({'pc_lighting.set_zone', 'pc_lighting.list'}, set(case['api']))
        self.assertIn('zone colours of', case['log'])
        sync = [s for s in self.steps if 'lighting.set_sync' in s.get('api', [])]
        self.assertTrue(sync and all('lighting sync (right follows left)' in s['log'] for s in sync))


def needs_eyes():
    return [(name_of(s), s['needs_eyes']) for s in load()['step'] if s.get('needs_eyes')]


if __name__ == '__main__':
    if '--list' in sys.argv:
        sys.stdout.reconfigure(encoding='utf-8')
        for name, why in needs_eyes():
            print(f'- {name}: {why}')
    else:
        unittest.main()
