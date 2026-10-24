"""Cheap offline checks for the 48-key Corne keymap. No ZMK build required.
Usage: python scripts/keymap/keymap_lint.py config/eyelash_corne.keymap
This models momentary thumb layers, not ZMK timing or arbitrary behaviors.
"""
from dataclasses import dataclass
import itertools
from pathlib import Path
import re
import sys


def strip_comments(source):
    return re.sub(r'"(?:\\.|[^"\\])*"|/\*.*?\*/|//[^\n]*',
                  lambda m: m[0] if m[0].startswith('"') else ' ', source, flags=re.S)


def node(source, name):
    match = re.search(r'\b' + re.escape(name) + r'\s*\{', source)
    if not match:
        raise ValueError(f'Missing {name} node')
    start = match.end()
    depth = 1
    for i in range(start, len(source)):
        depth += (source[i] == '{') - (source[i] == '}')
        if depth == 0:
            return source[start:i]
    raise ValueError(f'Unclosed {name} node')


@dataclass
class Keymap:
    source: str
    layers: list
    conditions: list


def parse(source):
    source = strip_comments(source)
    body = node(source, 'keymap')
    layers = []
    for layer in re.findall(r'\w+\s*\{([^{}]*)\}', body, re.S):
        name = re.search(r'display-name\s*=\s*"([^"]+)"', layer)
        bindings = re.search(r'(?<![\w-])bindings\s*=\s*<(.*?)>;', layer, re.S)
        if not bindings:
            raise ValueError('Layer missing bindings')
        values = [' '.join(b.split()) for b in re.findall(r'&[^&]+', bindings[1])]
        layers.append((name[1] if name else str(len(layers)), values))
    if not layers:
        raise ValueError('No keymap layers found')
    conditions = []
    for rule in re.findall(r'\w+\s*\{([^{}]*)\}', node(source, 'conditional_layers'), re.S):
        before = re.search(r'if-layers\s*=\s*<([\d\s]+)>', rule)
        after = re.search(r'then-layer\s*=\s*<(\d+)>', rule)
        if not before or not after:
            raise ValueError('Malformed conditional layer rule')
        conditions.append((tuple(map(int, before[1].split())), int(after[1])))
    return Keymap(source, layers, conditions)


def check_binding_counts(km):
    return [f'{name}: expected 48 bindings, found {len(bindings)}'
            for name, bindings in km.layers if len(bindings) != 48]


def check_layer_references(km):
    refs = [int(n) for n in re.findall(r'&(?:mo|tog|to|lt|sl)\s+(\d+)\b', km.source)]
    refs += [n for before, after in km.conditions for n in (*before, after)]
    return [f'Layer reference {n} is outside 0..{len(km.layers)-1}'
            for n in sorted(set(refs)) if not 0 <= n < len(km.layers)]


def conditional(active, rules):
    active = set(active) - {after for _, after in rules}
    # The rules here depend on directly held layers, matching layersim.
    for before, after in rules:
        if set(before) <= active:
            active.add(after)
    return active


class Keyboard:
    def __init__(self, km, base):
        self.km, self.base, self.held = km, set(base), {}

    def active(self):
        return conditional(self.base | set(self.held.values()), self.km.conditions)

    def resolve(self, pos):
        for layer in sorted(self.active(), reverse=True):
            binding = self.km.layers[layer][1][pos]
            if binding != '&trans':
                return binding
        return '&none'

    def press(self, pos):
        binding = self.resolve(pos)
        if not binding.startswith('&mo '):
            raise ValueError(f'Thumb {pos} resolved to {binding}, expected &mo')
        self.held[pos] = int(binding.split()[1])

    def release(self, pos):
        del self.held[pos]  # Release the binding captured on press, even on ADMIN.


