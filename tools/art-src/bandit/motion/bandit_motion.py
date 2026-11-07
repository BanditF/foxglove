"""Bandit Motion: a second set of Bandit fox scenes for the right-hand nice!view (68x140, 1-bit).

It adds movement and distortion styles to the house look and leaves the first library alone:
it imports the drawing helpers from ../bandit_art.py and never changes them.
  python -I bandit_motion.py out [Bandit_name ...]

Photosensitivity: no flashing and no inversions. Only small details change from frame to frame
(a scan beam, a few glitch rows, a drifting band); anything that changes a large area does it
gradually, over seconds.
"""

import math
import os
import random
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from PIL import Image  # noqa: E402

from bandit_art import (  # noqa: E402
    BAYER, CX, CY, FOX_OX, FOX_OY, FOX_SCALE, H, R, SS, W, P, blank, centered, compose, corners,
    draw_fox, frame_parts, hline, new_layer, nodes, rect, ring, small_fox, text35, text35_w,
    title_block, triangle, write_anim,
)
from PIL import ImageDraw  # noqa: E402

# ---------------------------------------------------------------- shared look
_rnd = random.Random(26)
DUST = []
while len(DUST) < 26:
    x, y = _rnd.randrange(2, W - 2), _rnd.randrange(9, 86)
    if math.hypot(x - CX, y - CY) > R + 2 and not (x < 31 and y < 10) and not (x > 50 and y < 10):
        DUST.append((x, y))


def dust(sp, f):
    for i, (x, y) in enumerate(DUST):
        if (i * 5 + f) % 13:  # each speck rests once in a while (single pixels: a twinkle)
            sp[x, y] = 1


def runner(sp, f, n, y=129):
    """A small marker riding a dotted rail in the footer, once across per loop (cheap to store:
    only the marker moves)."""
    hline(sp, 4, W - 5, y, 2)
    x = 4 + f * (W - 13) // n
    rect(sp, x, y - 1, x + 4, y + 1, fill=True)


def footer(sp, f, n, label, value=""):
    hline(sp, 4, W - 5, 114, 2)
    text35(sp, 4, 118, label)
    if value:
        text35(sp, W - 4 - text35_w(value), 118, value)
    runner(sp, f, n)
    hline(sp, 0, W - 1, 133)
    for y in range(134, H):  # checker floor: the midtone fills the bottom edge
        for x in range(W):
            if (x + y) % 2 == 0:
                sp[x, y] = 1


def hud(f, n, label, value="", phase=0.0, fox=True, tri=True):
    """The emblem layout every scene shares: fox in the ring, title, dust and a footer."""
    big, small, layer, sp = frame_parts()
    ring(big, phase)
    if fox:
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
    nodes(sp)
    triangle(sp, tri)
    corners(sp, 0, 0, W - 1, H - 1, 6)
    text35(sp, 3, 3, "BNDT-01")
    title_block(sp)
    dust(sp, f)
    footer(sp, f, n, label, value)
    return big, small, layer, sp


def fox_mask():
    layer = new_layer()
    draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
    return layer[1].point(lambda v: 255 if v else 0).convert("1")


def shift_row(img, y, dx):
    if dx == 0 or not 0 <= y < H:
        return
    row = img.crop((0, y, W, y + 1))
    img.paste(Image.new("1", (W, 1), 0), (0, y))
    img.paste(row, (dx, y))


def xor_dots(img, y, step, x0=4, x1=W - 5, offset=0):
    if not 0 <= y < H:
        return
    px = img.load()
    for x in range(x0 + offset, x1 + 1, step):
        px[x, y] = 0 if px[x, y] else 1


# ---------------------------------------------------------------- scenes
def scanline():
    """A slow CRT beam walks down the emblem: the rows under it bulge a pixel or two, a dotted
    beam line leads and a fainter one trails, and side pointers track it."""
    ys = list(range(12, 114, 3))
    n = len(ys)
    frames = []
    for f, y in enumerate(ys):
        big, small, layer, sp = hud(f, n, "SCAN", "L%03d" % y, phase=f * 15.0 / n)
        img = compose(big, small, layer)
        for dy, dx in ((-1, 2), (0, 2), (1, 2)):  # even shifts keep the checker's phase
            shift_row(img, y + dy, dx)
        xor_dots(img, y + 2, 2)
        xor_dots(img, y - 4, 4, offset=1)
        p = P(img)
        for i in range(3):  # side pointers, outside the frame brackets
            p[i, y] = 1 if i < 2 else 0
            p[W - 1 - i, y] = 1 if i < 2 else 0
        frames.append(img)
    return frames, 5, list(range(n))


