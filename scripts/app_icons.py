"""Draws Foxglove's app icon from the Bandit fox motif (#31) and the tray glyphs from the
Foxglove mark (#403).

  python -I scripts/app_icons.py               rewrite the files in app/resources/ (same names as
                                               KeyPeek's, so upstream syncs don't conflict)
  python -I scripts/app_icons.py --tray        only the tray glyphs
  python -I scripts/app_icons.py --sheet x.png also a review sheet of the tray glyphs

Writes:
- icon.svg (the master), icon.png (256), icon.ico (16, 20, 24, 32, 48, 64, 256): a dark tile with
  the low-poly fox. 24 px and below use a simplified fox (silhouette, mask, no inner lines).
- tray-icon-white*.png / tray-icon-black*.png: the sidebar's mark (#395), an orange arrowhead in
  a ring, for dark / light taskbars. Plain: a keyboard is connected (solid); -off: none is (the
  outline); -paused: Foxglove is paused (#404; a grey arrowhead with an orange pause badge).
  Drawn for Windows' small-icon sizes: the plain name is 32 px, -16/-20/-24 the smaller ones.
- tray-icon-template.png (44), -off and -paused: macOS templates (black + alpha).
- packaging/identity/Assets/logo44/50/150.png for the sparse package.
"""

import argparse
import os

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RES = os.path.join(ROOT, "app", "resources")

# the Bandit fox, in its 680 x 760 crop (from art-src/bandit/bandit_art.py)
FW, FH = 680.0, 760.0
EAR = [(20, 80), (245, 300), (115, 430)]
EAR_IN = [(62, 165), (196, 305), (120, 375)]
FACE = [(238, 296), (442, 296), (608, 462), (402, 724), (278, 724), (72, 462)]
CHEEK = [(8, 466), (170, 494), (310, 748), (250, 748)]
MASK = [(70, 456), (268, 500), (300, 700), (180, 572)]
EYE = [(150, 498), (266, 528), (236, 566)]
NOSE = [(300, 704), (380, 704), (340, 752)]
FACET = [(238, 296), (232, 410), (272, 470), (272, 600)]

ORANGE, CREAM, DARK, EDGE = "#e5732b", "#f2e6d0", "#16171b", "#2c2e35"


def mirror(p):
    return [(FW - x, y) for x, y in p]


def shrink(p, f):
    cx = sum(x for x, _ in p) / len(p)
    cy = sum(y for _, y in p) / len(p)
    return [(cx + (x - cx) * f, cy + (y - cy) * f) for x, y in p]


def fox_layers(simple):
    """(polygon, colour) in paint order; simple drops the inner ear, eyes and facets."""
    out = []
    for e in (EAR, mirror(EAR)):
        out.append((e, CREAM if not simple else ORANGE))
        out.append((shrink(e, 0.8), ORANGE))
    if not simple:
        out += [(shrink(p, 0.6), DARK) for p in (EAR_IN, mirror(EAR_IN))]
    out += [(CHEEK, CREAM), (mirror(CHEEK), CREAM), (FACE, ORANGE)]
    out += [(MASK, DARK), (mirror(MASK), DARK)]
    if not simple:
        out += [(EYE, CREAM), (mirror(EYE), CREAM)]
    out.append((NOSE, DARK))
    return out


def tile_geometry(size, simple):
    """Where the fox sits in a size x size tile: (scale, ox, oy)."""
    pad = size * (0.10 if simple else 0.14)
    scale = (size - 2 * pad) / FH * (1.0 if simple else 0.98)
    ox = (size - FW * scale) / 2
    oy = (size - FH * scale) / 2 + size * 0.01
    return scale, ox, oy


