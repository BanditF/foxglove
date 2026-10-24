"""Draws Foxglove's app icon and tray glyphs from the Bandit fox motif (#31).

  python -I scripts/app_icons.py               rewrite the files in app/resources/ (same names as
                                               KeyPeek's, so upstream syncs don't conflict)
  python -I scripts/app_icons.py --sheet x.png also a review sheet

Writes:
- icon.svg (the master), icon.png (256), icon.ico (16, 20, 24, 32, 48, 64, 256): a dark tile with
  the low-poly fox. 24 px and below use a simplified fox (silhouette, mask, no inner lines).
- tray-icon-white.png / tray-icon-black.png (64): the fox as a solid glyph with the mask cut
  out, for dark / light taskbars, plus *-off.png: the outline only, for "no keyboard connected".
- tray-icon-template.png (44) and tray-icon-template-off.png: macOS templates (black + alpha).
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


def tray(size, colour, connected):
    ss = 8
    S = size * ss
    sil, holes = silhouette(S, margin=S * 0.06)
    if connected:
        alpha = ImageChops.subtract(sil, holes)
    else:  # outline only: the silhouette minus itself eroded by about 1/9 of the icon
        k = max(3, (S // 9) | 1)
        alpha = ImageChops.subtract(sil, sil.filter(ImageFilter.MinFilter(k)))
    a = alpha.resize((size, size), Image.LANCZOS)
    out = Image.new("RGBA", (size, size), colour)
    out.putalpha(a)
    return out


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
    a = ap.parse_args()
    icons = {s: draw_icon(s) for s in ICO_SIZES}
    icons[256].save(os.path.join(RES, "icon.png"))
    icons[256].save(os.path.join(RES, "icon.ico"), format="ICO", sizes=[(s, s) for s in ICO_SIZES],
                    append_images=[icons[s] for s in ICO_SIZES[:-1]])
    open(os.path.join(RES, "icon.svg"), "w", encoding="utf-8", newline="\n").write(svg())
    for name, colour in (("white", (255, 255, 255, 255)), ("black", (0, 0, 0, 255))):
        tray(64, colour, True).save(os.path.join(RES, "tray-icon-%s.png" % name))
        tray(64, colour, False).save(os.path.join(RES, "tray-icon-%s-off.png" % name))
    tray(44, (0, 0, 0, 255), True).save(os.path.join(RES, "tray-icon-template.png"))
    tray(44, (0, 0, 0, 255), False).save(os.path.join(RES, "tray-icon-template-off.png"))
    assets = os.path.join(ROOT, "app", "packaging", "identity", "Assets")
    for s in (44, 50, 150):
        draw_icon(s).save(os.path.join(assets, "logo%d.png" % s))
    print("wrote app icon, tray glyphs and identity logos")
    if a.sheet:
        sheet(a.sheet, icons)


def sheet(path, icons):
    W = 900
    im = Image.new("RGBA", (W, 640), "#808080")
    d = ImageDraw.Draw(im)
    for row, bg in enumerate(("#202020", "#f3f3f3")):  # dark and light taskbar / desktop
        y0 = row * 320
        d.rectangle([0, y0, W, y0 + 318], fill=bg)
        im.alpha_composite(icons[256], (W - 266, y0 + 30))
        x = 10
        for s in ICO_SIZES[:-1]:
            im.alpha_composite(icons[s], (x, y0 + 10))
            x += s + 12
        x = 10
        y = y0 + 100
        col = (255, 255, 255, 255) if row == 0 else (0, 0, 0, 255)
        for connected in (True, False):
            for s in (16, 20, 24, 32, 48):
                g = tray(64, col, connected).resize((s, s), Image.LANCZOS)
                im.alpha_composite(g, (x, y + (48 - s) // 2))
                x += s + 12
            x += 30
        # tray glyphs at 4x, to judge the pixels
        x = 10
        for connected in (True, False):
            for s in (16, 24):
                g = tray(64, col, connected).resize((s, s), Image.LANCZOS).resize((s * 4, s * 4), Image.NEAREST)
                im.alpha_composite(g, (x, y0 + 170))
                x += s * 4 + 16
    im.save(path)
    print("sheet", path)


if __name__ == "__main__":
    main()