GLITCH = {  # frame: [(row, height, shift)], small and rare; even shifts keep the checker's phase
    5: [(31, 3, 4)], 6: [(31, 3, 2)],
    13: [(60, 2, -2), (43, 1, 4)],
    21: [(73, 3, -4)], 22: [(73, 3, -2), (22, 2, 2)],
    28: [(50, 4, 3)],
}
BLOCKS = {13: (18, 52, 8, 3, 4), 22: (40, 66, 6, 2, -4), 28: (24, 38, 7, 2, 4)}  # x, y, w, h, dx


def glitch():
    """The emblem holding steady while a few thin slices slip sideways now and then, with a
    small block of the fur torn loose; the sync counter dips while they show."""
    n = 32
    frames = []
    for f in range(n):
        hit = f in GLITCH
        big, small, layer, sp = hud(f, n, "SYNC", "%d%%" % (91 if hit else 99), phase=f * 15.0 / n)
        img = compose(big, small, layer)
        out = img.copy()
        for y0, h, dx in GLITCH.get(f, ()):
            band = img.crop((0, y0, W, y0 + h))
            out.paste(Image.new("1", (W, h), 0), (0, y0))
            out.paste(band, (dx, y0))
        if f in BLOCKS:
            x, y, w, h, dx = BLOCKS[f]
            out.paste(img.crop((x, y, x + w, y + h)), (x + dx, y))
        frames.append(out)
    return frames, 4, list(range(n))


BAYER8 = [[0, 32, 8, 40, 2, 34, 10, 42], [48, 16, 56, 24, 50, 18, 58, 26],
          [12, 44, 4, 36, 14, 46, 6, 38], [60, 28, 52, 20, 62, 30, 54, 22],
          [3, 35, 11, 43, 1, 33, 9, 41], [51, 19, 59, 27, 49, 17, 57, 25],
          [15, 47, 7, 39, 13, 45, 5, 37], [63, 31, 55, 23, 61, 29, 53, 21]]