def draw_icon(size):
    simple = size <= 24
    ss = 8 if size < 64 else 4
    S = size * ss
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    r = S * 0.22
    d.rounded_rectangle([0, 0, S - 1, S - 1], radius=r, fill=DARK, outline=EDGE if size >= 32 else None,
                        width=max(1, S // 128))
    scale, ox, oy = tile_geometry(S, simple)
    tr = lambda p: [(ox + x * scale, oy + y * scale) for x, y in p]
    for p, c in fox_layers(simple):
        d.polygon(tr(p), fill=c)
    if not simple:
        w = max(1, round(S / 160))
        d.line(tr([(340, 300), (340, 700)]), fill=DARK, width=w)
        for p in (FACET, mirror(FACET)):
            d.line(tr(p), fill=DARK, width=w)
    if size >= 48:  # Bandit corner brackets, in orange
        n, inset, w = S * 0.12, S * 0.07, max(2, S // 64)
        for x, y, dx, dy in ((inset, inset, 1, 1), (S - inset, inset, -1, 1), (inset, S - inset, 1, -1),
                             (S - inset, S - inset, -1, -1)):
            d.line([(x, y + dy * n), (x, y), (x + dx * n, y)], fill=ORANGE, width=w)
    return im.resize((size, size), Image.LANCZOS)


def silhouette(S, margin):
    """The fox as one shape (ears, cheeks, face), with the mask cut out, as an L mask."""
    sil = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(sil)
    scale = (S - 2 * margin) / FH
    ox, oy = (S - FW * scale) / 2, margin
    tr = lambda p: [(ox + x * scale, oy + y * scale) for x, y in p]
    for p in (EAR, mirror(EAR), CHEEK, mirror(CHEEK), FACE):
        d.polygon(tr(p), fill=255)
    holes = Image.new("L", (S, S), 0)
    h = ImageDraw.Draw(holes)
    for p in (MASK, mirror(MASK), NOSE):
        h.polygon(tr(p), fill=255)
    return sil, holes


# The mark in its 30-unit box, as shell.rs `fox_mark` draws it.
MARK = [(15.0, 3.0), (27.0, 26.0), (15.0, 20.0), (3.0, 26.0)]
RING = (15.0, 15.0, 13.0)

# Per taskbar: the arrowhead (the theme's oranges, #384), the ring, the paused arrowhead, and what
# the pause badge sits on (cut out of the mark so the badge reads on any wallpaper behind it).
TRAY_COLOURS = {
    "white": {"accent": (0xF0, 0x8A, 0x3C), "ring": (255, 255, 255, 165), "muted": (0x9A, 0xA0, 0xA8)},
    "black": {"accent": (0xB4, 0x54, 0x0F), "ring": (0, 0, 0, 140), "muted": (0x5F, 0x64, 0x6C)},
    "template": {"accent": (0, 0, 0), "ring": (0, 0, 0, 150), "muted": (0, 0, 0)},
}
TRAY_SIZES = [16, 20, 24, 32]
TRAY_STATES = ["on", "off", "paused"]


def tray(size, variant, state):
    """One glyph: `state` is "on" (solid), "off" (outline) or "paused"."""
    c = TRAY_COLOURS[variant]
    ss = 16
    S = size * ss
    u = S / 30.0  # the mark's unit
    px = ss  # one output pixel, in supersampled pixels
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    # The ring: one pixel up to 24 px, a pixel and a half at 32.
    rw = round(px * (1.0 if size <= 24 else 1.5))
    cx, cy, r = (v * u for v in RING)
    r = min(r, S / 2 - rw / 2)
    d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=c["ring"], width=rw)
    arrow = [(x * u, y * u) for x, y in MARK]
    if state == "on":
        d.polygon(arrow, fill=c["accent"] + (255,))
    elif state == "off":
        mask = Image.new("L", (S, S), 0)
        ImageDraw.Draw(mask).polygon(arrow, fill=255)
        k = round(px * (1.4 if size <= 20 else 1.6)) * 2 + 1
        ring = ImageChops.subtract(mask, mask.filter(ImageFilter.MinFilter(k)))
        layer = Image.new("RGBA", (S, S), c["accent"] + (255,))
        layer.putalpha(ring)
        im.alpha_composite(layer)
    else:
        d.polygon(arrow, fill=c["muted"] + (255,))
        # The badge: a disc in the bottom-right corner, cut clear of the mark, with two bars.
        br = S * 0.235
        bx, by = S - br, S - br
        gap = px * (1.0 if size <= 24 else 1.5)
        cut = Image.new("L", (S, S), 255)
        ImageDraw.Draw(cut).ellipse([bx - br - gap, by - br - gap, bx + br + gap, by + br + gap], fill=0)
        im.putalpha(ImageChops.multiply(im.getchannel("A"), cut))
        d = ImageDraw.Draw(im)
        d.ellipse([bx - br, by - br, bx + br, by + br], fill=c["accent"] + (255,))
        # Bars snapped to whole output pixels so they stay sharp.
        bw = max(1, round(size / 16))
        bh = max(3, round(size * 0.24))
        gap_bars = max(1, round(size / 16))
        left = round(bx / px - (2 * bw + gap_bars) / 2)
        top = round(by / px - bh / 2)
        for x0 in (left, left + bw + gap_bars):
            d.rectangle([x0 * px, top * px, (x0 + bw) * px - 1, (top + bh) * px - 1], fill=(0, 0, 0, 0))
    return im.reduce(ss)


def tray_name(variant, state, size):
    suffix = {"on": "", "off": "-off", "paused": "-paused"}[state]
    sized = "" if size in (32, 44) else "-%d" % size
    return "tray-icon-%s%s%s.png" % (variant, suffix, sized)


def write_tray():
    for variant in ("white", "black"):
        for state in TRAY_STATES:
            for size in TRAY_SIZES:
                tray(size, variant, state).save(os.path.join(RES, tray_name(variant, state, size)))
    for state in TRAY_STATES:
        tray(44, "template", state).save(os.path.join(RES, tray_name("template", state, 44)))


def svg():
    """The master, as vectors (the full fox in the dark tile)."""
    S = 256
    scale, ox, oy = tile_geometry(S, False)
    pts = lambda p: " ".join("%.1f,%.1f" % (ox + x * scale, oy + y * scale) for x, y in p)
    body = ["<polygon points=\"%s\" fill=\"%s\"/>" % (pts(p), c) for p, c in fox_layers(False)]
    w = S / 160
    lines = [(340, 300), (340, 700)]
    body.append("<polyline points=\"%s\" fill=\"none\" stroke=\"%s\" stroke-width=\"%.2f\"/>" % (pts(lines), DARK, w))
    for p in (FACET, mirror(FACET)):
        body.append("<polyline points=\"%s\" fill=\"none\" stroke=\"%s\" stroke-width=\"%.2f\"/>" % (pts(p), DARK, w))
    n, inset = S * 0.12, S * 0.07
    br = " ".join("M%.1f %.1f L%.1f %.1f L%.1f %.1f" % (x, y + dy * n, x, y, x + dx * n, y)
                  for x, y, dx, dy in ((inset, inset, 1, 1), (S - inset, inset, -1, 1),
                                       (inset, S - inset, 1, -1), (S - inset, S - inset, -1, -1)))
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">\n'
            '<rect x="0.5" y="0.5" width="255" height="255" rx="56" fill="%s" stroke="%s"/>\n%s\n'
            '<path d="%s" fill="none" stroke="%s" stroke-width="4"/>\n</svg>\n'
            % (DARK, EDGE, "\n".join(body), br, ORANGE))


ICO_SIZES = [16, 20, 24, 32, 48, 64, 256]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sheet")
    ap.add_argument("--tray", action="store_true")
    a = ap.parse_args()
    if a.tray:
        write_tray()
        print("wrote the tray glyphs")
        if a.sheet:
            sheet(a.sheet)
        return
    icons = {s: draw_icon(s) for s in ICO_SIZES}
    icons[256].save(os.path.join(RES, "icon.png"))
    icons[256].save(os.path.join(RES, "icon.ico"), format="ICO", sizes=[(s, s) for s in ICO_SIZES],
                    append_images=[icons[s] for s in ICO_SIZES[:-1]])
    open(os.path.join(RES, "icon.svg"), "w", encoding="utf-8", newline="\n").write(svg())
    write_tray()
    assets = os.path.join(ROOT, "app", "packaging", "identity", "Assets")
    for s in (44, 50, 150):
        draw_icon(s).save(os.path.join(assets, "logo%d.png" % s))
    print("wrote app icon, tray glyphs and identity logos")
    if a.sheet:
        sheet(a.sheet)


def sheet(path):
    """Every tray glyph at its real size on a dark and a light taskbar, then at 6x to judge the
    pixels, and the macOS templates."""
    from PIL import ImageFont

    try:
        font = ImageFont.truetype("segoeui.ttf", 15)
    except OSError:
        font = ImageFont.load_default()
    zoom = 6
    col_w = 16 + sum(s * zoom + 12 for s in TRAY_SIZES)
    W = 140 + col_w
    rows = [(v, st) for v in ("white", "black") for st in TRAY_STATES]
    row_h = 32 * zoom + 24
    H = 40 + len(rows) * row_h + 150
    im = Image.new("RGBA", (W, H), "#808080")
    d = ImageDraw.Draw(im)
    d.text((12, 10), "Foxglove tray glyphs (#403): 16 / 20 / 24 / 32 px, actual size then x%d" % zoom,
           fill="#ffffff", font=font)
    bgs = {"white": "#202020", "black": "#f3f3f3"}
    for i, (variant, state) in enumerate(rows):
        y0 = 40 + i * row_h
        bg = bgs[variant]
        fg = "#ffffff" if variant == "white" else "#000000"
        d.rectangle([0, y0, W, y0 + row_h - 4], fill=bg)
        label = {"on": "connected", "off": "no keyboard", "paused": "paused"}[state]
        d.text((12, y0 + 8), "%s taskbar" % ("dark" if variant == "white" else "light"), fill=fg, font=font)
        d.text((12, y0 + 28), label, fill=fg, font=font)
        x = 12
        for s in TRAY_SIZES:
            g = Image.open(os.path.join(RES, tray_name(variant, state, s)))
            im.alpha_composite(g, (x, y0 + 60 + (32 - s) // 2))
            x += s + 6
        x = 140
        for s in TRAY_SIZES:
            g = Image.open(os.path.join(RES, tray_name(variant, state, s)))
            big = g.resize((s * zoom, s * zoom), Image.NEAREST)
            im.alpha_composite(big, (x, y0 + 10 + (32 - s) * zoom // 2))
            x += s * zoom + 12
    y0 = 40 + len(rows) * row_h
    d.rectangle([0, y0, W, H], fill="#e8e8e8")
    d.text((12, y0 + 8), "macOS templates (44 px, tinted by the menu bar)", fill="#000000", font=font)
    x = 12
    for state in TRAY_STATES:
        g = Image.open(os.path.join(RES, tray_name("template", state, 44)))
        im.alpha_composite(g, (x, y0 + 40))
        im.alpha_composite(g.resize((88, 88), Image.NEAREST), (x + 56, y0 + 40))
        x += 180
    im.save(path)
    print("sheet", path)


if __name__ == "__main__":
    main()
