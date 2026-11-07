"""Bandit HUD art for the right-hand nice!view (68x140 portrait, 1-bit).

Writes animation folders (meta.txt + frame_N.png) that scripts/flipper_art.py turns into art sets:
  python scripts/bandit_art.py OUTDIR
Original art based on the user's Bandit fox motif.
"""

import math
import os
import random
import sys

from PIL import Image, ImageDraw, ImageOps

W, H = 68, 140
SS = 4  # supersampling for the fox and rings
CX, CY, R = 34, 50, 32  # HUD ring

class P:
    """Bounds-checked pixel writer for the 1x layer (1 = lit, 2 = force dark)."""

    def __init__(self, img):
        self.px = img.load()

    def point(self, xy, v=1):
        x, y = xy
        if 0 <= x < W and 0 <= y < H:
            self.px[x, y] = v

    def __setitem__(self, xy, v):
        self.point(xy, v)


# ---------------------------------------------------------------- fonts
F35 = {
    "A": "010101111101101", "B": "110101110101110", "C": "011100100100011", "D": "110101101101110",
    "E": "111100110100111", "F": "111100110100100", "G": "011100101101011", "H": "101101111101101",
    "I": "111010010010111", "J": "001001001101010", "K": "101101110101101", "L": "100100100100111",
    "M": "101111111101101", "N": "110101101101101", "O": "010101101101010", "P": "110101110100100",
    "Q": "010101101110011", "R": "110101110101101", "S": "011100010001110", "T": "111010010010010",
    "U": "101101101101111", "V": "101101101101010", "W": "101101111111101", "X": "101101010101101",
    "Y": "101101010010010", "Z": "111001010100111", "0": "111101101101111", "1": "010110010010111",
    "2": "110001010100111", "3": "110001010001110", "4": "101101111001001", "5": "111100110001110",
    "6": "011100111101111", "7": "111001010010010", "8": "111101111101111", "9": "111101111001110",
    "'": "010010000000000", "-": "000000111000000", ".": "000000000000010", ":": "000010000010000",
    "/": "001001010100100", " ": "000000000000000", ">": "100010001010100", "%": "101001010100101",
}
F57 = {
    "B": ["11110", "10001", "10001", "11110", "10001", "10001", "11110"],
    "A": ["01110", "10001", "10001", "11111", "10001", "10001", "10001"],
    "N": ["10001", "11001", "10101", "10011", "10001", "10001", "10001"],
    "D": ["11110", "10001", "10001", "10001", "10001", "10001", "11110"],
    "I": ["11111", "00100", "00100", "00100", "00100", "00100", "11111"],
    "T": ["11111", "00100", "00100", "00100", "00100", "00100", "00100"],
    "F": ["11111", "10000", "10000", "11110", "10000", "10000", "10000"],
    "O": ["01110", "10001", "10001", "10001", "10001", "10001", "01110"],
    "C": ["01111", "10000", "10000", "10000", "10000", "10000", "01111"],
    "U": ["10001", "10001", "10001", "10001", "10001", "10001", "01110"],
    "S": ["01111", "10000", "10000", "01110", "00001", "00001", "11110"],
    "R": ["11110", "10001", "10001", "11110", "10100", "10010", "10001"],
}