def cloak():
    """The fox decloaks top-down behind a dithered edge and a dotted beam, holds, then fades back
    out through an ordered dither, a sixteenth of it at a time. The ring keeps turning."""
    n_in, n_hold, n_out, n_gone = 16, 12, 16, 4
    n = n_in + n_hold + n_out + n_gone
    mask = fox_mask().load()
    y_top, y_bot = 13, 89
    frames = []
    for f in range(n):
        phase = f * 30.0 / n
        if f < n_in:
            k, state = (f + 1) * 2, "in"
        elif f < n_in + n_hold:
            k, state = 99, "on"
        elif f < n_in + n_hold + n_out:
            k, state = 64 - (f - n_in - n_hold + 1) * 4, "out"
        else:
            k, state = 0, "off"
        vis = {"in": (f + 1) * 100 // n_in, "on": 100, "out": k * 100 // 64, "off": 0}[state]
        label = "CLOAK"
        value = "OFF" if vis == 100 else ("ON" if vis == 0 else "%d%%" % vis)
        big, small, layer, sp = hud(f, n, label, value, phase=phase)
        on = compose(big, small, layer)
        big, small, layer, sp = hud(f, n, label, value, phase=phase, fox=False)
        off = compose(big, small, layer)
        opx, fpx = on.load(), off.load()
        for y in range(H):
            front = 16.0 * (y - y_top) / (y_bot - y_top)
            for x in range(W):
                if not mask[x, y]:
                    continue
                if state == "in":
                    shown = BAYER[y % 4][x % 4] + front < k
                elif state == "out":
                    shown = BAYER8[y % 8][x % 8] < k
                else:
                    shown = state == "on"
                if not shown:
                    opx[x, y] = fpx[x, y]
        if state == "in":  # the beam rides the middle of the dithered edge
            yb = int(y_top + (k - 8) * (y_bot - y_top) / 16.0)
            for x in range(0, W, 2):
                if 0 <= yb < H and mask[x, yb]:
                    opx[x, yb] = 1
        for i in range(12):  # visibility bar under the label
            if i < vis * 12 // 100:
                opx[4 + i * 2, 124] = 1
        frames.append(on)
    return frames, 4, list(range(n))


def signal_v2():
    """Bandit_signal's story told gently (the first one cuts to static and blinks): the link
    holds, slips, the fox fades out behind a drifting band of static, SIGNAL LOST waits in the
    empty ring, the bars climb back while SEARCHING, and the fox scans back in top-down."""
    mask = fox_mask().load()
    y_top, y_bot = 13, 89
    plan = ([("hold", 0)] * 6 + [("slip", 0), ("slip", 1)] + [("hold", 0)] * 2
            + [("fade", i) for i in range(16)] + [("lost", i) for i in range(8)]
            + [("search", i) for i in range(10)] + [("back", i) for i in range(16)]
            + [("hold", 0)] * 4)
    n = len(plan)
    slips = [[(34, 2, 2), (58, 1, -2)], [(34, 2, 4), (66, 2, -2)]]
    frames = []
    for f, (state, i) in enumerate(plan):
        if state == "fade":
            k, sig = 64 - (i + 1) * 4, 96 - i * 6
        elif state == "back":
            k, sig = (i + 1) * 2, 100
        else:
            k, sig = (0, 0) if state in ("lost", "search") else (99, 100)
        value = {"lost": "LOST", "search": "%d%%" % (i * 10), "back": "%d%%" % ((i + 1) * 100 // 16)}.get(
            state, "LOCK" if sig == 100 else "%d%%" % sig)
        big, small, layer, sp = hud(f, n, "CH 07", value)
        on = compose(big, small, layer)
        big, small, layer, sp = hud(f, n, "CH 07", value, fox=False)
        if state in ("lost", "search"):
            if state == "lost":
                centered(sp, 43, "SIGNAL")
                centered(sp, 51, "LOST")
            else:
                centered(sp, 36, "SEARCHING")
                for b in range(5):  # bars climb back, one every two frames
                    h = 3 + b * 2
                    x = 23 + b * 5
                    rect(sp, x, 60 - h, x + 2, 60, fill=b < i // 2 + 1)
        off = compose(big, small, layer)
        if state in ("lost", "search"):  # the ring's text reaches past the fox's outline
            on = off.copy()
        opx, fpx = on.load(), off.load()
        for y in range(H):
            front = 16.0 * (y - y_top) / (y_bot - y_top)
            for x in range(W):
                if not mask[x, y]:
                    continue
                if state == "fade":
                    shown = BAYER8[y % 8][x % 8] < k
                elif state == "back":
                    shown = BAYER[y % 4][x % 4] + front < k
                else:
                    shown = k == 99
                if not shown:
                    opx[x, y] = fpx[x, y]
        if state == "back":  # the beam rides the dithered edge
            yb = int(y_top + (k - 8) * (y_bot - y_top) / 16.0)
            for x in range(0, W, 2):
                if 0 <= yb < H and mask[x, yb]:
                    opx[x, yb] = 1
        rnd = random.Random(f * 17 + 3)
        if state == "fade":  # a sparse band of static drifts down over the fading fox
            y0 = 16 + i * 4
            for y in range(y0, y0 + 5):
                for x in range(4, W - 4):
                    if rnd.random() < 0.05:
                        opx[x, y] = 0 if opx[x, y] else 1
        elif state in ("lost", "search"):  # a few specks of static in the empty ring
            for _ in range(10):
                x, y = rnd.randrange(CX - 22, CX + 23), rnd.randrange(CY - 22, CY + 23)
                if math.hypot(x - CX, y - CY) < 24 and mask[x, y]:
                    opx[x, y] = 1
        out = on
        if state == "slip":
            out = on.copy()
            for y0, h, dx in slips[i]:
                band = on.crop((0, y0, W, y0 + h))
                out.paste(Image.new("1", (W, h), 0), (0, y0))
                out.paste(band, (dx, y0))
        frames.append(out)
    return frames, 4, list(range(n))


def drift():
    """A band of weak signal drifts down over the emblem: its rows lean sideways like a VHS
    tracking error, with sparse static and a dotted carrier line in the middle."""
    ys = list(range(8, 120, 3))
    n = len(ys)
    frames = []
    for f, y0 in enumerate(ys):
        big, small, layer, sp = hud(f, n, "CH 07", "TRK", phase=f * 15.0 / n)
        img = compose(big, small, layer)
        rnd = random.Random(f * 31 + 7)
        px = img.load()
        for r in range(7):
            y = y0 + r
            shift_row(img, y, 2 if 0 < r < 6 else 0)  # even: the checker keeps its phase
            if not 0 <= y < H:
                continue
            for x in range(W):
                if rnd.random() < 0.05:
                    px[x, y] = 0 if px[x, y] else 1
        xor_dots(img, y0 + 3, 3, 0, W - 1)
        frames.append(img)
    return frames, 4, list(range(n))


def edge(mask):
    """Outline of a 1-bit mask (pixels in it with a 4-neighbour outside)."""
    m = mask.load()
    pts = []
    for y in range(H):
        for x in range(W):
            if m[x, y] and any(not (0 <= x + dx < W and 0 <= y + dy < H) or not m[x + dx, y + dy]
                               for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))):
                pts.append((x, y))
    return pts


def ghost():
    """'Chromatic' misregistration in 1-bit: two outline ghosts of the fox slide out to either
    side and back, one solid and one dotted, like colour channels drifting apart."""
    n = 24
    mask = fox_mask()
    m = mask.load()
    outline = edge(mask)
    frames = []
    for f in range(n):
        d = int(round(4 * (0.5 - 0.5 * math.cos(2 * math.pi * f / n))))
        big, small, layer, sp = hud(f, n, "ALIGN", "%dPX" % d)
        img = compose(big, small, layer)
        p = P(img)
        for gx, gy, dotted in ((d, 0, False), (-d, -(d // 2), True)):
            if gx == 0 and gy == 0:
                continue
            for x, y in outline:
                x2, y2 = x + gx, y + gy
                if 0 <= x2 < W and 0 <= y2 < H and not m[x2, y2] and (not dotted or (x2 + y2) % 2):
                    p[x2, y2] = 1
        frames.append(img)
    return frames, 4, list(range(n))


def compass():
    """A compass bezel swings around a smaller fox and settles: degree ticks, N E S W riding
    the ring, a fixed lubber mark and the heading readout."""
    n = 32
    frames = []
    for f in range(n):
        hdg = 28 * math.sin(2 * math.pi * f / n) * (0.75 + 0.25 * math.cos(2 * math.pi * f / n))
        big, small, layer, sp = frame_parts()
        d = ImageDraw.Draw(big)
        box = [(CX - R) * SS, (CY - R) * SS, (CX + R) * SS, (CY + R) * SS]
        d.ellipse(box, outline=255, width=SS)
        for deg in range(0, 360, 10):
            a = math.radians(deg - hdg - 90)
            r1 = R - (6 if deg % 30 == 0 else 3)
            d.line([((CX + (R - 1) * math.cos(a)) * SS, (CY + (R - 1) * math.sin(a)) * SS),
                    ((CX + r1 * math.cos(a)) * SS, (CY + r1 * math.sin(a)) * SS)],
                   fill=255, width=SS)
        tone, tmask = layer  # a dithered bezel band for the letters to ride in
        dt, dm = ImageDraw.Draw(tone), ImageDraw.Draw(tmask)
        for r, v in ((25, 128), (20, 0)):
            dt.ellipse([CX - r, CY - r, CX + r, CY + r], fill=v)
            dm.ellipse([CX - r, CY - r, CX + r, CY + r], fill=255 if v else 0)
        small_fox(layer, CX, CY - 14, 28)
        for deg, ch in ((0, "N"), (90, "E"), (180, "S"), (270, "W")):
            a = math.radians(deg - hdg - 90)
            lx, ly = int(round(CX + 23 * math.cos(a))) - 1, int(round(CY + 23 * math.sin(a))) - 2
            for xx in range(lx - 1, lx + 4):
                for yy in range(ly - 1, ly + 6):
                    sp[xx, yy] = 2
            text35(sp, lx, ly, ch)
        for i in range(4):  # lubber mark above the ring, fixed
            for j in range(-i, i + 1):
                sp[CX + j, 14 - i] = 1
        for a in (0, 90, 180, 270):  # fixed inner index dots
            r = math.radians(a)
            sp[int(round(CX + 13 * math.cos(r))), int(round(CY + 13 * math.sin(r)))] = 1 if a else 0
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "BNDT-01")
        h = int(round(hdg)) % 360
        title_block(sp)
        dust(sp, f)
        footer(sp, f, n, "HDG", "%03d" % h)
        frames.append(compose(big, small, layer))
    return frames, 4, list(range(n))


SCENES = [
    ("Bandit_scanline", scanline), ("Bandit_glitch", glitch), ("Bandit_cloak", cloak),
    ("Bandit_drift", drift), ("Bandit_ghost", ghost), ("Bandit_compass", compass),
    ("Bandit_signal_v2", signal_v2),
]


def main():
    out = sys.argv[1]
    only = set(sys.argv[2:])
    for name, fn in SCENES:
        if not only or name in only:
            write_anim(out, name, *fn())


if __name__ == "__main__":
    main()
