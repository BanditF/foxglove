"""Drawn mockups for docs/design/device-focus-layouts-thumbnails-lights-declutter.md (ux-1105).

Python + Pillow, no app code: the shapes and colours follow app/src/foxglove/theme.rs (DARK
palette) and the app's fonts. Run:

    python docs/design/mockups/ux_1105_mockups.py <out-dir>

It writes ux-1105-proposed-*.png into <out-dir>. Mockups only; the real UI is egui.
"""

import math
import os
import sys

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
FONTS = os.path.normpath(os.path.join(HERE, "..", "..", "..", "app", "resources", "fonts"))
S = 1.5  # pixels per point, as the egui previews

# theme.rs DARK
GROUND = (0x0E, 0x12, 0x11)
SURFACE = (0x16, 0x1C, 0x1A)
RAISED = (0x1D, 0x24, 0x22)
WELL = (0x0A, 0x0D, 0x0C)
LINE = (0x2A, 0x33, 0x30)
CONTROL = (0x23, 0x2B, 0x28)
CONTROL_LINE = (0x34, 0x40, 0x3B)
TEXT = (0xD9, 0xE0, 0xDC)
STRONG = (0xF2, 0xF5, 0xF3)
MUTED = (0x8E, 0x9A, 0x94)
ACCENT = (0xF0, 0x8A, 0x3C)
SEL_FILL = (0x3A, 0x24, 0x12)
SEL_TEXT = (0xFF, 0xCF, 0xA6)
OK = (0x5F, 0xC4, 0x8E)
WARN = (0xF0, 0xB5, 0x3C)
NOTE = (0x6F, 0xA8, 0xFF)  # annotation colour (not a theme colour)

SWATCHES = [(0xF0, 0x8A, 0x3C), (0xFF, 0x5F, 0x3C), (0xF0, 0xD2, 0x3C),
            (0x5F, 0xC4, 0x8E), (0x7F, 0xB0, 0xE0), (0xE6, 0xE6, 0xE6)]


def font(kind, size):
    name = {"body": "IBMPlexSans-Regular.ttf", "mono": "IBMPlexMono-Medium.ttf",
            "hud": "ChakraPetch-SemiBold.ttf", "sym": None}[kind]
    if name is None:
        return _sym(int(size * S))
    return _font(name, int(size * S))


_CACHE = {}


def _font(name, px):
    key = (name, px)
    if key not in _CACHE:
        _CACHE[key] = ImageFont.truetype(os.path.join(FONTS, name), px)
    return _CACHE[key]


def _sym(px):
    key = ("sym", px)
    if key not in _CACHE:
        _CACHE[key] = ImageFont.truetype(r"C:\Windows\Fonts\seguisym.ttf", px)
    return _CACHE[key]


SYMBOLS = set("↖▾▸⋯✓⚠◑ⓘ→")


class Canvas:
    def __init__(self, w, h, bg=GROUND):
        self.img = Image.new("RGB", (int(w * S), int(h * S)), bg)
        self.d = ImageDraw.Draw(self.img)

    def p(self, *xy):
        return [v * S for v in xy]

    def rect(self, x, y, w, h, fill=None, outline=None, r=6, width=1):
        self.d.rounded_rectangle(self.p(x, y, x + w, y + h), radius=r * S, fill=fill,
                                 outline=outline, width=max(1, int(width * S)))

    def runs(self, s, size, kind):
        """The text split into runs the app font has, and symbols from Segoe UI Symbol."""
        f, sym = font(kind, size), font("sym", size)
        out = []
        for ch in s:
            use = sym if ch in SYMBOLS else f
            if out and out[-1][1] is use:
                out[-1][0] += ch
            else:
                out.append([ch, use])
        return out

    def text(self, x, y, s, size=13, color=TEXT, kind="body", anchor="la"):
        if anchor != "la" and not any(ch in SYMBOLS for ch in s):
            self.d.text(self.p(x, y), s, font=font(kind, size), fill=color, anchor=anchor)
            return self.tw(s, size, kind)
        w = self.tw(s, size, kind)
        if anchor[0] == "r":
            x -= w
        elif anchor[0] == "m":
            x -= w / 2
        base = font(kind, size)
        for run, f in self.runs(s, size, kind):
            dy = 0 if f is base else (base.getmetrics()[0] - f.getmetrics()[0]) / S
            self.d.text(self.p(x, y + dy), run, font=f, fill=color)
            x += self.d.textlength(run, font=f) / S
        return w

    def tw(self, s, size=13, kind="body"):
        return sum(self.d.textlength(r, font=f) for r, f in self.runs(s, size, kind)) / S

    def wrap(self, x, y, s, width, size=13, color=TEXT, kind="body", lh=1.35):
        words, line, yy = s.split(), "", y
        for w in words:
            t = (line + " " + w).strip()
            if self.tw(t, size, kind) > width and line:
                self.text(x, yy, line, size, color, kind)
                yy += size * lh
                line = w
            else:
                line = t
        if line:
            self.text(x, yy, line, size, color, kind)
            yy += size * lh
        return yy

    def circle(self, cx, cy, r, fill=None, outline=None, width=1):
        self.d.ellipse(self.p(cx - r, cy - r, cx + r, cy + r), fill=fill, outline=outline,
                       width=max(1, int(width * S)))

    def line(self, x0, y0, x1, y1, color=LINE, width=1):
        self.d.line(self.p(x0, y0, x1, y1), fill=color, width=max(1, int(width * S)))

    def save(self, path):
        self.img.save(path)
        print(path)


# --- widgets --------------------------------------------------------------------------------

def card(c, x, y, w, h, title):
    c.rect(x, y, w, h, fill=SURFACE, outline=LINE, r=8)
    c.text(x + 16, y + 12, title, 14.5, STRONG, "hud")


def pill(c, x, y, label, selected=False, size=12.5):
    w = c.tw(label, size) + 22
    if selected:
        c.rect(x, y, w, 24, fill=SEL_FILL, outline=ACCENT, r=12)
        c.text(x + 11, y + 4, label, size, SEL_TEXT)
    else:
        c.rect(x, y, w, 24, fill=CONTROL, outline=CONTROL_LINE, r=12)
        c.text(x + 11, y + 4, label, size, TEXT)
    return w


