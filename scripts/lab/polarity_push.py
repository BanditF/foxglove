"""#666 push checks on hardware (On-call): with the app under test on a temp config, push the
light-background and dark-background versions of one drawing as encoded files
(tests/art/ink-fixture.bin, tests/art/dark-fixture.bin, the dark one stored as is), and check:
- a pushed piece flips with the panel (Light, then back to Dark);
- the light- and dark-background files look the same on the right (auto normalises the dark one);
- ink-fixture.bin with Ink: Invert is the inverse.
Captures are the right's readback through foxglove-lab (capture_screen); the status strip is
masked. Each check takes the best pair over 4 captures, so a capture mid-redraw doesn't fail it.

  python scripts/lab/polarity_push.py --app <foxglove.exe> --lab <foxglove-lab.exe> --out <dir> [--serial <left's>]
"""
import argparse
import json
import os
import subprocess
import sys
import time

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from labkit import REPO, App, connect, temp_config, tool_arg, users_keyboard  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument('--app', required=True, type=tool_arg('--app'))
ap.add_argument('--lab', required=True, type=tool_arg('--lab'))
ap.add_argument('--out', required=True)
ap.add_argument('--serial', default='', help="the left's USB serial when more than one host-link device is attached")
a = ap.parse_args()
os.makedirs(a.out, exist_ok=True)


def lab(tag, steps):
    """One foxglove-lab case next to the app; the report's PNG attachments."""
    out = os.path.join(a.out, tag)
    suite = out + '.toml'
    with open(suite, 'w', encoding='utf-8') as f:
        f.write('schema = 1\n[[cases]]\nname = "%s"\ndescription = "%s"\nsteps = [%s]\n' % (tag, tag, ', '.join(steps)))
    subprocess.run([a.lab, '--allow-app', 'run', '--suite', suite, '--non-interactive', '--out', out],
                   capture_output=True, timeout=120)
    c = json.load(open(os.path.join(out, 'report.json'), encoding='utf-8'))['cases'][0]
    return c, [os.path.join(out, x) for s in c['steps'] for x in s.get('attachments', []) if x.endswith('.png')]


def cap(tag):
    _, att = lab(tag, ['{ capture_screen = { half = "right" } }'])
    return Image.open(att[-1]).convert('L') if att else None


def caps(tag, k=4):
    r = []
    for i in range(k):
        im = cap(f'{tag}-{i}')
        if im:
            r.append(im)
        time.sleep(0.3)
    return r


def same_pct(p, q):
    """Share of pixels below the status strip that match."""
    w, h = p.size
    y0 = 20 * h // 160
    P, Q = p.load(), q.load()
    n = m = 0
    for y in range(y0, h):
        for x in range(w):
            n += 1
            m += (P[x, y] > 127) == (Q[x, y] > 127)
    return 100 * m / n


fails = 0


def check(name, ok, detail):
    global fails
    fails += not ok
    print(('PASS ' if ok else 'FAIL ') + f'{name}: {detail}', flush=True)


def polarity(light):
    """Sets the panel polarity with the host link's D7 12 (0 dark, 1 light) on the left; returns
    the D7 92 reply in words."""
    import hid
    found = [x for x in hid.enumerate(0x1D50, 0x615E) if x['usage_page'] == 0xFF60 and x['usage'] == 0x61
             and (not a.serial or x['serial_number'] == a.serial)]
    if not found:
        return 'no host-link device'
    h = hid.device()
    h.open_path(found[0]['path'])
    try:
        h.write(bytes([0, 0xD7, 0x12, 1 if light else 0]).ljust(33, b'\0'))
        end = time.time() + 1.5
        while time.time() < end:
            r = h.read(32, 100)
            if r and r[:2] == [0xD7, 0x92]:
                return f'status {r[2]} polarity {r[3]} right {r[5]}'
        return 'no D7 92 reply'
    finally:
        h.close()


cfg = temp_config('foxglove-polarity-push-', users_keyboard())
light_bin = str(REPO / 'tests' / 'art' / 'ink-fixture.bin')
dark_bin = str(REPO / 'tests' / 'art' / 'dark-fixture.bin')
with App(a.app, cfg):
    api = connect('polarity-push', ['devices.read', 'art.write'])
    dev = None
    for _ in range(40):
        d = api.call('devices.list', {}).get('devices', [])
        if d:
            dev = d[0]['id']
            break
        time.sleep(0.5)
    if dev is None:
        print('RESULT BLOCKED: no keyboard')
        sys.exit(2)
    start = api.call('art.status', {'device': dev})
    print('art.status before:', start, flush=True)
    print('push light-background file:', api.call('art.select', {'device': dev, 'target': {'file': light_bin}}), flush=True)
    A = caps('light-file')
    print('panel Light:', polarity(True), flush=True)
    time.sleep(1.5)
    A2 = caps('light-file-on-light')
    print('panel Dark:', polarity(False), flush=True)
    time.sleep(1.5)
    flip = max((100 - same_pct(p, q) for p in A for q in A2), default=0)
    check('a pushed piece flips with the panel (Light)', flip >= 98, f'best inverse {flip:.1f}%')
    print('push dark-background file:', api.call('art.select', {'device': dev, 'target': {'file': dark_bin}}), flush=True)
    B = caps('dark-file')
    best = max((same_pct(p, q) for p in A for q in B), default=0)
    check('light- and dark-background files look the same', best >= 98, f'best match {best:.1f}% ({len(A)}x{len(B)} captures)')
    print('push light-background file, Ink: Invert:',
          api.call('art.select', {'device': dev, 'target': {'file': light_bin}, 'polarity': 'invert'}), flush=True)
    C = caps('invert')
    inv = max((100 - same_pct(p, q) for p in A for q in C), default=0)
    check('Ink: Invert flips the piece', inv >= 98, f'best inverse {inv:.1f}%')
    if start and start.get('built_in'):
        print('back to', start['built_in'],
              api.call('art.select', {'device': dev, 'target': {'built_in': start['built_in']}}), flush=True)
print('RESULT', 'PASS' if fails == 0 else 'FAIL')
sys.exit(1 if fails else 0)