def check_thumb_replay(km):
    failures = []
    try:
        game = next(i for i, (name, _) in enumerate(km.layers) if name == 'GAME')
        for base in ({0}, {0, game}):
            # Derive the intended held layers from each mode BEFORE either thumb is down.
            initial = Keyboard(km, base)
            intended = {p: int(initial.resolve(p).split()[1]) for p in (43, 46)}
            targets = {after for before, after in km.conditions if set(before) == set(intended.values())}
            if not targets:
                failures.append(f'{base}: no conditional rule for both thumbs')
            for press in itertools.permutations((43, 46)):
                for release in itertools.permutations((43, 46)):
                    kb = Keyboard(km, base)
                    held = set()
                    for down, pos in [(True, p) for p in press] + [(False, p) for p in release]:
                        if down:
                            kb.press(pos)
                            held.add(pos)
                        else:
                            kb.release(pos)
                            held.remove(pos)
                        expected = conditional(base | {intended[p] for p in held}, km.conditions)
                        if kb.active() != expected:
                            failures.append(f'{base}, press {press}, release {release}: expected {expected}, got {kb.active()}')
                    if kb.held or kb.active() != conditional(base, km.conditions):
                        failures.append('Stuck layer after releasing both thumbs')
    except (ValueError, IndexError, StopIteration) as exc:
        failures.append(f'Thumb replay cannot run: {exc}')
    return failures


def check_soft_off(km):
    failures = []
    base = km.layers[0][1]
    positions = {key: [i for i, b in enumerate(base) if b == '&kp ' + key] for key in ('Q', 'S', 'Z')}
    if any(len(p) != 1 for p in positions.values()):
        failures.append('Cannot identify unique base Q/S/Z positions')
    else:
        forbidden = {p[0] for p in positions.values()}
        for combo in re.findall(r'\w+\s*\{([^{}]*)\}', node(km.source, 'combos'), re.S):
            match = re.search(r'key-positions\s*=\s*<([\d\s]+)>', combo)
            if match and set(map(int, match[1].split())) == forbidden:
                failures.append(f'Forbidden vendor Q+S+Z combo at {sorted(forbidden)}')
    # ADMIN is reached by holding NUMBER+SYMBOL or GAME+ + SYMBOL. Its &tog 5
    # does not bypass the hold: ZMK conditional rules own the then-layer state.
    targets = {after for _, after in km.conditions}
    def reachable(allow_hold):
        reached = {0}
        while True:
            previous = set(reached)
            behaviors = 'mo|lt|to|tog|sl' if allow_hold else 'to|tog|sl'
            for i in previous:
                for binding in km.layers[i][1]:
                    match = re.match(r'&(?:' + behaviors + r')\s+(\d+)\b', binding)
                    if match and int(match[1]) not in targets:
                        reached.add(int(match[1]))
            for before, after in km.conditions:
                if set(before) <= reached:
                    reached.add(after)
            if reached == previous:
                return reached
    if check_layer_references(km):
        return failures + ['Cannot check soft-off reachability with invalid layer references']
    with_hold, without_hold = reachable(True), reachable(False)
    for i, (name, bindings) in enumerate(km.layers):
        if any(re.search(r'&soft_off\b', b) for b in bindings):
            if i not in with_hold or i in without_hold:
                failures.append(f'{name}: soft_off is not behind a hold')
    # Refuse soft-off in combos/behavior aliases that bypass the inspected layer bindings.
    uses = len(re.findall(r'&soft_off\b(?!\s*\{)', km.source))
    layer_uses = sum(len(re.findall(r'&soft_off\b', b)) for _, bs in km.layers for b in bs)
    if uses != layer_uses:
        failures.append('soft_off appears outside a layer binding')
    return failures


def lint(source):
    try:
        km = parse(source)
        failures = check_binding_counts(km) + check_layer_references(km) + check_soft_off(km)
        if not check_binding_counts(km) and not check_layer_references(km):
            failures += check_thumb_replay(km)
        return failures
    except ValueError as exc:
        return [str(exc)]


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 1:
        print('Usage: keymap_lint.py config/eyelash_corne.keymap')
        return 1
    try:
        failures = lint(Path(args[0]).read_text())
    except OSError as exc:
        failures = [str(exc)]
    for failure in failures:
        print('FAIL: ' + failure)
    if not failures:
        print('Keymap passed: 48 bindings/layer, layer references, thumb replay, and soft-off safety.')
    return int(bool(failures))


if __name__ == '__main__':
    sys.exit(main())