def button(c, x, y, label, primary=False, size=12.5):
    w = c.tw(label, size) + 20
    if primary:
        c.rect(x, y, w, 24, fill=ACCENT, r=5)
        c.text(x + 10, y + 4, label, size, (0x14, 0x0D, 0x07))
    else:
        c.rect(x, y, w, 24, fill=CONTROL, outline=CONTROL_LINE, r=5)
        c.text(x + 10, y + 4, label, size, TEXT)
    return w


def chip(c, x, y, label, color=TEXT, fill=WELL, outline=LINE, size=11.5):
    w = c.tw(label, size) + 16
    c.rect(x, y, w, 19, fill=fill, outline=outline, r=9)
    c.text(x + 8, y + 2, label, size, color)
    return w


def info(c, x, y, color=MUTED):
    """The ⓘ: a small ring with an i, for the full text on hover."""
    c.circle(x + 7, y + 8, 6.5, outline=color, width=1.2)
    c.text(x + 7, y + 8.5, "i", 10, color, "mono", anchor="mm")
    return 16


def checkbox(c, x, y, label, on=True, size=12.5, color=TEXT):
    c.rect(x, y + 2, 15, 15, fill=ACCENT if on else CONTROL, outline=ACCENT if on else CONTROL_LINE, r=3)
    if on:
        c.line(x + 3.5, y + 9.5, x + 6.5, y + 12.5, (0x14, 0x0D, 0x07), 2)
        c.line(x + 6.5, y + 12.5, x + 12, y + 5.5, (0x14, 0x0D, 0x07), 2)
    return 22 + c.text(x + 22, y + 1, label, size, color)


def slider(c, x, y, w, frac):
    c.rect(x, y + 8, w, 5, fill=CONTROL, r=2)
    c.rect(x, y + 8, w * frac, 5, fill=ACCENT, r=2)
    c.circle(x + w * frac, y + 10.5, 7, fill=STRONG)


def swatch(c, x, y, rgb, selected=False, r=12):
    if selected:
        c.circle(x + r, y + r, r + 3.5, outline=STRONG, width=2)
    c.circle(x + r, y + r, r + 2, fill=GROUND)
    c.circle(x + r, y + r, r, fill=rgb)
    return 2 * r + 10


def caps(c, x, y, s, color=MUTED):
    return c.text(x, y, s.upper(), 11, color, "hud")


def tooltip(c, x, y, w, s, size=12):
    lines_h = 0
    # measure
    tmp = Canvas(1, 1)
    words, line, n = s.split(), "", 1
    for wd in words:
        t = (line + " " + wd).strip()
        if tmp.tw(t, size) > w - 20 and line:
            n += 1
            line = wd
        else:
            line = t
    lines_h = n * size * 1.35
    c.rect(x + 3, y + 3, w, lines_h + 16, fill=(0, 0, 0), r=6)
    c.rect(x, y, w, lines_h + 16, fill=RAISED, outline=CONTROL_LINE, r=6)
    c.wrap(x + 10, y + 7, s, w - 20, size, TEXT)
    return lines_h + 16


def note(c, x, y, s, w=260, size=11.5):
    """A blue design note, not part of the UI."""
    return c.wrap(x, y, s, w, size, NOTE)


def arrow(c, x0, y0, x1, y1):
    c.line(x0, y0, x1, y1, NOTE, 1.2)
    a = math.atan2(y1 - y0, x1 - x0)
    for da in (2.6, -2.6):
        c.line(x1, y1, x1 + 7 * math.cos(a + da), y1 + 7 * math.sin(a + da), NOTE, 1.2)


SIDE = 213  # the sidebar's width, as the app's shell


def shell(c, W, H, page, menu_open=False):
    """The window as the app draws it: title bar, sidebar, the status strip led by the keyboard
    switcher (the focus), the page title. Returns the content's left edge, top and width."""
    c.rect(0, 0, W, H, fill=(0x0B, 0x0E, 0x0D), r=0)
    c.rect(16, 16, W - 32, H - 32, fill=GROUND, outline=LINE, r=10)
    c.rect(16, 16, W - 32, 38, fill=RAISED, r=10)
    c.text(W / 2, 26, "Foxglove", 15, TEXT, "hud", anchor="ma")
    c.text(W - 40, 26, "\u00d7", 15, MUTED, anchor="ma")
    c.line(16 + SIDE, 54, 16 + SIDE, H - 16, LINE)
    c.text(52, 70, "FOXGLOVE", 17, STRONG, "hud")
    c.text(52, 94, "STEEL FOR GOOD", 9.5, MUTED, "mono")
    c.d.polygon([v * S for v in (30, 72, 46, 78, 38, 94)], fill=ACCENT)
    y = 132
    for name in ["Keyboard", "Lights", "Scenes", "Screens", "Plugins", "App"]:
        if name == page:
            c.rect(28, y - 8, SIDE - 24, 34, fill=RAISED, outline=LINE, r=6)
        c.circle(46, y + 9, 3.5, fill=ACCENT if name == page else CONTROL_LINE)
        c.text(60, y, name, 14, STRONG if name == page else TEXT)
        y += 41
    # the strip: the switcher first, then the focused keyboard's parts, then Profile
    sx = 16 + SIDE + 18
    c.rect(sx, 66, 222, 44, fill=RAISED, outline=ACCENT if menu_open else CONTROL_LINE, r=7)
    thumb_frame(c, sx + 6, 70, lambda a, b, w, h: mini_keyboard(c, a, b, w, h, split=True, tint=ACCENT), w=46, h=36)
    c.text(sx + 60, 70, "Eyelash Corne", 13, STRONG)
    c.text(sx + 60, 88, "2 keyboards \u00b7 switch", 11, MUTED)
    c.text(sx + 204, 77, "\u25be", 13, TEXT)
    hx = sx + 234
    c.rect(hx, 72, 262, 30, fill=SURFACE, outline=LINE, r=6)
    c.circle(hx + 12, 87, 4, fill=OK)
    c.text(hx + 22, 78, "Left  on USB \u00b7 0.3.0 \u00b7 battery 95%", 12.5, TEXT)
    hx += 270
    c.rect(hx, 72, 200, 30, fill=SURFACE, outline=LINE, r=6)
    c.circle(hx + 12, 87, 4, fill=OK)
    c.text(hx + 22, 78, "Right  linked \u00b7 battery 61%", 12.5, TEXT)
    c.rect(W - 32 - 140, 70, 140, 26, fill=CONTROL, outline=CONTROL_LINE, r=5)
    c.text(W - 32 - 130, 74, "Profile: Default  \u25be", 12.5, TEXT)
    c.line(16 + SIDE, 124, W - 16, 124, LINE)
    c.text(16 + SIDE + 22, 136, page, 22, STRONG, "hud")
    return 16 + SIDE + 22, 178, W - 32 - SIDE - 44


