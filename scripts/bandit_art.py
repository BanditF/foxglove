"""Bandit HUD art for the right-hand nice!view (68x140 portrait, 1-bit).

Writes animation folders (meta.txt + frame_N.png) that scripts/flipper_art.py turns into art sets:
  python scripts/bandit_art.py OUTDIR
Original art based on the user's Bandit fox motif.
"""

import math
import os
import sys

from PIL import Image, ImageDraw

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


def draw_fox(layer, ox, oy, scale, parts=PARTS):
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
        for p in (EYE, mirror(EYE)):
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


def still():
    return [full_frame(0, 0)], 0, [0]


def write_anim(outdir, name, frames, fps, order):
    d = os.path.join(outdir, name)
    os.makedirs(d, exist_ok=True)
    for i, f in enumerate(frames):
        # flipper_art: white = background, black = ink; our lit pixels are white on black.
        f.convert("L").save(os.path.join(d, "frame_%d.png" % i))
    with open(os.path.join(d, "meta.txt"), "w", newline="\n") as m:
        m.write("Filetype: Flipper Animation\nVersion: 1\n\n")
        m.write("Width: %d\nHeight: %d\nPassive frames: %d\nActive frames: 0\n" % (W, H, len(order)))
        m.write("Frames order: %s\n" % " ".join(map(str, order)))
        m.write("Active cycles: 0\nFrame rate: %d\nDuration: 3600\nActive cooldown: 0\n" % max(fps, 1))


def main():
    out = sys.argv[1]
    for name, fn in (("Bandit_idle", idle), ("Bandit_boot", boot), ("Bandit_lock", lock)):
        frames, fps, order = fn()
        write_anim(out, name, frames, fps, order)
    f, _, _ = still()
    f[0].convert("L").save(os.path.join(out, "Bandit_card.png"))


if __name__ == "__main__":
    main()