def text35(px, x, y, s, on=1):
    for ch in s:
        g = F35[ch.upper()]
        for i, b in enumerate(g):
            if b == "1":
                px.point((x + i % 3, y + i // 3), on)
        x += 4


def text35_w(s):
    return len(s) * 4 - 1


def text57(px, x, y, s, gap=3, on=1):
    for ch in s:
        for r, row in enumerate(F57[ch]):
            for c, b in enumerate(row):
                if b == "1":
                    px.point((x + c, y + r), on)
        x += 5 + gap


# ---------------------------------------------------------------- fox (crop coords 680 x 760)
FOX_W, FOX_H = 680.0, 760.0


def mirror(poly):
    return [(FOX_W - x, y) for x, y in poly]


EAR = [(20, 80), (245, 300), (115, 430)]
EAR_IN = [(62, 165), (196, 305), (120, 375)]
FACE = [(238, 296), (442, 296), (608, 462), (402, 724), (278, 724), (72, 462)]
CHEEK = [(8, 466), (170, 494), (310, 748), (250, 748)]
MASK = [(70, 456), (268, 500), (300, 700), (180, 572)]
EYE = [(150, 498), (266, 528), (236, 566)]
NOSE = [(300, 704), (380, 704), (340, 752)]
FACET = [(238, 296), (232, 410), (272, 470), (272, 600)]

PARTS = ["ears", "face", "cheeks", "mask", "lines"]


FACE_L = [(238, 296), (340, 296), (340, 724), (278, 724), (72, 462)]
MUZZLE_L = [(272, 470), (340, 440), (340, 704), (300, 704), (272, 600)]
BROW_L = [(238, 296), (340, 296), (340, 360), (236, 380)]


def draw_fox(layer, ox, oy, scale, parts=PARTS, eyes=True):
    """Low-poly fox as grey tones (lit from the upper left), dithered later by compose()."""
    tone, mask = layer
    dt, dm = ImageDraw.Draw(tone), ImageDraw.Draw(mask)

    def tr(poly):
        return [(ox + x * scale, oy + y * scale) for x, y in poly]

    def poly(p, v):
        dt.polygon(tr(p), fill=v)
        dm.polygon(tr(p), fill=255)

    def line(p, v=0):
        dt.line(tr(p), fill=v, width=1)
        dm.line(tr(p), fill=255, width=1)

    if "ears" in parts:
        for p, rim, fill in ((EAR, 255, ORANGE), (mirror(EAR), 255, ORANGE)):
            poly(p, rim)
            poly(shrink(p, 0.8), fill)
        for p in (EAR_IN, mirror(EAR_IN)):
            poly(shrink(p, 0.6), 0)
    if "cheeks" in parts:
        poly(CHEEK, 255)
        poly(mirror(CHEEK), 255)
    if "face" in parts:
        poly(FACE, ORANGE)
    if "mask" in parts:
        for p in (MASK, mirror(MASK)):
            poly(p, 0)
        for p in (EYE, mirror(EYE)) if eyes else ():
            poly(p, 255)
        poly(NOSE, 0)
    if "lines" in parts:
        line([(340, 300), (340, 700)])
        for p in (FACET, mirror(FACET)):
            line(p)
        for p in (MUZZLE_L, mirror(MUZZLE_L)):
            line(p[1:4])


ORANGE = 128  # rendered as a checker dither, like the orange of the motif against cream


def shrink(poly, f):
    cx = sum(x for x, _ in poly) / len(poly)
    cy = sum(y for _, y in poly) / len(poly)
    return [(cx + (x - cx) * f, cy + (y - cy) * f) for x, y in poly]


FOX_SCALE = 60 / FOX_W
FOX_OX, FOX_OY = CX - 30, 13


# ---------------------------------------------------------------- HUD pieces (1x, on '1' images)
def corners(px, x0, y0, x1, y1, n=4):
    for (x, y, dx, dy) in ((x0, y0, 1, 1), (x1, y0, -1, 1), (x0, y1, 1, -1), (x1, y1, -1, -1)):
        for i in range(n):
            px.point((x + dx * i, y), 1)
            px.point((x, y + dy * i), 1)


def ring(big, phase=0.0, sweep=360.0, dashes=24):
    """Dashed ring on the supersampled image, rotating with phase (degrees)."""
    d = ImageDraw.Draw(big)
    box = [(CX - R) * SS, (CY - R) * SS, (CX + R) * SS, (CY + R) * SS]
    step = 360.0 / dashes
    a = 0.0
    while a < sweep - 0.01:
        start = a + phase - 90
        d.arc(box, start, start + step * 0.55, fill=255, width=SS)
        a += step
    # radial ticks every 15 degrees
    for k in range(int(sweep // 15)):
        a = math.radians(k * 15 - 90)
        r0, r1 = R - 5, R - 3 if k % 2 else R - 7
        d.line([((CX + r0 * math.cos(a)) * SS, (CY + r0 * math.sin(a)) * SS),
                ((CX + r1 * math.cos(a)) * SS, (CY + r1 * math.sin(a)) * SS)], fill=255, width=SS)
    # inner thin ring, solid but broken at the cardinal points
    r2 = R - 3
    box2 = [(CX - r2) * SS, (CY - r2) * SS, (CX + r2) * SS, (CY + r2) * SS]
    for q in range(4):
        s = q * 90 - 90 + 12
        e = min(q * 90 - 90 + 78, sweep - 90)
        if e > s:
            d.arc(box2, s, e, fill=255, width=SS // 2 + 1)


def nodes(px):
    for x, y in ((CX, CY - R), (CX + R, CY), (CX, CY + R), (CX - R, CY)):
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                px.point((x + dx, y + dy), 1)


def triangle(px, on=True):
    if not on:
        return
    x, y = CX, 25
    for i in range(4):
        px.point((x - i, y + i), 1)
        px.point((x + i, y + i), 1)
    for i in range(-3, 4):
        px.point((x + i, y + 3), 1)


def waveform(px, y0, phase, x0=6, x1=44):
    for x in range(x0, x1):
        a = math.sin((x + phase) * 0.55) * math.sin((x * 0.13 + phase * 0.21))
        h = int(round(abs(a) * 4))
        for y in range(y0 - h, y0 + 1):
            px.point((x, y), 1)


def lock_icon(px, x, y, closed=True):
    for i in range(5):
        for j in range(4):
            px.point((x + i, y + 3 + j), 1)
    px.point((x + 2, y + 5), 0)
    sh = 0 if closed else -1
    for j in range(3):
        px.point((x + 1 + sh, y + j), 1)
        px.point((x + 3, y + j), 1)
    px.point((x + 2 + (0 if closed else -1), y), 1)


def title_block(px, letters=6, subtitle=True):
    """BANDIT INC. in the bracketed title box; letters < 6 types the name in (boot)."""
    word = "BANDIT"[:letters]
    tw = 6 * 5 + 5 * 2 + 3 + text35_w("INC.")
    x = (W - tw) // 2
    y = 92
    text57(px, x, y, word, gap=2)
    corners(px, 3, 87, W - 4, 103, 3)
    if subtitle:
        text35(px, x + 6 * 5 + 5 * 2 + 3, y + 2, "INC.")
        s = TAGLINE
        text35(px, (W - text35_w(s)) // 2, 107, s)


TAGLINE = "STEEL FOR GOOD"


def side_ticks(px, frame):
    for y in range(22, 84, 6):
        px.point((1, y), 1)
        px.point((W - 2, y), 1)
    y = 22 + (frame * 6) % 62
    for x in (0, 1, 2):
        px.point((x, y), 1)
        px.point((W - 1 - x, 84 - (y - 22)), 1)


BAYER = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]


def compose(big, small, layer=None):
    """Threshold the supersampled line layer, put the Bayer-dithered tone layer over it, then
    the 1x layer (1 = lit, 2 = dark). White = lit."""
    lo = big.resize((W, H), Image.BOX)
    out = Image.new("1", (W, H), 0)
    lpx, opx, spx = lo.load(), out.load(), small.load()
    for y in range(H):
        for x in range(W):
            opx[x, y] = 1 if lpx[x, y] >= 128 else 0
    if layer is not None:
        tpx, mpx = layer[0].load(), layer[1].load()
        for y in range(H):
            for x in range(W):
                if mpx[x, y]:
                    opx[x, y] = 1 if tpx[x, y] > (BAYER[y % 4][x % 4] + 0.5) * 16 else 0
    for y in range(H):
        for x in range(W):
            if spx[x, y]:
                opx[x, y] = 0 if spx[x, y] == 2 else 1
    return out


def blank():
    return Image.new("L", (W * SS, H * SS), 0), Image.new("L", (W, H), 0)


def new_layer(texture=False):
    """Tone layer + mask. With texture, a faint dot field fills the ring like a radar scope."""
    tone, mask = Image.new("L", (W, H), 0), Image.new("L", (W, H), 0)
    if texture:
        dt, dm = ImageDraw.Draw(tone), ImageDraw.Draw(mask)
        box = [CX - R + 6, CY - R + 6, CX + R - 6, CY + R - 6]
        dt.ellipse(box, fill=14)
        dm.ellipse(box, fill=255)
    return tone, mask


def xor_line(img, y, every=1):
    px = img.load()
    for x in range(4, W - 4, every):
        px[x, y] = 0 if px[x, y] else 1


# ---------------------------------------------------------------- scenes
def full_frame(frame, phase, scan=None, tri=True, wave_phase=0, lock=True, glitch=0):
    big, small = blank()
    layer = new_layer()
    ring(big, phase)
    draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
    sp = P(small)
    nodes(sp)
    triangle(sp, tri)
    corners(sp, 0, 0, W - 1, H - 1, 6)
    side_ticks(sp, frame)
    title_block(sp)
    waveform(sp, 124, wave_phase)
    text35(sp, 6, 129, "ENCRYPTED")
    lock_icon(sp, 54, 118, lock)
    text35(sp, 50, 129, "%02d" % ((frame * 7 + 3) % 100))
    text35(sp, 3, 3, "BNDT-01")
    img = compose(big, small, layer)
    if scan is not None:
        xor_line(img, scan, 2)
    if glitch:
        img = glitch_slices(img, glitch)
    return img


def glitch_slices(img, seed):
    out = img.copy()
    rows = [(30 + seed * 7) % 80 + 20, (55 + seed * 13) % 80 + 20]
    for i, y0 in enumerate(rows):
        h = 3 + i * 2
        band = img.crop((0, y0, W, y0 + h))
        dx = 3 if i == 0 else -4
        out.paste(Image.new("1", (W, h), 0), (0, y0))
        out.paste(band, (dx, y0))
    return out


def idle():
    frames = []
    for f in range(16):
        frames.append(full_frame(f, phase=f * 360.0 / 48, scan=24 + (f * 4) % 60,
                                 tri=f % 4 < 2, wave_phase=f * 3))
    return frames, 4, list(range(16))


def boot():
    frames = []
    # 0-1: cursor blink
    for on in (1, 0):
        big, small = blank()
        layer = new_layer()
        sp = P(small)
        text35(sp, 3, 3, ">" if on else " ")
        if on:
            for x in range(7, 10):
                sp[x, 7] = 1
        frames.append(compose(big, small, layer))
    # 2: corners + id
    big, small = blank()
    layer = new_layer()
    sp = P(small)
    corners(sp, 0, 0, W - 1, H - 1, 6)
    text35(sp, 3, 3, "BNDT-01")
    frames.append(compose(big, small, layer))
    # 3-8: ring sweeps in
    for k in range(1, 7):
        big, small = blank()
        layer = new_layer()
        ring(big, 0, sweep=60 * k)
        sp = P(small)
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "BNDT-01")
        if k == 6:
            nodes(sp)
        frames.append(compose(big, small, layer))
    # 9-13: fox builds up
    for k in range(1, len(PARTS) + 1):
        big, small = blank()
        layer = new_layer()
        ring(big, 0)
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE, PARTS[:k])
        sp = P(small)
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "BNDT-01")
        nodes(sp)
        frames.append(compose(big, small, layer))
    # 14-19: title types in
    for k in range(1, 7):
        big, small = blank()
        layer = new_layer()
        ring(big, 0)
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
        sp = P(small)
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "BNDT-01")
        nodes(sp)
        triangle(sp)
        title_block(sp, k, subtitle=k == 6)
        frames.append(compose(big, small, layer))
    # 20: full HUD, 21: glitch
    frames.append(full_frame(0, 0))
    frames.append(full_frame(0, 0, glitch=1))
    order = list(range(21)) + [20] * 10 + [21, 20, 20, 20, 21] + [20] * 6
    return frames, 4, order


def lock():
    frames = []
    for k in range(7):
        big, small = blank()
        layer = new_layer()
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
        sp = P(small)
        m = 22 - k * 3  # reticle margin closing in
        corners(sp, CX - 27 - m // 2 - 2, 20 - m // 2, CX + 27 + m // 2 + 2, 84 + m // 2, 5)
        for i in range(-2, 3):  # crosshair ticks
            sp[CX + i, 16 - m // 3] = 1
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "TRK %02d" % (k * 14 + 9))
        s = "ACQUIRING"[: 3 + k]
        text35(sp, 6, 98, s)
        text35(sp, 6, 106, "%d%%" % min(99, k * 16 + 4))
        for x in range(6, 6 + k * 8):
            sp[x, 114] = 1
        frames.append(compose(big, small, layer))
    for k in range(2):
        big, small = blank()
        layer = new_layer()
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
        sp = P(small)
        corners(sp, CX - 29, 19, CX + 29, 85, 7)
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "TRK 99")
        if k == 0:
            text35(sp, (W - text35_w("TARGET")) // 2, 96, "TARGET")
            text35(sp, (W - text35_w("LOCKED")) // 2, 103, "LOCKED")
        lock_icon(sp, CX - 2, 112)
        for x in range(6, 62):
            sp[x, 122] = 1
        text35(sp, 6, 126, "BANDIT")
        text35(sp, 46, 126, "100%")
        img = compose(big, small, layer)
        if k == 0:
            img = invert_box(img, 4, 94, W - 5, 109)
        frames.append(img)
    order = list(range(7)) + [7, 8] * 4 + [7] * 4
    return frames, 4, order


def invert_box(img, x0, y0, x1, y1):
    px = img.load()
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            px[x, y] = 0 if px[x, y] else 1
    return img


def write_anim(outdir, name, frames, fps, order):
    d = os.path.join(outdir, name)
    os.makedirs(d, exist_ok=True)
    for i, f in enumerate(frames):
        # Lit pixels are saved black: flipper_art stores white as "background" bits, and the
        # keyboard (main since 796017e) shows those dark, so black here comes out lit.
        ImageOps.invert(f.convert("L")).save(os.path.join(d, "frame_%d.png" % i))
    with open(os.path.join(d, "meta.txt"), "w", newline="\n") as m:
        m.write("Filetype: Flipper Animation\nVersion: 1\n\n")
        m.write("Width: %d\nHeight: %d\nPassive frames: %d\nActive frames: 0\n" % (W, H, len(order)))
        m.write("Frames order: %s\n" % " ".join(map(str, order)))
        m.write("Active cycles: 0\nFrame rate: %d\nDuration: 3600\nActive cooldown: 0\n" % max(fps, 1))




# ---------------------------------------------------------------- more scenes
def frame_parts():
    big, small = blank()
    return big, small, new_layer(), P(small)


def hud_chrome(sp, label):
    corners(sp, 0, 0, W - 1, H - 1, 6)
    text35(sp, 3, 3, label)


def centered(sp, y, s):
    text35(sp, (W - text35_w(s)) // 2, y, s)


def hline(sp, x0, x1, y, step=1):
    for x in range(x0, x1 + 1, step):
        sp[x, y] = 1


def vline(sp, x, y0, y1, step=1):
    for y in range(y0, y1 + 1, step):
        sp[x, y] = 1


def rect(sp, x0, y0, x1, y1, fill=False):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            if fill or y in (y0, y1) or x in (x0, x1):
                sp[x, y] = 1


def small_fox(layer, cx, top, width, eyes=True):
    """Fox scaled to width, ear tips at y = top."""
    scale = width / FOX_W
    draw_fox(layer, cx - width / 2, top - 80 * scale, scale, eyes=eyes)


def emblem():
    """The title card with a light glint sweeping across the fox."""
    frames = []
    for f in range(12):
        big, small, layer, sp = frame_parts()
        ring(big, 0)
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
        if f < 8:  # glint: a diagonal band turns the dithered fur solid
            tpx = layer[0].load()
            b = -40 + f * 14
            for y in range(H):
                for x in range(W):
                    if 0 <= (x + y) - (b + 60) < 5 and tpx[x, y] == ORANGE:
                        tpx[x, y] = 255
        nodes(sp)
        triangle(sp, f % 6 < 3)
        corners(sp, 0, 0, W - 1, H - 1, 6)
        side_ticks(sp, f)
        title_block(sp)
        text35(sp, 3, 3, "BNDT-01")
        centered(sp, 120, "EST. MMXXVI")
        for i in range(3):  # three status pips, one lit per beat
            rect(sp, 26 + i * 6, 130, 29 + i * 6, 133, fill=(i == f % 3))
        frames.append(compose(big, small, layer))
    return frames, 4, list(range(12))


def radar():
    frames = []
    rcx, rcy, rr = 34, 48, 30
    blips = [(48, 36, 50), (22, 58, 200), (40, 66, 140), (20, 30, 300)]  # x, y, bearing
    n = 12
    for f in range(n):
        big, small, layer, sp = frame_parts()
        d = ImageDraw.Draw(big)
        for r in (rr, 20, 10):
            box = [(rcx - r) * SS, (rcy - r) * SS, (rcx + r) * SS, (rcy + r) * SS]
            if r == rr:
                d.ellipse(box, outline=255, width=SS)
            else:
                for a in range(0, 360, 12):
                    d.arc(box, a, a + 4, fill=255, width=SS)
        ang = f * 360 / n  # sweep bearing, 0 = north, clockwise
        tone, mask = layer  # trailing wedge, dithered, behind the sweep line
        dt, dm = ImageDraw.Draw(tone), ImageDraw.Draw(mask)
        box1 = [rcx - rr + 1, rcy - rr + 1, rcx + rr - 1, rcy + rr - 1]
        dt.pieslice(box1, ang - 90 - 50, ang - 90, fill=40)
        dm.pieslice(box1, ang - 90 - 50, ang - 90, fill=255)
        a = math.radians(ang - 90)
        d.line([(rcx * SS, rcy * SS), ((rcx + rr * math.cos(a)) * SS, (rcy + rr * math.sin(a)) * SS)],
               fill=255, width=SS)
        hline(sp, rcx - rr - 3, rcx + rr + 3, rcy, 3)
        vline(sp, rcx, rcy - rr - 3, rcy + rr + 3, 3)
        lit = 0
        for x, y, bearing in blips:
            age = (ang - bearing) % 360
            if age < 120:  # bright just after the sweep passes, then fades
                lit += 1
                size = 1 if age < 60 else 0
                for dx in range(-size, size + 1):
                    for dy in range(-size, size + 1):
                        sp[x + dx, y + dy] = 1
                if age < 30:
                    corners(sp, x - 3, y - 3, x + 3, y + 3, 2)
        hud_chrome(sp, "SCAN %03d" % int(ang))
        centered(sp, 84, "SCANNING" + "." * (f % 4))
        text35(sp, 6, 93, "CONTACTS")
        text35(sp, 54, 93, "%02d" % lit)
        hline(sp, 4, W - 5, 100, 2)
        small_fox(layer, 34, 104, 24)
        text35(sp, 4, 131, "BANDIT")
        text35(sp, 44, 131, "RNG 4K")
        frames.append(compose(big, small, layer))
    return frames, 4, list(range(n))


def noise_band(img, seed, y0, y1):
    rnd = random.Random(seed)
    px = img.load()
    for y in range(y0, y1):
        if rnd.random() < 0.35:
            continue
        for x in range(W):
            px[x, y] = 1 if rnd.random() < 0.45 else 0
    return img


def signal():
    """Signal drops to static, 'SIGNAL LOST', searches, and the fox comes back."""
    base = full_frame(0, 0)
    frames = [base, glitch_slices(base, 2), glitch_slices(base, 5)]
    for k in range(3):  # 3-5: static
        frames.append(noise_band(glitch_slices(base, 7 + k), 100 + k, 10 + k * 9, 110 - k * 12))
    for k in range(2):  # 6-7: SIGNAL LOST, blinking
        big, small, layer, sp = frame_parts()
        hud_chrome(sp, "CH 07")
        rect(sp, 6, 52, W - 7, 76)
        if k == 0:
            centered(sp, 58, "SIGNAL")
            centered(sp, 66, "LOST")
        frames.append(compose(big, small, layer))
    for k in range(5):  # 8-12: searching, bars climb
        big, small, layer, sp = frame_parts()
        hud_chrome(sp, "CH 07")
        centered(sp, 46, "SEARCHING")
        for i in range(5):
            h = 4 + i * 4
            x = 18 + i * 7
            rect(sp, x, 80 - h, x + 4, 80, fill=i <= k)
        centered(sp, 88, "%d%%" % (k * 25))
        frames.append(compose(big, small, layer))
    for k in range(1, 4):  # 13-15: the fox scans back in, top to bottom
        img = base.copy()
        cut = k * H // 4
        img.paste(Image.new("1", (W, H - cut), 0), (0, cut))
        xor_line(img, cut)
        frames.append(img)
    order = ([0] * 6 + [1, 0, 2] + [3, 4, 5, 6, 7, 6, 7, 6, 7] + list(range(8, 13))
             + [13, 14, 15] + [0] * 4)
    return frames, 4, order


HEX = "0123456789ABCDEF"


def decrypt():
    """Hex rows resolve one by one into the motto, then ACCESS GRANTED."""
    frames = []
    rnd = random.Random(7)
    target = ["STEEL", "FOR", "GOOD", "", "BANDIT", "INC.", "", "OK"]
    n = 14
    for f in range(n):
        big, small, layer, sp = frame_parts()
        hud_chrome(sp, "DECRYPT")
        for r, word in enumerate(target):
            y = 14 + r * 8
            if f >= 4 + r:  # rows resolve one after another
                text35(sp, 6, y, word)
                text35(sp, 50, y, "OK" if word else "--")
            else:
                text35(sp, 6, y, "".join(rnd.choice(HEX) for _ in range(6)))
                text35(sp, 34, y, "".join(rnd.choice(HEX) for _ in range(2)))
                if (f + r) % 3 == 0:
                    sp[60, y + 2] = 1
        pct = min(100, int(100 * (f + 1) / (n - 2)))
        hline(sp, 4, W - 5, 82, 2)
        rect(sp, 4, 86, W - 5, 91)
        for x in range(5, 5 + (W - 10) * pct // 100):
            for y in (87, 88, 89, 90):
                sp[x, y] = 1 if (x + y) % 2 or pct == 100 else 0
        done = f >= n - 2
        if done:
            small_fox(layer, 34, 96, 26)
        else:
            centered(sp, 100, "KEY %02d/%02d" % (f + 1, n - 2))
            waveform(sp, 120, f * 5, 6, 62)
        img = compose(big, small, layer)
        if done:
            cap = Image.new("L", (W, H), 0)
            centered(P(cap), 127, "ACCESS GRANTED")
            inv = f == n - 2
            if inv:
                invert_box(img, 3, 125, W - 4, 133)
            px, cpx = img.load(), cap.load()
            for y in range(125, 134):
                for x in range(W):
                    if cpx[x, y]:
                        px[x, y] = 0 if inv else 1
        frames.append(img)
    order = list(range(n - 1)) + [n - 2, n - 1] * 3 + [n - 1] * 6
    return frames, 4, order


def uplink():
    """Courier route: a packet hops node to node down the screen until the link is up."""
    nodes_xy = [(12, 18), (52, 34), (20, 52), (48, 70), (16, 88)]
    path = []
    for (x0, y0), (x1, y1) in zip(nodes_xy, nodes_xy[1:]):
        for t in range(4):
            path.append((x0 + (x1 - x0) * t / 4, y0 + (y1 - y0) * t / 4))
    path.append(nodes_xy[-1])
    frames = []
    for f, (qx, qy) in enumerate(path):
        big, small, layer, sp = frame_parts()
        hud_chrome(sp, "UPLINK")
        reached = min(len(nodes_xy) - 1, f // 4 + (1 if f == len(path) - 1 else 0))
        for i, ((x0, y0), (x1, y1)) in enumerate(zip(nodes_xy, nodes_xy[1:])):
            steps = int(max(abs(x1 - x0), abs(y1 - y0)))
            for t in range(0, steps, 2 if i >= reached else 1):
                sp[int(round(x0 + (x1 - x0) * t / steps)), int(round(y0 + (y1 - y0) * t / steps))] = 1
        for i, (x, y) in enumerate(nodes_xy):
            for dx in range(-4, 5):  # clear around the node
                for dy in range(-4, 5):
                    sp[x + dx, y + dy] = 2
            rect(sp, x - 3, y - 3, x + 3, y + 3, fill=i <= reached)
            if i <= reached:
                sp[x, y] = 2
            text35(sp, x + 6 if x < 34 else x - 15, y - 2, "N%d" % (i + 1))
        qx, qy = int(qx), int(qy)
        rect(sp, qx - 1, qy - 1, qx + 1, qy + 1, fill=True)
        corners(sp, qx - 4, qy - 4, qx + 4, qy + 4, 2)
        pct = int(100 * f / (len(path) - 1))
        hline(sp, 4, W - 5, 100, 2)
        if f == len(path) - 1:
            centered(sp, 106, "LINK UP")
            centered(sp, 116, "STEEL FOR GOOD")
        else:
            centered(sp, 106, "RELAY %d%%" % pct)
            rect(sp, 8, 116, W - 9, 120)
            for x in range(9, 9 + (W - 18) * pct // 100):
                for y in (117, 118, 119):
                    sp[x, y] = 1
        text35(sp, 4, 131, "BANDIT")
        text35(sp, 52, 131, "%03d" % (f * 37 % 1000))
        frames.append(compose(big, small, layer))
    n = len(frames)
    return frames, 4, list(range(n)) + [n - 1] * 6


def ecg(x):
    """One heartbeat every 32 px."""
    return {10: -1, 11: -2, 12: 1, 13: -10, 14: 6, 15: 2, 20: -1, 21: -2, 22: -2, 23: -1}.get(x % 32, 0)


HEART = [(54, 92), (56, 92), (53, 93), (54, 93), (55, 93), (56, 93), (57, 93), (54, 94), (55, 94),
         (56, 94), (55, 95)]


def vitals():
    frames = []
    n = 8
    for f in range(n):
        big, small, layer, sp = frame_parts()
        hud_chrome(sp, "VITALS")
        small_fox(layer, 34, 12, 30)
        centered(sp, 48, "AGENT BANDIT")
        rect(sp, 3, 56, W - 4, 86)
        for x in range(4, W - 4, 4):
            for y in range(60, 86, 4):
                sp[x, y] = 1
        prev = None
        for x in range(4, W - 4):
            y = 74 + ecg(x + f * 4)
            if prev is not None:
                for yy in range(min(prev, y), max(prev, y) + 1):
                    sp[x, yy] = 1
            prev = y
        text35(sp, 4, 92, "BPM")
        text35(sp, 20, 92, "%d" % [62, 63, 63, 64, 64, 63, 62, 62][f])
        if f % 4 in (0, 1):
            for xy in HEART:
                sp[xy] = 1
        text35(sp, 4, 102, "STRESS")
        for i in range(8):
            rect(sp, 32 + i * 4, 102, 34 + i * 4, 106, fill=i < 2)
        text35(sp, 4, 112, "O2  98%")
        text35(sp, 4, 122, "STATUS")
        text35(sp, 32, 122, "CALM")
        frames.append(compose(big, small, layer))
    return frames, 4, list(range(n))


def topo_field(x, y):
    return (math.sin(x * 0.11 + 1.3) * math.cos(y * 0.09) + 0.6 * math.sin((x + y) * 0.07)
            + 0.4 * math.cos(x * 0.05 - y * 0.13 + 2.0))


def topo():
    """Contour map with a marker walking a dotted route to the objective."""
    frames = []
    x0, y0, x1, y1 = 3, 12, W - 4, 92
    contours = []
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            a = int(topo_field(x, y) * 4)
            if a != int(topo_field(x + 1, y) * 4) or a != int(topo_field(x, y + 1) * 4):
                contours.append((x, y))
    route = [(10, 84), (18, 70), (30, 64), (36, 50), (48, 42), (54, 28), (46, 18)]
    pts = []
    for (ax, ay), (bx, by) in zip(route, route[1:]):
        for t in range(3):
            pts.append((ax + (bx - ax) * t / 3, ay + (by - ay) * t / 3))
    pts.append(route[-1])
    for f, (mx, my) in enumerate(pts):
        big, small, layer, sp = frame_parts()
        for x, y in contours:  # contours dotted, so the route reads on top
            if (x + y) % 2 == 0:
                sp[x, y] = 1
        rect(sp, x0, y0, x1, y1)
        for (ax, ay), (bx, by) in zip(route, route[1:]):  # dashed route on a cleared path
            steps = int(max(abs(bx - ax), abs(by - ay)))
            for t in range(steps):
                x, y = int(ax + (bx - ax) * t / steps), int(ay + (by - ay) * t / steps)
                for dx in (-1, 0, 1):
                    for dy in (-1, 0, 1):
                        sp[x + dx, y + dy] = 2
            for t in range(steps):
                if t % 4 != 3:
                    x, y = int(ax + (bx - ax) * t / steps), int(ay + (by - ay) * t / steps)
                    sp[x, y] = 1
                    sp[x + 1, y] = 1
        ex, ey = route[-1]
        for i in range(-3, 4):  # objective X
            sp[ex + i, ey + i] = 1
            sp[ex + i, ey - i] = 1
        mx, my = int(mx), int(my)
        for dx in range(-5, 6):  # marker, cut out of the map so it stands clear
            for dy in range(-5, 6):
                sp[mx + dx, my + dy] = 2
        rect(sp, mx - 3, my - 3, mx + 3, my + 3)
        rect(sp, mx - 1, my - 1, mx + 1, my + 1, fill=f % 2 == 0)
        corners(sp, mx - 5, my - 5, mx + 5, my + 5, 2)
        hud_chrome(sp, "ROUTE")
        text35(sp, 36, 3, "WP %d/%d" % (min(len(route), f // 3 + 1), len(route)))
        text35(sp, 4, 98, "N 47.%02d" % (60 + f))
        text35(sp, 4, 106, "W 122.%02d" % (33 - f))
        text35(sp, 4, 116, "DIST %dM" % ((len(pts) - 1 - f) * 120))
        hline(sp, 4, W - 5, 124, 2)
        text35(sp, 4, 130, "ON FOOT" if f < len(pts) - 1 else "ARRIVED")
        frames.append(compose(big, small, layer))
    n = len(frames)
    return frames, 4, list(range(n)) + [n - 1] * 4


def dossier():
    """Agent file: photo with a scan line, fields typing in, a redaction that flickers open."""
    frames = []
    rows = [("AGENT", "BANDIT"), ("STATUS", "ACTIVE"), ("CREW", "INC.")]
    n = 12
    for f in range(n):
        big, small, layer, sp = frame_parts()
        hud_chrome(sp, "FILE 01")
        rect(sp, 14, 11, 53, 50)
        small_fox(layer, 34, 15, 34)
        img_scan = 13 + (f * 3) % 36
        y = 56
        for i, (k, v) in enumerate(rows):
            text35(sp, 4, y, k)
            shown = v[: max(0, f * 2 - i * 3)]
            text35(sp, 32, y, shown)
            if len(shown) < len(v) and f % 2 == 0:
                hline(sp, 32 + len(shown) * 4, 34 + len(shown) * 4, y + 5)
            y += 9
        if f >= 6:
            centered(sp, y + 2, "STEEL FOR GOOD")
        y += 12
        for i in range(3):  # redacted lines; the middle one flickers open
            if i == 1 and f in (9, 10):
                text35(sp, 4, y, "TRUST THE DEN")
            else:
                rect(sp, 4, y, 4 + [56, 50, 44][i], y + 4, fill=True)
            y += 8
        rect(sp, 46, 126, 63, 134)
        text35(sp, 48, 128, "TOP")
        img = compose(big, small, layer)
        xor_line_span(img, img_scan, 15, 52)
        frames.append(img)
    return frames, 4, list(range(n)) + [n - 1] * 4


def xor_line_span(img, y, x0, x1):
    px = img.load()
    for x in range(x0, x1 + 1, 2):
        px[x, y] = 0 if px[x, y] else 1


def standby():
    """Low-power look: slow ring, a breathing pip and a lazy blink."""
    frames = []
    n = 12
    for f in range(n):
        big, small, layer, sp = frame_parts()
        ring(big, f * 2.5, dashes=12)
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE, eyes=f not in (7, 8))
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "BNDT-01")
        centered(sp, 96, "STANDBY")
        r = [0, 1, 2, 3, 2, 1][f % 6]
        for dx in range(-r, r + 1):
            for dy in range(-r, r + 1):
                if dx * dx + dy * dy <= r * r:
                    sp[34 + dx, 114 + dy] = 1
        centered(sp, 126, "Z" * (1 + f // 4))
        frames.append(compose(big, small, layer))
    return frames, 2, list(range(n))


def fox_outline():
    """Union of the fox's outer shapes, for silhouettes."""
    return [EAR, mirror(EAR), FACE, CHEEK, mirror(CHEEK)]


def skyline():
    """Night city, full bleed: stars, a moon with the fox in silhouette, city glow, a far and a
    near skyline, a searchlight sweeping and windows flickering. Three tones only (black, 50%
    checker, lit), depth from layering rather than outlines."""
    rnd = random.Random(11)
    far, x = [], 0
    while x < W:
        w = rnd.randint(4, 8)
        far.append((x, w, rnd.randint(62, 80)))
        x += w
    near, x = [], -2
    while x < W:
        w = rnd.randint(7, 13)
        near.append((x, w, rnd.randint(88, 112)))
        x += w + rnd.randint(0, 2)
    tall = max(range(len(near)), key=lambda i: -near[i][2])  # tallest gets the searchlight
    windows = []
    for bx, bw, top in near:
        for wy in range(top + 4, 132, 4):
            for wx in range(bx + 2, bx + bw - 1, 3):
                if rnd.random() < 0.45:
                    windows.append((wx, wy))
    stars = [(rnd.randrange(W), rnd.randrange(2, 60)) for _ in range(28)]
    frames = []
    n = 16
    for f in range(n):
        tone = Image.new("L", (W, H), 0)
        mask = Image.new("L", (W, H), 255)
        d = ImageDraw.Draw(tone)
        # city glow low on the horizon
        d.rectangle([0, 84, W, 100], fill=40)
        # searchlight from the tallest roof, swinging back and forth
        bx, bw, top = near[tall]
        ox, oy = bx + bw // 2, top - 3
        a = math.radians(-90 + 55 * math.sin(f * 2 * math.pi / n))
        spread = math.radians(5)
        tip = lambda t: (ox + 120 * math.cos(a + t), oy + 120 * math.sin(a + t))
        d.polygon([(ox, oy), tip(-spread), tip(spread)], fill=128)
        # moon with the fox cut out of it
        mx, my, mr = 44, 26, 15
        d.ellipse([mx - mr, my - mr, mx + mr, my + mr], fill=255)
        scale = 22 / FOX_W
        for p in fox_outline():
            d.polygon([(mx - 11 + x * scale, my - 13 + y * scale) for x, y in p], fill=0)
        for p in (EYE, mirror(EYE)):
            d.polygon([(mx - 11 + x * scale, my - 13 + y * scale) for x, y in p], fill=255)
        # far skyline: a dim 50% mass; near skyline: black blocks with a lit roof and left edge
        for x0, w, t in far:
            d.rectangle([x0, t, x0 + w - 1, H], fill=128)
        for x0, w, t in near:
            d.rectangle([x0, t, x0 + w - 1, H], fill=0)
            d.line([(x0, t), (x0 + w - 1, t)], fill=255)
            d.line([(x0, t), (x0, 131)], fill=255)
        # street with a car's lights driving across
        d.rectangle([0, 132, W, H], fill=0)
        d.line([(0, 132), (W, 132)], fill=255)
        cx = (f * 6) % (W + 20) - 10
        d.rectangle([cx, 134, cx + 7, 136], fill=255)
        d.line([(cx + 8, 135), (cx + 14, 135)], fill=128)
        d.line([(ox, top - 1), (ox, top - 6)], fill=255)  # antenna
        tpx = tone.load()
        if f % 4 < 2:
            tpx[ox, top - 7] = 255  # blinking aircraft light
        for i, (wx, wy) in enumerate(windows):
            if (i * 7 + f // 2) % 23 != 0:  # a few windows switch off now and then
                tpx[wx, wy] = 255
        for i, (sx, sy) in enumerate(stars):
            if tpx[sx, sy] == 0 and (i + f) % 9 != 0:  # twinkle
                tpx[sx, sy] = 255 if i % 3 else 128
        # billboard on a mid roof
        bx2, bw2, top2 = near[(tall + 2) % len(near)]
        d.rectangle([bx2 - 2, top2 - 10, bx2 + 24, top2 - 3], fill=0)
        d.rectangle([bx2 - 2, top2 - 10, bx2 + 24, top2 - 3], outline=255)
        d.line([(bx2 + 2, top2 - 3), (bx2 + 2, top2)], fill=255)
        d.line([(bx2 + 20, top2 - 3), (bx2 + 20, top2)], fill=255)
        big, small = blank()
        sp = P(small)
        if f % 8 != 7:  # the sign flickers once per loop
            text35(sp, bx2 + 1, top2 - 8, "BANDIT")
        frames.append(compose(big, small, (tone, mask)))
    return frames, 4, list(range(n))


def gem():
    """A faceted crystal of our own (replaces the borrowed Gem crystal, foxglove#110): a hexagonal
    bipyramid turning in 3D, lit from the upper left, bobbing over its shadow, glints on the tips."""
    frames = []
    n = 16
    cx, cy, s = 34, 60, 25
    light = (-0.55, -0.6, 0.58)
    ring = [(math.cos(k * math.pi / 3), 0.0, math.sin(k * math.pi / 3)) for k in range(6)]
    top, bot = (0.0, -1.15, 0.0), (0.0, 1.7, 0.0)
    faces = [(top, ring[k], ring[(k + 1) % 6]) for k in range(6)] + [(bot, ring[(k + 1) % 6], ring[k]) for k in range(6)]
    for f in range(n):
        a = f * (math.pi / 3) / n  # 60 degrees per loop: the gem's own symmetry makes it seamless
        bob = math.sin(f * 2 * math.pi / n) * 3
        rot = lambda p: (p[0] * math.cos(a) + p[2] * math.sin(a), p[1], -p[0] * math.sin(a) + p[2] * math.cos(a))
        big, small, layer, sp = frame_parts()
        tone, mask = layer
        dt, dm = ImageDraw.Draw(tone), ImageDraw.Draw(mask)
        # shadow, smaller when the gem rises
        sw = 18 - bob
        dt.ellipse([cx - sw, 112, cx + sw, 118], fill=48); dm.ellipse([cx - sw, 112, cx + sw, 118], fill=255)
        drawn = []
        for tri in faces:
            p = [rot(v) for v in tri]
            u = [p[1][i] - p[0][i] for i in range(3)]
            v = [p[2][i] - p[0][i] for i in range(3)]
            nrm = (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0])
            ln = math.sqrt(sum(c * c for c in nrm)) or 1
            nrm = tuple(c / ln for c in nrm)
            if nrm[2] >= 0:  # facing away
                continue
            shade = max(0.0, -(nrm[0] * light[0] + nrm[1] * light[1] + nrm[2] * light[2]))
            pts = [(cx + q[0] * s, cy + bob + q[1] * s) for q in p]
            drawn.append((sum(q[2] for q in p), pts, shade))
        for _, pts, shade in sorted(drawn, reverse=True):
            dt.polygon(pts, fill=int(40 + 215 * shade ** 1.4)); dm.polygon(pts, fill=255)
        for _, pts, _ in drawn:  # dark edges keep the facets apart
            dt.line(pts + pts[:1], fill=0, width=1); dm.line(pts + pts[:1], fill=255, width=1)
        # glints travel round the tips
        for k, (gx, gy) in enumerate(((cx, cy + bob - 1.15 * s), (cx + s, cy + bob), (cx - s, cy + bob))):
            if (f + k * 5) % 16 < 3:
                for d in range(-3, 4):
                    sp[int(gx) + d, int(gy)] = 1
                    sp[int(gx), int(gy) + d] = 1
        for k in range(14):  # a few fixed stars
            if (k + f) % 7:
                sp[(k * 29 + 5) % W, (k * 17 + 3) % 30 + 4] = 1
        hud_chrome(sp, "BNDT-01")
        centered(sp, 128, "FACET")
        frames.append(compose(big, small, layer))
    return frames, 4, list(range(n))


def prism():
    """The fox emblem set in a diamond, a glint sweeping across it, the frame's facets catching
    the light in turn."""
    frames = []
    n = 16
    top, right, bottom, left = (34, 14), (64, 66), (34, 124), (4, 66)
    for f in range(n):
        big, small, layer, sp = frame_parts()
        d = ImageDraw.Draw(big)
        S = SS
        poly = [(x * S, y * S) for x, y in (top, right, bottom, left)]
        d.polygon(poly, outline=255, width=2 * S)
        inner = [(34, 26), (54, 66), (34, 112), (14, 66)]
        d.polygon([(x * S, y * S) for x, y in inner], outline=255, width=S)
        for (ox, oy), (ix, iy) in zip((top, right, bottom, left), inner):  # facet lines
            d.line([(ox * S, oy * S), (ix * S, iy * S)], fill=255, width=S)
        small_fox(layer, 34, 46, 34)
        img = compose(big, small, layer)
        # the glint: a narrow band that brightens everything it crosses, then a pause
        if f < 10:
            b = -40 + f * 16
            px = img.load()
            for y in range(H):
                for x in range(W):
                    if 0 <= (x + y) - (b + 70) < 4 and px[x, y] == 0 and (x + y) % 2 == 0:
                        px[x, y] = 1
        sp2 = P(img)
        k = f % 4  # facet that catches the light
        (ox, oy), (ix, iy) = list(zip((top, right, bottom, left), inner))[k]
        for t in range(0, 10, 2):
            sp2[ox + (ix - ox) * t // 10, oy + (iy - oy) * t // 10] = 1
        if f % 8 < 2:  # star on the top point
            for dd in range(-3, 4):
                sp2[34 + dd, 10] = 1
                sp2[34, 10 + dd] = 1
        frames.append(img)
    return frames, 4, list(range(n))


def orbit():
    """The fox mask at the centre of three tilted orbits, a node riding each one (passing behind
    the fox on the far side), under a slow starfield."""
    frames = []
    n = 24
    cx, cy = 34, 64
    orbits = [(0.0, 1), (math.pi / 3, 2), (-math.pi / 3, 3)]  # tilt, speed (integer: seamless)
    for f in range(n):
        big, small, layer, sp = frame_parts()
        d = ImageDraw.Draw(big)
        for k in range(22):
            if (k * 3 + f) % 9:
                sp[(k * 31 + f // 6) % W, (k * 47) % (H - 20) + 10] = 1
        nodes_back, nodes_front = [], []
        for tilt, speed in orbits:
            for t in range(0, 360, 6):  # dotted orbit
                a = math.radians(t)
                x, y = 30 * math.cos(a), 9 * math.sin(a)
                rx, ry = x * math.cos(tilt) - y * math.sin(tilt), x * math.sin(tilt) + y * math.cos(tilt)
                d.point(((cx + rx) * SS, (cy + ry) * SS), fill=0)
                sp[int(cx + rx), int(cy + ry)] = 1
            a = f * 2 * math.pi * speed / n
            x, y = 30 * math.cos(a), 9 * math.sin(a)
            rx, ry = x * math.cos(tilt) - y * math.sin(tilt), x * math.sin(tilt) + y * math.cos(tilt)
            (nodes_front if math.sin(a) > 0 else nodes_back).append((int(cx + rx), int(cy + ry)))
        for x, y in nodes_back:
            rect(sp, x - 1, y - 1, x + 1, y + 1, fill=True)
        small_fox(layer, cx, cy - 15, 30)
        img = compose(big, small, layer)
        sp2 = P(img)
        for x, y in nodes_front:
            for dx in range(-3, 4):
                for dy in range(-3, 4):
                    sp2[x + dx, y + dy] = 0
            rect(sp2, x - 2, y - 2, x + 2, y + 2, fill=True)
        frames.append(img)
    return frames, 5, list(range(n))


def focus():
    """For the Pomodoro plugin: the ring fills like a timer dial while the fox keeps watch."""
    frames = []
    box = [(CX - R) * SS, (CY - R) * SS, (CX + R) * SS, (CY + R) * SS]
    for f in range(12):
        big, small, layer, sp = frame_parts()
        ring(big, 0)
        d = ImageDraw.Draw(big)
        if f:  # the filled sweep, one twelfth more each frame
            d.arc(box, -90, -90 + f * 30, fill=255, width=3 * SS)
        a = math.radians(f * 30 - 90)  # the hand's tip, a bright node on the ring
        x, y = (CX + R * math.cos(a)) * SS, (CY + R * math.sin(a)) * SS
        d.ellipse([x - 2.5 * SS, y - 2.5 * SS, x + 2.5 * SS, y + 2.5 * SS], fill=255)
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "BNDT-01")
        text35(sp, W - 3 - text35_w("T-25"), 3, "T-25")
        word = "FOCUS"
        text57(sp, (W - (5 * 5 + 4 * 2)) // 2, 92, word, gap=2)
        corners(sp, 3, 87, W - 4, 103, 3)
        centered(sp, 107, "DO NOT DISTURB")
        for i in range(4):  # four rounds; the current one blinks
            rect(sp, 23 + i * 6, 126, 26 + i * 6, 129, fill=(i == 0 and f % 2 == 0))
        frames.append(compose(big, small, layer))
    return frames, 2, list(range(12))


def onair():
    """For the Broadcast Status plugin: signal rings pulse out behind the fox, ON AIR blinks."""
    frames = []
    for f in range(8):
        big, small, layer, sp = frame_parts()
        d = ImageDraw.Draw(big)
        for k in range(3):  # three rings, each moving out one step per frame
            r = 10 + ((f + k * 8 / 3) % 8) * 3.0
            box = [(CX - r) * SS, (CY - r) * SS, (CX + r) * SS, (CY + r) * SS]
            d.ellipse(box, outline=255, width=SS + (SS if r < 20 else 0))
        draw_fox(layer, FOX_OX, FOX_OY, FOX_SCALE)
        corners(sp, 0, 0, W - 1, H - 1, 6)
        text35(sp, 3, 3, "BNDT-01")
        text35(sp, W - 3 - text35_w("CH-01"), 3, "CH-01")
        x = (W - (12 + 6 + 19)) // 2  # "ON" (12 px), a 6 px space, "AIR" (19 px)
        text57(sp, x, 92, "ON", gap=2)
        text57(sp, x + 18, 92, "AIR", gap=2)
        corners(sp, 3, 87, W - 4, 103, 3)
        if f % 2 == 0:  # the tally light
            rect(sp, 6, 93, 9, 97, fill=True)
            rect(sp, W - 10, 93, W - 7, 97, fill=True)
        centered(sp, 107, "SIGNAL STRONG")
        for i in range(5):  # a level meter that walks up and down
            h = [3, 5, 7, 5, 3][(i + f) % 5]
            rect(sp, 24 + i * 4, 133 - h, 26 + i * 4, 133, fill=True)
        frames.append(compose(big, small, layer))
    return frames, 4, list(range(8))


SCENES = [
    ("Bandit_boot", boot), ("Bandit_idle", idle), ("Bandit_lock", lock), ("Bandit_emblem", emblem),
    ("Bandit_radar", radar), ("Bandit_signal", signal), ("Bandit_decrypt", decrypt),
    ("Bandit_uplink", uplink), ("Bandit_vitals", vitals), ("Bandit_route", topo),
    ("Bandit_dossier", dossier), ("Bandit_standby", standby), ("Bandit_skyline", skyline),
    ("Bandit_gem", gem), ("Bandit_prism", prism), ("Bandit_orbit", orbit),
    ("Bandit_focus", focus),
    ("Bandit_onair", onair),
]


def main():
    out = sys.argv[1]
    only = set(sys.argv[2:])
    for name, fn in SCENES:
        if not only or name in only:
            write_anim(out, name, *fn())


if __name__ == "__main__":
    main()