# --- low-fi device drawings (thumbnails): shapes from kind + zones, never per-product art -------

def mini_keyboard(c, x, y, w, h, split=False, tint=ACCENT, zones=None, rows=3):
    """A keyboard outline with key rows; `zones` is a list of colours tinted left to right."""
    def half(hx, hw, cols_off=0):
        c.rect(hx, y, hw, h, fill=CONTROL, outline=CONTROL_LINE, r=2.5)
        cols = 6 if split else max(4, min(14, int(hw / 3)))
        for r in range(rows):
            for k in range(cols):
                kx = hx + 2 + k * (hw - 4) / cols
                ky = y + 2 + r * (h - 4) / (rows + (1 if split else 0))
                zi = (k + cols_off) * len(zones) // (cols * (2 if split else 1)) if zones else 0
                col = zones[zi] if zones else tint
                col = tuple(int(v * 0.85) for v in col)
                kw = max(0.6, (hw - 4) / cols - 1.0)
                kh = max(0.6, (h - 4) / (rows + (1 if split else 0)) - 1.0)
                c.d.rectangle(c.p(kx + 0.4, ky + 0.4, kx + 0.4 + kw, ky + 0.4 + kh), fill=col)
    if split:
        hw = w * 0.46
        half(x, hw)
        half(x + w - hw, hw, 6)
    else:
        half(x, w)


def mini_case(c, x, y, w, h, fans, strip=None):
    """A case front: its fans as rings in a row, a strip down the side."""
    c.rect(x, y, w, h, fill=CONTROL, outline=CONTROL_LINE, r=2.5)
    n = len(fans)
    room = w - (8 if strip else 2)
    r = min(h * 0.34, room / (2 * n) - 1)
    for i, col in enumerate(fans):
        cx = x + 2 + room * (i + 0.5) / n
        c.circle(cx, y + h / 2, r, outline=col, width=2.2)
        c.circle(cx, y + h / 2, r * 0.3, fill=CONTROL_LINE)
    if strip:
        c.rect(x + w - 6, y + 3, 3, h - 6, fill=strip, r=1)


def mini_cooler(c, x, y, w, h, col):
    cx, cy, r = x + w / 2, y + h / 2, min(w, h) / 2 - 1
    c.circle(cx, cy, r, fill=CONTROL, outline=CONTROL_LINE)
    c.circle(cx, cy, r - 3, outline=col, width=2.4)


def mini_strip(c, x, y, w, h, cols):
    sh = 7
    sy = y + (h - sh) / 2
    c.rect(x, sy, w, sh, fill=CONTROL, outline=CONTROL_LINE, r=3)
    n = len(cols)
    for i, col in enumerate(cols):
        c.circle(x + 4 + i * (w - 8) / max(1, n - 1), sy + sh / 2, 2, fill=col)


def mini_mouse(c, x, y, w, h, col):
    mw, mh = w * 0.5, h - 2
    mx = x + (w - mw) / 2
    c.rect(mx, y + 1, mw, mh, fill=CONTROL, outline=CONTROL_LINE, r=mw / 2)
    c.line(mx + mw / 2, y + 3, mx + mw / 2, y + mh * 0.42, CONTROL_LINE, 1)
    if col:
        c.rect(mx + mw / 2 - 2, y + mh * 0.62, 4, 6, fill=col, r=2)


def mini_headset(c, x, y, w, h, col=None):
    cx = x + w / 2
    c.d.arc(c.p(cx - w * 0.32, y + 2, cx + w * 0.32, y + h * 1.1), 180, 360, fill=CONTROL_LINE,
            width=int(2.5 * S))
    for sx in (cx - w * 0.32, cx + w * 0.32):
        c.rect(sx - 4, y + h * 0.5, 8, h * 0.45, fill=CONTROL, outline=CONTROL_LINE, r=3)
        if col:
            c.rect(sx - 1.5, y + h * 0.6, 3, h * 0.25, fill=col, r=1)


def mini_controller(c, x, y, w, h):
    c.rect(x + 3, y + h * 0.25, w - 6, h * 0.55, fill=CONTROL, outline=CONTROL_LINE, r=h * 0.27)
    c.circle(x + w * 0.32, y + h * 0.5, 2.5, fill=CONTROL_LINE)
    c.circle(x + w * 0.68, y + h * 0.5, 2.5, fill=CONTROL_LINE)


