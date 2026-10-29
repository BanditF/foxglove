"""The right panel after the PC takes and hands back the LEDs (#621 check 4; the sitting's old R7-R8,
"open and close the Foxglove window three times, did the right show static?"). The Foxglove
window's preview takes the LEDs with SET_FRAME / SET_EFFECT and hands them back; both switch the
right's LED rail, which also powers its screen, and #621 rewrites the whole panel after each.
foxglove-lab's frame-dot takes the LEDs (SET_FRAME, a dot through every lamp) and hands them back
(RELEASE) the same way, without a window. After each of `--cycles` runs the right's whole-screen
rewrite count (D7 8E bytes 16-20, HELLO bit 25; `foxglove-lab status` prints RIGHT_REDRAWS) must
have grown by `--at-least` (3): each event starts two rewrites (100 ms and 1 s after it, coalesced
within a burst), and the dot runs about 1.3 s, so the take gives two and the hand-back two more;
3 proves the hand-back's burst ran, not only the take's. The count proves the rewrite ran; whether
the panel then looks clean is the sitting's R2 look (the panel is write-only, no readback sees
noise).

Needs Foxglove quit (foxglove-lab refuses while it runs: its scenes own the lights) and both halves
on a build with HELLO bits 14 (frames) and 25 (main firmware since 1ebdf9d; 13ff04b has no bit 25:
BLOCKED). Launches nothing; makes no input (frame-dot types nothing).

    python scripts/lab/right_redraws.py [--lab target/release/foxglove-lab.exe] [--cycles 3]
    (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, re, subprocess, time
from labkit import Checks, REPO, TARGET, tool_arg


def parse_redraws(out):
    """RIGHT_REDRAWS from `foxglove-lab status`: the count, or None (unknown, an error, no bit 25)."""
    m = re.search(r'^RIGHT_REDRAWS: (\d+)\s*$', out, re.M)
    return int(m.group(1)) if m else None


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
    ap.add_argument('--cycles', type=int, default=3)
    ap.add_argument('--within', type=float, default=5.0, help='seconds for the count to grow after a hand-back')
    ap.add_argument('--at-least', type=int, default=3, help='rewrites per take-and-hand-back cycle')
    args = ap.parse_args()
    check = Checks()

    def lab(*a, timeout=60):
        return subprocess.run([str(args.lab), *a], capture_output=True, text=True, encoding='utf-8',
                              errors='replace', cwd=REPO, timeout=timeout)

    def redraws():
        r = lab('status')
        return parse_redraws(r.stdout), r

    n, r = redraws()
    if n is None:
        why = ('Foxglove is running (quit it first)' if 'Foxglove is running' in r.stderr else
               next((l for l in r.stdout.splitlines() if l.startswith('RIGHT_REDRAWS')), r.stderr.strip()[-200:]))
        check('the right reports its redraw count', False, why, blocked=True)
        check.finish()
    check('the right reports its redraw count', True, n)
    for i in range(1, args.cycles + 1):
        f = lab('frame-dot', '--step-ms', '30', timeout=120)
        if f.returncode == 4:
            check(f'cycle {i}: take and hand back the LEDs', False, 'no SET_FRAME (HELLO bit 14)', blocked=True)
            break
        if not check(f'cycle {i}: take and hand back the LEDs (frame-dot)', f.returncode == 0,
                     f'exit {f.returncode} {f.stderr.strip()[-200:]}'):
            break
        end, now = time.time() + args.within, None
        while time.time() < end:  # bounded; the count has no event of its own
            now, _ = redraws()
            if now is not None and now >= n + args.at_least:
                break
            time.sleep(1)
        check(f'cycle {i}: the right rewrote its panel after the take and after the hand-back',
              now is not None and now >= n + args.at_least, f'RIGHT_REDRAWS {n} -> {now} (wanted +{args.at_least})')
        n = now if now is not None else n
    check.finish()


if __name__ == '__main__':
    main()
