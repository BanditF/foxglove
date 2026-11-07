"""Photosensitivity check for Bandit art: for each step of the play order (looped, so the wrap
counts too), how many pixels change, as a share of the 68x140 screen, and how many 8x8 blocks
change by more than half (a local inversion).
  python -I check.py out [Bandit_name ...]
Exits 1 if a step changes more than LIMIT of the screen or flips half of any 8x8 block.
`steps()` takes frames as rows of booleans, so the repo test can run it on decoded blobs too.
"""

import os
import sys

W, H = 68, 140
LIMIT = 0.04  # the Bandit Motion set peaks at 3.5% (compass); Bandit_signal (v1) hits 28.8%


def steps(seq):
    """(most pixels changed in one step, most 8x8 blocks changed by over half in one step) over
    a looped sequence of frames, each H rows of W booleans."""
    worst, worst_blocks = 0, 0
    for a, b in zip(seq, seq[1:] + seq[:1]):
        diff = [[a[y][x] != b[y][x] for x in range(W)] for y in range(H)]
        n = sum(map(sum, diff))
        blocks = sum(1 for by in range(0, H, 8) for bx in range(0, W, 8)
                     if sum(diff[y][x] for y in range(by, min(by + 8, H))
                            for x in range(bx, min(bx + 8, W))) > 32)
        worst, worst_blocks = max(worst, n), max(worst_blocks, blocks)
    return worst, worst_blocks


def passes(worst, worst_blocks):
    return worst <= LIMIT * W * H and worst_blocks == 0


def frames(d, name):
    from PIL import Image

    with open(os.path.join(d, name, "meta.txt"), encoding="utf-8") as f:
        meta = dict(l.split(":", 1) for l in f if ":" in l)
    order = [int(i) for i in meta["Frames order"].split()]
    rows = {}
    for i in set(order):
        px = Image.open(os.path.join(d, name, "frame_%d.png" % i)).convert("1").load()
        rows[i] = [[bool(px[x, y]) for x in range(W)] for y in range(H)]
    return [rows[i] for i in order], int(meta["Frame rate"])


def main():
    d = sys.argv[1]
    names = sys.argv[2:] or sorted(os.listdir(d))
    bad = False
    for name in names:
        seq, fps = frames(d, name)
        worst, worst_blocks = steps(seq)
        ok = passes(worst, worst_blocks)
        bad |= not ok
        print("%-16s %3d steps @%d fps  max change %4d px (%4.1f%%)  half-flipped 8x8 blocks %d  %s"
              % (name, len(seq), fps, worst, 100.0 * worst / (W * H), worst_blocks,
                 "ok" if ok else "FAIL"))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