def mini_lamps(c, x, y, w, h, cols):
    """A LampArray Foxglove knows only as lamps: a dot grid in their colours."""
    c.rect(x, y, w, h, fill=CONTROL, outline=CONTROL_LINE, r=2.5)
    n = len(cols)
    per = math.ceil(math.sqrt(n * w / h))
    for i, col in enumerate(cols):
        c.circle(x + 4 + (i % per) * (w - 8) / max(1, per - 1),
                 y + 4 + (i // per) * (h - 8) / max(1, math.ceil(n / per) - 1), 1.6, fill=col)


def mini_unknown(c, x, y, w, h):
    c.d.rounded_rectangle(c.p(x, y, x + w, y + h), radius=3 * S, outline=CONTROL_LINE,
                          width=int(1 * S))
    c.text(x + w / 2, y + h / 2, "?", 12, MUTED, "mono", anchor="mm")


def thumb_frame(c, x, y, draw, w=56, h=38):
    c.rect(x, y, w, h, fill=WELL, outline=LINE, r=5)
    draw(x + 6, y + 5, w - 12, h - 10)


# --- key layouts -------------------------------------------------------------------------------

def corne_half(right=False):
    stagger = [0.375, 0.375, 0.125, 0.0, 0.125, 0.25]
    keys = []
    for col in range(6):
        for row in range(3):
            keys.append((col, row + stagger[col], 1, 1))
    for t, (tx, ty) in enumerate([(3.5, 3.55), (4.6, 3.7), (5.7, 3.9)]):
        keys.append((tx, ty, 1, 1))
    if right:
        keys = [(7 - x - w, y, w, h) for (x, y, w, h) in keys]
    return keys


def full_size():
    """A full-size ANSI board in key units (what a VIA definition's KLE rows give)."""
    keys = []

    def row(y, items, x=0.0):
        for it in items:
            if isinstance(it, tuple) and it[0] == "gap":
                x += it[1]
                continue
            w = it
            keys.append((x, y, w, 1))
            x += w
    row(0, [1, ("gap", 1), 1, 1, 1, 1, ("gap", 0.5), 1, 1, 1, 1, ("gap", 0.5), 1, 1, 1, 1,
            ("gap", 0.25), 1, 1, 1, ("gap", 0.25), 1, 1, 1, 1])
    row(1.25, [1] * 13 + [2, ("gap", 0.25), 1, 1, 1, ("gap", 0.25), 1, 1, 1, 1])
    row(2.25, [1.5] + [1] * 12 + [1.5, ("gap", 0.25), 1, 1, 1, ("gap", 0.25), 1, 1, 1])
    row(3.25, [1.75] + [1] * 11 + [2.25, ("gap", 3.5), 1, 1, 1])
    row(4.25, [2.25] + [1] * 10 + [2.75, ("gap", 1.25), 1, ("gap", 1.25), 1, 1, 1])
    row(5.25, [1.25, 1.25, 1.25, 6.25, 1.25, 1.25, 1.25, 1.25, ("gap", 0.25), 1, 1, 1,
               ("gap", 0.25), 2, 1])
    # tall numpad keys
    keys.append((21.5, 2.25, 1, 2))
    keys.append((21.5, 4.25, 1, 2))
    return keys


def draw_keys(c, x, y, unit, keys, tint=None, zone_of=None, label=None, gap=0.08):
    for i, (kx, ky, kw, kh) in enumerate(keys):
        col = zone_of(i, kx, ky) if zone_of else (tint or CONTROL)
        c.rect(x + (kx + gap) * unit, y + (ky + gap) * unit, (kw - 2 * gap) * unit,
               (kh - 2 * gap) * unit, fill=col, outline=CONTROL_LINE, r=unit * 0.12)


def shade(col, f):
    return tuple(int(v * f) for v in col)


# --- 1: device focus --------------------------------------------------------------------------

def focus(out):
    W, H = 1240, 760
    c = Canvas(W, H)
    x0, y0, cw = shell(c, W, H, "Keyboard", menu_open=True)
    # the Keyboard page under it, named for the focus
    card(c, x0, y0, cw, 250, "Eyelash Corne")
    chip(c, x0 + 140, y0 + 12, "Layout from the keyboard", OK)
    unit = 28
    draw_keys(c, x0 + 30, y0 + 52, unit, corne_half())
    draw_keys(c, x0 + 30 + 7.8 * unit, y0 + 52, unit, corne_half(right=True))
    note(c, x0 + 30, y0 + 200, "Card titles name the focused keyboard, so every page says which"
         " one it means. Pages carry no picker of their own.", 440)
    # the switcher's list, drawn beside the card so the page stays readable (in the app it
    # drops straight down from the switcher)
    mx, my = x0 + cw - 340, y0 - 30
    arrow(c, 16 + SIDE + 18 + 222, 96, mx - 4, my + 30)
    c.text(mx, my - 20, "the switcher, opened", 11.5, NOTE)
    c.rect(mx + 3, my + 3, 320, 262, fill=(0, 0, 0), r=8)
    c.rect(mx, my, 320, 262, fill=RAISED, outline=CONTROL_LINE, r=8)
    caps(c, mx + 14, my + 12, "Keyboard shown in Foxglove")
    rows = [("Eyelash Corne", "ZMK \u00b7 USB \u00b7 Foxglove firmware", True, True),
            ("Keychron K5 Pro", "VIA \u00b7 USB \u00b7 read-only", False, False),
            ("Halcyon Corne", "ZMK \u00b7 Bluetooth \u00b7 not connected", False, None)]
    y = my + 34
    for name, sub, sel, split in rows:
        if sel:
            c.rect(mx + 6, y - 4, 308, 46, fill=SEL_FILL, r=6)

        def draw(a, b, w, h, s=split):
            if s is None:
                mini_keyboard(c, a, b, w, h, split=True, tint=CONTROL_LINE)
            else:
                mini_keyboard(c, a, b, w, h, split=s, tint=ACCENT if s else (0x7F, 0xB0, 0xE0),
                              rows=3 if s else 4)
        thumb_frame(c, mx + 14, y, draw, w=48, h=34)
        c.text(mx + 72, y, name, 13, SEL_TEXT if sel else (MUTED if split is None else STRONG))
        c.text(mx + 72, y + 18, sub, 11, MUTED)
        if sel:
            c.text(mx + 292, y + 8, "\u2713", 13, ACCENT)
        y += 50
    c.line(mx + 10, y, mx + 310, y, LINE)
    w = checkbox(c, mx + 14, y + 10, "Follow the keyboard I type on", on=False)
    info(c, mx + 14 + w + 6, y + 11)
    c.text(mx + 14, y + 36, "A keyboard that goes away stays listed, greyed", 11, MUTED)

    # which views follow the focus
    ty = y0 + 270
    rows = [
        ("Keyboard page drawing", "Follows the focus", None),
        ("Lights \u203a keyboard card, zones, Find my LEDs, Calibrate", "Follows the focus", None),
        ("Scenes room", "Draws every keyboard; the focused one is ringed", None),
        ("Layer overlay (KeyPeek)", "Follows the focus, or pinned", "Keyboard: Follow focus \u25be"),
        ("Wallpaper feed", "Follows the focus, or pinned", "Keyboard: Follow focus \u25be"),
        ("Tray panel", "Follows the focus; its title names it", None),
    ]
    c.rect(x0, ty, cw, 34 + 38 * len(rows), fill=SURFACE, outline=LINE, r=8)
    caps(c, x0 + 16, ty + 12, "View")
    caps(c, x0 + 400, ty + 12, "Which keyboard")
    caps(c, x0 + 700, ty + 12, "Override, in that view's card")
    yy = ty + 34
    for name, rule, override in rows:
        c.line(x0 + 1, yy, x0 + cw - 1, yy, LINE)
        c.text(x0 + 16, yy + 10, name, 12.5, TEXT)
        c.text(x0 + 400, yy + 10, rule, 12.5, MUTED)
        if override:
            c.rect(x0 + 700, yy + 6, c.tw(override, 12) + 20, 24, fill=CONTROL, outline=CONTROL_LINE, r=5)
            c.text(x0 + 710, yy + 10, override, 12, TEXT)
        else:
            c.text(x0 + 700, yy + 10, "\u2014", 12.5, MUTED)
        yy += 38
    note(c, 40, 420, "The switcher leads the status strip, which already describes the"
         " keyboard. One click opens the list, one click picks. With one keyboard the row stays,"
         " with no \u25be.", 175)
    c.save(os.path.join(out, "ux-1105-proposed-1-focus.png"))


# --- 2: key layout detection ------------------------------------------------------------------

def layouts(out):
    W, H = 1180, 640
    c = Canvas(W, H)
    x0 = 24
    c.text(x0, 20, "Each keyboard drawn from its own layout; the chip says where the shape came from",
           16, STRONG)

    # Corne from ZMK Studio's physical layout
    y = 56
    c.rect(x0, y, 540, 250, fill=SURFACE, outline=LINE, r=8)
    c.text(x0 + 16, y + 12, "EYELASH CORNE", 13, STRONG, "hud")
    chip(c, x0 + 150, y + 11, "Layout from the keyboard", OK)
    unit = 30
    draw_keys(c, x0 + 30, y + 54, unit, corne_half())
    draw_keys(c, x0 + 30 + 7.8 * unit, y + 54, unit, corne_half(right=True))
    c.text(x0 + 16, y + 222, "ZMK Studio physical layout “Eyelash 42” (1 of 2)", 11.5, MUTED)
    c.text(x0 + 360, y + 222, "Layout: Eyelash 42 ▾", 11.5, TEXT)

    # K5 Pro from its VIA definition
    c.rect(x0 + 556, y, 576, 250, fill=SURFACE, outline=LINE, r=8)
    c.text(x0 + 572, y + 12, "KEYCHRON K5 PRO", 13, STRONG, "hud")
    chip(c, x0 + 720, y + 11, "Layout from its VIA definition", OK)
    unit = 23.5
    draw_keys(c, x0 + 572, y + 50, unit, full_size(), gap=0.07)
    c.text(x0 + 572, y + 222, "ANSI · layout options read from the keyboard (#770)", 11.5, MUTED)

    # Unknown board: plain grid, guessed
    y2 = y + 270
    c.rect(x0, y2, 540, 230, fill=SURFACE, outline=LINE, r=8)
    c.text(x0 + 16, y2 + 12, "ZMK KEYBOARD", 13, STRONG, "hud")
    chip(c, x0 + 140, y2 + 11, "Shape guessed from 36 keys", WARN, outline=(0x5A, 0x48, 0x20))
    unit = 30
    keys = [((i % 12) * 1.0, (i // 12) * 1.0, 1, 1) for i in range(36)]
    draw_keys(c, x0 + 90, y2 + 54, unit, keys, tint=(0x1A, 0x20, 0x1E))
    c.text(x0 + 16, y2 + 160, "Key order is right; their places aren't known.", 12, MUTED)
    c.text(x0 + 16, y2 + 182, "Load a layout…", 12.5, ACCENT)
    c.text(x0 + 116, y2 + 182, "·  VIA / QMK info.json / KLE", 12, MUTED)

    # Fallback ladder
    c.rect(x0 + 556, y2, 576, 230, fill=SURFACE, outline=LINE, r=8)
    caps(c, x0 + 572, y2 + 14, "Where the shape comes from, best first")
    steps = [
        ("1", "The keyboard says", "ZMK Studio physical layouts; Vial's own definition", OK),
        ("2", "A definition you loaded", "VIA JSON, QMK info.json, KLE (Settings › Other keyboards)", OK),
        ("3", "Its board profile", "[drawing] in resources/boards (our boards, community ones)", TEXT),
        ("4", "Guessed", "a plain grid sized from the keymap's key count", WARN),
    ]
    yy = y2 + 40
    for n, title, sub, col in steps:
        c.circle(x0 + 584, yy + 10, 10, outline=col, width=1.5)
        c.text(x0 + 584, yy + 10, n, 11, col, "mono", anchor="mm")
        c.text(x0 + 604, yy, title, 13, STRONG)
        c.text(x0 + 604, yy + 19, sub, 11.5, MUTED)
        yy += 46

    note(c, x0, y2 + 248, "Every surface that draws keys (Keyboard page, zones, Find my LEDs, Scenes"
         " room, the overlay, the wallpaper feed) takes this one shape, so they can't disagree."
         " The overlay already reads ZMK's physical layout; the pages move onto the same source.", 1100)
    c.save(os.path.join(out, "ux-1105-proposed-2-layouts.png"))


# --- 3: device thumbnails ---------------------------------------------------------------------

def thumbs(out):
    W, H = 1180, 820
    c = Canvas(W, H)
    x0 = 24
    # Other devices card with thumbnails
    card(c, x0, 20, 700, 560, "Other devices")
    devices = [
        ("OMEN 45L case", "Built-in support", "5 lighting zones",
         lambda a, b, w, h: mini_case(c, a, b, w, h, [ACCENT, ACCENT, (0xFF, 0x5F, 0x3C)], strip=ACCENT)),
        ("OMEN cooler", "Built-in support", "1 lighting zone",
         lambda a, b, w, h: mini_cooler(c, a, b, w, h, (0x7F, 0xB0, 0xE0))),
        ("Keychron K5 Pro", "Over VIA · read-only", "Per-key lighting · Show in Foxglove",
         lambda a, b, w, h: mini_keyboard(c, a, b, w, h, zones=[(0x7F, 0xB0, 0xE0), (0x5F, 0xC4, 0x8E),
                                                                 (0xF0, 0xD2, 0x3C)], rows=4)),
        ("G502 mouse", "Windows lighting", "2 lamps",
         lambda a, b, w, h: mini_mouse(c, a, b, w, h, (0x5F, 0xC4, 0x8E))),
        ("Desk strip", "Through OpenRGB", "Per-LED lighting, 30 LEDs",
         lambda a, b, w, h: mini_strip(c, a, b, w, h, [SWATCHES[i % 5] for i in range(9)])),
        ("Light bar", "Windows lighting", "Per-LED lighting, 24 lamps",
         lambda a, b, w, h: mini_lamps(c, a, b, w, h, [ACCENT] * 24)),
        ("USB device 1A2C:2124", "Not controllable yet", "No lights Foxglove knows of",
         lambda a, b, w, h: mini_unknown(c, a, b, w, h)),
    ]
    y = 52
    for name, ctl, zones, draw in devices:
        c.line(x0 + 1, y, x0 + 699, y, LINE)
        thumb_frame(c, x0 + 16, y + 10, draw)
        c.text(x0 + 84, y + 10, name, 13, STRONG)
        c.text(x0 + 88 + c.tw(name, 13), y + 10, ctl, 12, MUTED)
        c.text(x0 + 84, y + 30, zones, 12, TEXT)
        if ctl != "Not controllable yet":
            bx = x0 + 600
            button(c, bx, y + 14, "Identify")
        y += 58
    c.line(x0 + 1, y, x0 + 699, y, LINE)
    note(c, x0 + 16, y + 14, "56×38 pt, in the row's left margin. Tints are the colours the device"
         " shows now (Foxglove's last write, or Windows' report), through the calibration; dark when off;"
         " grey outline when Foxglove can't read them. Click = Identify.", 660)

    # Battery list with thumbnails
    bx = x0 + 720
    card(c, bx, 20, 412, 300, "Batteries")
    bats = [("Left half", "USB · 100%", lambda a, b, w, h: mini_keyboard(c, a, b, w * 0.55, h, tint=ACCENT)),
            ("Right half", "82%", lambda a, b, w, h: mini_keyboard(c, a + w * 0.45, b, w * 0.55, h, tint=ACCENT)),
            ("G502 mouse", "64%", lambda a, b, w, h: mini_mouse(c, a, b, w, h, (0x5F, 0xC4, 0x8E))),
            ("Xbox controller", "40%", lambda a, b, w, h: mini_controller(c, a, b, w, h)),
            ("JBL headset", "15%  low", lambda a, b, w, h: mini_headset(c, a, b, w, h))]
    y = 52
    for name, lvl, draw in bats:
        thumb_frame(c, bx + 16, y, draw, w=44, h=30)
        c.text(bx + 72, y + 6, name, 12.5, TEXT)
        c.text(bx + 300, y + 6, lvl, 12.5, WARN if "low" in lvl else MUTED, "mono")
        y += 46
    note(c, bx + 16, y, "Battery rows: 44×30, no tint for devices whose lights Foxglove doesn't"
         " drive (the shape alone).", 380)

    # The shape vocabulary
    card(c, bx, 336, 412, 244, "Shapes, from kind + zones")
    shapes = [("Keyboard", lambda a, b, w, h: mini_keyboard(c, a, b, w, h, tint=ACCENT)),
              ("Split", lambda a, b, w, h: mini_keyboard(c, a, b, w, h, split=True, tint=ACCENT)),
              ("Case", lambda a, b, w, h: mini_case(c, a, b, w, h, [ACCENT, ACCENT], strip=ACCENT)),
              ("Cooler", lambda a, b, w, h: mini_cooler(c, a, b, w, h, ACCENT)),
              ("Strip", lambda a, b, w, h: mini_strip(c, a, b, w, h, [ACCENT] * 7)),
              ("Mouse", lambda a, b, w, h: mini_mouse(c, a, b, w, h, ACCENT)),
              ("Headset", lambda a, b, w, h: mini_headset(c, a, b, w, h, ACCENT)),
              ("Controller", lambda a, b, w, h: mini_controller(c, a, b, w, h)),
              ("Lamps only", lambda a, b, w, h: mini_lamps(c, a, b, w, h, [ACCENT] * 12)),
              ("Unknown", lambda a, b, w, h: mini_unknown(c, a, b, w, h))]
    for i, (name, draw) in enumerate(shapes):
        sx = bx + 16 + (i % 5) * 78
        sy = 372 + (i // 5) * 96
        thumb_frame(c, sx, sy, draw)
        c.text(sx + 28, sy + 46, name, 11, MUTED, anchor="ma")

    # Keyboard thumbnail = the real layout, at thumb size
    card(c, x0, 600, 1132, 196, "A keyboard's thumbnail is its own layout, shrunk")
    unit = 6.2
    draw_keys(c, x0 + 30, 650, unit * 1.8, corne_half(), zone_of=lambda i, x, y: shade(ACCENT, 0.5 + 0.08 * (int(x) % 6)), gap=0.1)
    draw_keys(c, x0 + 30 + 7.8 * unit * 1.8, 650, unit * 1.8, corne_half(True),
              zone_of=lambda i, x, y: shade(ACCENT, 0.5 + 0.08 * (int(x) % 6)), gap=0.1)
    c.text(x0 + 30, 744, "Eyelash Corne: 6 zones per half, by column", 11.5, MUTED)
    draw_keys(c, x0 + 360, 650, unit * 1.3, full_size(),
              zone_of=lambda i, x, y: [(0x7F, 0xB0, 0xE0), (0x5F, 0xC4, 0x8E), (0xF0, 0xD2, 0x3C)][int(x // 7.6) % 3], gap=0.1)
    c.text(x0 + 360, 744, "K5 Pro: per-key, shown as its colours", 11.5, MUTED)
    note(c, x0 + 640, 650, "Same layout source as the Keyboard page (decision 2). At 56×38 the"
         " keys merge into rows, which is the point: an outline and its colours at a glance."
         " Per-product art never ships; a board without a layout gets the plain Keyboard shape.", 440)
    c.save(os.path.join(out, "ux-1105-proposed-3-thumbs.png"))


# --- 4: Lights page declutter -----------------------------------------------------------------

def lights(out):
    W, H = 1240, 1020
    c = Canvas(W, H)
    x0, y0, cw = shell(c, W, H, "Lights")
    # Keyboard card
    y = y0
    card(c, x0, y, cw, 360, "Eyelash Corne")
    # status highlights row
    yy = y + 44
    xx = x0 + 16
    xx += chip(c, xx, yy, "Lights now: Foxglove", TEXT) + 6
    xx += info(c, xx, yy + 1) + 8
    xx += chip(c, xx, yy, "6 zones per half", TEXT) + 6
    c.text(xx + 2, yy + 2, "Show zones", 12, ACCENT)
    xx += c.tw("Show zones", 12) + 16
    xx += chip(c, xx, yy, "calibrated", OK) + 6
    xx += chip(c, xx, yy, "◑ dims on battery", TEXT) + 6
    # left column: controls
    lx, ly = x0 + 16, yy + 36
    pw = pill(c, lx, ly, "Off")
    pill(c, lx + pw + 6, ly, "On", True)
    caps(c, lx + 520, ly + 6, "Effect")
    ex = lx
    for i, e in enumerate(["Solid", "Breathe", "Spectrum", "Swirl"]):
        ex += pill(c, ex, ly + 32, e, i == 0) + 6
    caps(c, lx, ly + 70, "Colour")
    sx = lx
    for i, rgb in enumerate(SWATCHES):
        sx += swatch(c, sx, ly + 88, rgb, i == 0)
    pill(c, sx, ly + 90, "Custom")
    c.text(lx, ly + 126, "Brightness", 12.5)
    c.text(lx + 530, ly + 126, "80%", 12.5, TEXT, "mono", anchor="ra")
    slider(c, lx, ly + 142, 530, 0.8)
    c.text(lx, ly + 170, "Speed", 12.5)
    c.text(lx + 530, ly + 170, "3", 12.5, TEXT, "mono", anchor="ra")
    slider(c, lx, ly + 186, 530, 0.5)
    bw = button(c, lx, ly + 220, "Apply colour", primary=True)
    c.rect(lx + bw + 8, ly + 225, 28, 14, fill=ACCENT, r=3)
    # right column: halves + switches
    rx = x0 + 600
    caps(c, rx, ly, "Both halves now")
    for i, (n, t) in enumerate([("Left", "Solid · 80%"), ("Right", "Solid · 80%")]):
        c.text(rx, ly + 22 + i * 22, n, 12.5, MUTED)
        c.rect(rx + 52, ly + 25 + i * 22, 28, 14, fill=ACCENT, r=3)
        c.text(rx + 88, ly + 22 + i * 22, t, 12.5)
    button(c, rx, ly + 74, "Refresh")
    button(c, rx + 76, ly + 74, "Calibrate…")
    yy2 = ly + 112
    for label, on in [("Follow Windows accent", False), ("Follow the wallpaper", True),
                      ("Right follows left", True), ("Dim on battery", True)]:
        w = checkbox(c, rx, yy2, label, on)
        info(c, rx + w + 6, yy2 + 1)
        yy2 += 26
    c.text(rx + 22, yy2, "Adjust levels…", 12, ACCENT)
    # one tooltip open, to show where the words went
    c.text(rx + 150, ly + 152, "↖", 13, STRONG, "sym")
    tooltip(c, rx + 40, ly + 160, 300,
            "The underglow, and the lights set to match the keyboard, take the accent of the wallpaper"
            " on screen whenever it changes. It comes from the wallpaper's Foxglove palette"
            " (foxglove.json), else from its picture; a grey picture gives the Windows accent colour."
            " A running scene keeps the lights until it ends.", 11.5)

    # Other devices, decluttered
    y = y0 + 380
    card(c, x0, y, cw, 420, "Other devices")
    info(c, x0 + 16 + c.tw("Other devices", 14.5, "hud") + 8, y + 13)
    yy = y + 44
    c.rect(x0 + 16, yy, cw - 32, 28, fill=(0x2A, 0x22, 0x10), outline=(0x5A, 0x48, 0x20), r=5)
    c.text(x0 + 28, yy + 6, "⚠  OMEN Light Studio may override the case", 12.5, WARN)
    info(c, x0 + 28 + c.tw("⚠  OMEN Light Studio may override the case", 12.5) + 8, yy + 6, WARN)
    yy += 40
    # one open device row
    c.line(x0 + 1, yy, x0 + cw - 1, yy, LINE)
    thumb_frame(c, x0 + 16, yy + 10, lambda a, b, w, h: mini_case(c, a, b, w, h, [ACCENT, ACCENT, (0xFF, 0x5F, 0x3C)], strip=ACCENT))
    c.text(x0 + 84, yy + 10, "OMEN 45L case", 13, STRONG)
    c.text(x0 + 90 + c.tw("OMEN 45L case", 13), yy + 10, "Built-in support · calibrated", 12, MUTED)
    c.text(x0 + 84, yy + 30, "5 zones", 12, TEXT)
    info(c, x0 + 84 + c.tw("5 zones", 12) + 6, yy + 30)
    bx = x0 + cw - 16
    for lbl in ["⋯", "Identify", "Off", "Calibrate…"]:
        bx -= c.tw(lbl, 12.5) + 20 + 6
        button(c, bx, yy + 12, lbl)
    yy += 60
    zx = x0 + 84
    for i, (zn, col) in enumerate([("Fan 1", ACCENT), ("Fan 2", ACCENT), ("Fan 3", (0xFF, 0x5F, 0x3C)),
                                   ("Logo", ACCENT), ("Strip", ACCENT)]):
        c.rect(zx, yy, 40, 20, fill=col, outline=CONTROL_LINE, r=4)
        c.text(zx + 48, yy + 2, zn, 12)
        zx += 118
    note(c, x0 + 84, yy + 26, "Design note: each zone's #rrggbb box moves into its swatch's popup, one click away.", 600, 11)
    yy += 52
    caps(c, x0 + 84, yy + 4, "All zones")
    sx = x0 + 160
    for rgb in SWATCHES:
        sx += swatch(c, sx, yy, rgb, r=10)
    yy += 32
    button(c, x0 + 84, yy, "Apply colours", primary=True)
    w = checkbox(c, x0 + 220, yy + 2, "Match keyboard", True)
    info(c, x0 + 220 + w + 6, yy + 3)
    yy += 40
    # collapsed rows
    for name, ctl, draw in [
        ("Keychron K5 Pro", "Over VIA · read-only · Show in Foxglove",lambda a, b, w, h: mini_keyboard(c, a, b, w, h, tint=(0x7F, 0xB0, 0xE0), rows=4)),
        ("G502 mouse", "Windows lighting · 2 lamps", lambda a, b, w, h: mini_mouse(c, a, b, w, h, (0x5F, 0xC4, 0x8E)))]:
        c.line(x0 + 1, yy, x0 + cw - 1, yy, LINE)
        thumb_frame(c, x0 + 16, yy + 8, draw, w=48, h=32)
        c.text(x0 + 76, yy + 10, name, 13, STRONG)
        c.text(x0 + 82 + c.tw(name, 13), yy + 10, ctl, 12, MUTED)
        if name.startswith("G502"):
            chip(c, x0 + 76, yy + 28, "in front only", WARN, outline=(0x5A, 0x48, 0x20))
            info(c, x0 + 76 + c.tw("in front only", 11.5) + 22, yy + 28, WARN)
        c.text(x0 + cw - 30, yy + 12, "▸", 13, MUTED)
        yy += 48
    c.line(x0 + 1, yy, x0 + cw - 1, yy, LINE)
    c.text(x0 + 16, yy + 12, "▸  More sources", 12.5, TEXT)
    c.text(x0 + 140, yy + 13, "OpenRGB off · Add a device…", 12, MUTED)
    note(c, 36, 470, "Each sentence on the page today is kept: in a chip, an ⓘ, a tooltip, or a"
         " collapsed row. The doc's inventory lists where each one went.", 170)
    c.save(os.path.join(out, "ux-1105-proposed-4-lights.png"))


def lights_states(out):
    """The amber cases kept visible but short, and the expander open."""
    W, H = 1180, 380
    c = Canvas(W, H)
    x0, cw = 24, 1132
    c.text(x0, 18, "Amber stays on the page, in one short line; the long text moves behind ⓘ or"
           " the button", 15, STRONG)
    y = 52
    caps(c, x0, y, "Now (today's text)")
    caps(c, x0 + 580, y, "Proposed")
    rows = [
        ("Windows Dynamic Lighting holds the keyboard's lights. Turn off Settings > Personalization >"
         " Dynamic Lighting > Eyelash Corne > “Use Dynamic Lighting on my devices” to give them"
         " back. The scene “Dusk” comes back then.",
         "Windows has the keyboard's lights · Dusk waits", "Open Dynamic Lighting settings"),
        ("Windows Dynamic Lighting holds the keyboard's lights; Foxglove draws through it while its"
         " window is in front. Turn off Settings > ... to give them back.",
         "Windows has the lights · Foxglove draws through it in front", "Open Dynamic Lighting settings"),
        ("OMEN Light Studio is running and may override Foxglove's colours on OMEN 45L case.",
         "OMEN Light Studio may override the case", None),
        ("Host: an IP address or localhost; port: 1 to 65535.", "Host or port not valid", None),
        ("Another app has these lights now. Settings > Personalization > Dynamic Lighting sets which app"
         " comes first.", "Another app has these lights", "Open Dynamic Lighting settings"),
    ]
    y += 24
    for now, short, btn in rows:
        hh = c.wrap(x0, y, now, 540, 12, WARN) - y
        c.rect(x0 + 580, y - 4, 552, 28, fill=(0x2A, 0x22, 0x10), outline=(0x5A, 0x48, 0x20), r=5)
        c.text(x0 + 592, y + 2, "⚠  " + short, 12.5, WARN)
        ix = x0 + 592 + c.tw("⚠  " + short, 12.5) + 8
        info(c, ix, y + 2, WARN)
        if btn:
            bw = c.tw(btn, 11.5) + 16
            c.rect(x0 + 1132 - bw - 8, y, bw, 20, fill=CONTROL, outline=CONTROL_LINE, r=4)
            c.text(x0 + 1132 - bw, y + 2, btn, 11.5, TEXT)
        y += max(hh, 30) + 14
    note(c, x0, y + 6, "Rule: the amber line names the problem and who has the lights (≤ 60"
         " characters). The fix lives on its button; the full sentence, Settings path included, is the"
         " ⓘ tooltip and the copy the health view keeps. Nothing amber is ever collapsed.", 1100)
    c.save(os.path.join(out, "ux-1105-proposed-4b-amber.png"))


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(out, exist_ok=True)
    focus(out)
    layouts(out)
    thumbs(out)
    lights(out)
    lights_states(out)


if __name__ == "__main__":
    main()
