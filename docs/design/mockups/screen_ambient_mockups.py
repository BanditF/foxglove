"""Mockups for Screen glow, the screen ambient effect (#496). Python + Pillow, no app code.

Run: python docs/design/mockups/screen_ambient_mockups.py [out_dir]
Writes ux-screen-ambient-proposed-*.png (default: C:/Users/fuchs/splitkeeb/review/screen-ambient)
and fails if any drawn label is under 4.5:1. Colours are Foxglove's DARK tokens (app theme.rs:
ground, surface, surface_raised, line, muted) with the Bandit orange, as in knob_exclude_mockups.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402  (font, contrast, rounded boxes)
from hideout_tabs_mockups import F, F_B, F_H, F_S, F_TINY, ICON, rr  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\review\screen-ambient"
GROUND = (0x0E, 0x12, 0x11)
SURFACE = (0x16, 0x1C, 0x1A)
RAISED = (0x1D, 0x24, 0x22)
LINE = (0x2A, 0x33, 0x30)
MUTED = (0x8E, 0x9A, 0x94)
TEXT = (0xE8, 0xEE, 0xEB)
ACCENT = (0xFF, 0x8A, 0x2A)
ON_ACCENT = (0x1A, 0x0E, 0x04)
OK = (0x6C, 0xD4, 0x8E)
WARN = (0xF2, 0xC1, 0x4E)
PAIRS = []
EYE = "\ue7b3"  # MDL2 "view": the screen-reading mark (tray, status line)


def text(d, xy, s, fill, f=F, bg=None, label=None):
    d.text(xy, s, fill=fill, font=f)
    if bg is not None:
        PAIRS.append((label or s[:24], fill, bg))


def button(d, x, y, label, primary=False, on=SURFACE, f=F, focus=False):
    bw = d.textlength(label, font=f) + 24
    if primary:
        rr(d, [x, y, x + bw, y + 30], fill=ACCENT, r=5)
        text(d, (x + 12, y + 5), label, ON_ACCENT, f, ACCENT, "primary button")
    else:
        rr(d, [x, y, x + bw, y + 30], outline=LINE, r=5)
        text(d, (x + 12, y + 5), label, TEXT, f, on, f"button {label}")
    if focus:
        rr(d, [x - 3, y - 3, x + bw + 3, y + 33], outline=TEXT, r=7, w=2)
    return x + bw + 10


def pills(d, x, y, items, active, on=SURFACE):
    for it in items:
        w = d.textlength(it, font=F) + 22
        if it == active:
            rr(d, [x, y, x + w, y + 28], fill=ACCENT, r=14)
            text(d, (x + 11, y + 4), it, ON_ACCENT, F, ACCENT, "pill on")
        else:
            rr(d, [x, y, x + w, y + 28], outline=LINE, r=14)
            text(d, (x + 11, y + 4), it, TEXT, F, on, f"pill {it}")
        x += w + 8
    return x


def card(d, x, y, w, h, title):
    rr(d, [x, y, x + w, y + h], fill=SURFACE, outline=LINE, r=10)
    text(d, (x + 18, y + 12), title, TEXT, F_H, SURFACE)


# The room: two monitors, the case to the right, the keyboard halves below the primary.
EDGE = {  # live edge colours (a sunset scene on the left screen, a blue UI on the right)
    "l_top": (0xE0, 0x7A, 0x3A), "l_left": (0xC8, 0x4A, 0x5A), "l_bottom": (0x7A, 0x3A, 0x6A),
    "r_top": (0x3A, 0x7A, 0xD0), "r_right": (0x2A, 0x5A, 0xA0), "r_bottom": (0x3A, 0x4A, 0x8A),
}


def room(d, x, y, live):
    """The Room card's map. live: the edge strips in their colours and a line from each lit
    device to the part of the edge it follows; else a still drawing (before Start)."""
    mon_l = [x + 30, y + 20, x + 230, y + 135]
    mon_r = [x + 240, y + 30, x + 400, y + 125]
    for b in (mon_l, mon_r):
        rr(d, b, outline=MUTED, r=3)
    strip = 8
    grey = (0x3A, 0x44, 0x40)

    def bar(box, key):
        d.rectangle(box, fill=EDGE[key] if live else grey)

    bar([mon_l[0], mon_l[1], mon_l[2], mon_l[1] + strip], "l_top")
    bar([mon_l[0], mon_l[1], mon_l[0] + strip, mon_l[3]], "l_left")
    bar([mon_l[0], mon_l[3] - strip, mon_l[2], mon_l[3]], "l_bottom")
    bar([mon_r[0], mon_r[1], mon_r[2], mon_r[1] + strip], "r_top")
    bar([mon_r[2] - strip, mon_r[1], mon_r[2], mon_r[3]], "r_right")
    bar([mon_r[0], mon_r[3] - strip, mon_r[2], mon_r[3]], "r_bottom")
    text(d, (mon_l[0] + 60, mon_l[1] + 50), "Main screen", MUTED, F_S, SURFACE, "map label")
    text(d, (mon_r[0] + 36, mon_r[1] + 40), "Right screen", MUTED, F_S, SURFACE, "map label")
    # case (3 zone dots) to the right
    cx = x + 440
    rr(d, [cx, y + 40, cx + 50, y + 170], outline=MUTED, r=4)
    zones = [(cx + 25, y + 70), (cx + 25, y + 105), (cx + 25, y + 140)]
    for i, (zx, zy) in enumerate(zones):
        col = [EDGE["r_right"], EDGE["r_right"], EDGE["r_bottom"]][i] if live else grey
        d.ellipse([zx - 7, zy - 7, zx + 7, zy + 7], fill=col)
        if live:
            d.line([(zx - 7, zy), (mon_r[2], min(max(zy, mon_r[1] + 10), mon_r[3] - 4))], fill=MUTED, width=1)
    text(d, (cx - 2, y + 176), "Case", MUTED, F_S, SURFACE, "map label")
    # keyboard halves below the main screen
    halves = [[x + 50, y + 165, x + 120, y + 195], [x + 140, y + 165, x + 210, y + 195]]
    cols = [EDGE["l_bottom"], (0x9A, 0x40, 0x60)]
    for i, hb in enumerate(halves):
        rr(d, hb, fill=cols[i] if live else grey, outline=MUTED, r=4)
        if live:
            mx = (hb[0] + hb[2]) // 2
            d.line([(mx, hb[1]), (mx, mon_l[3])], fill=MUTED, width=1)
    text(d, (x + 92, y + 200), "Keyboard", MUTED, F_S, SURFACE, "map label")
    # mouse, shown only
    rr(d, [x + 240, y + 170, x + 262, y + 198], outline=LINE, r=8)
    text(d, (x + 270, y + 176), "Mouse (shown only)", MUTED, F_S, SURFACE, "map label")


def page(title):
    im, d = m.new(1400, 860)
    text(d, (30, 22), title, TEXT, m.F_T, GROUND)
    return im, d


def effects_list(d, x, y, active):
    text(d, (x, y), "EFFECTS", MUTED, F_TINY, SURFACE)
    names = ["Scanline", "Sweep", "Ripple", "Breathe", "Gradient", "Drift", "Two-tone", "Tide effect", "Screen glow"]
    yy = y + 20
    for n in names:
        if n == active:
            rr(d, [x - 6, yy - 2, x + 200, yy + 22], fill=RAISED, r=4)
            d.rectangle([x - 6, yy - 2, x - 3, yy + 22], fill=ACCENT)
            text(d, (x + 4, yy), n, TEXT, F_B, RAISED, "effect selected")
        else:
            text(d, (x + 4, yy), n, TEXT, F, SURFACE, "effect")
        yy += 26
    return yy


def consent():
    im, d = page("1  First Start of Screen glow: the permission card, in the Effect card")
    card(d, 30, 70, 560, 290, "Room")
    room(d, 60, 110, live=False)
    text(d, (48, 330), "Not running. Screen glow shows your screen's colours here once it runs.", MUTED, F_S, SURFACE)
    card(d, 30, 380, 560, 440, "Looks")
    effects_list(d, 52, 420, "Screen glow")
    text(d, (52, 670), "Screen glow is never in Wallpapers, and Save as look keeps its Smoothing.", MUTED, F_S, SURFACE)

    x, y = 620, 70
    card(d, x, y, 750, 560, "Effect")
    text(d, (x + 18, y + 50), "Screen glow", TEXT, F_B, SURFACE)
    text(d, (x + 18, y + 72), "Your lights take the colours at the edges of your screens, like a TV's ambient light.", MUTED, F, SURFACE)
    # permission card
    py = y + 110
    rr(d, [x + 18, py, x + 732, py + 300], fill=RAISED, outline=ACCENT, r=8)
    d.text((x + 36, py + 18), EYE, fill=TEXT, font=ICON)
    text(d, (x + 62, py + 12), "Let Foxglove read your screen?", TEXT, F_H, RAISED)
    lines = [
        "To colour your lights, Foxglove looks at everything on your screens, up to 20 times a",
        "second, while Screen glow runs.",
        "",
        "·  The picture never leaves Foxglove and is never saved. Only the edge colours go to the lights.",
        "·  It pauses while a game is full screen, the PC is locked or the display is off.",
        "·  While it runs, the tray icon shows an eye. Stop it there or here at any time.",
    ]
    for i, ln in enumerate(lines):
        text(d, (x + 36, py + 52 + i * 24), ln, TEXT, F, RAISED, "consent body")
    bx = button(d, x + 36, py + 216, "Allow and start", primary=True, on=RAISED)
    button(d, bx, py + 216, "Not now", on=RAISED, focus=True)
    text(d, (x + 36, py + 260), "Foxglove remembers your answer. You can take it back under this effect.", MUTED, F_S, RAISED)
    text(d, (x + 18, y + 430), "Start and the permission card:", MUTED, F_S, SURFACE)
    text(d, (x + 18, y + 452), "Start on Screen glow shows this card instead of starting. Not now closes it; nothing runs.", MUTED, F_S, SURFACE)
    text(d, (x + 18, y + 474), "Keyboard focus lands on Not now, so a stray Enter never says yes. Esc = Not now.", MUTED, F_S, SURFACE)
    text(d, (x + 18, y + 496), "No Smoothing, no preview: nothing reads the screen before Allow.", MUTED, F_S, SURFACE)
    notes = [
        "Why inline and not a Windows dialog: it sits where the user chose the effect, keeps the page's words, and can't hide behind a game.",
        "A profile's Start with this profile, or resuming after a restart, never asks: without a yes it doesn't start, and the status line says why.",
    ]
    for i, ln in enumerate(notes):
        text(d, (620, 660 + i * 24), ln, MUTED, F_S, GROUND)
    return im


def running():
    im, d = page("2  Running: the map shows which edge each light follows; one Smoothing control")
    card(d, 30, 70, 560, 360, "Room")
    room(d, 60, 110, live=True)
    d.text((48, 334), EYE, fill=OK, font=ICON)
    text(d, (72, 330), "Running: Screen glow · reading your screen", OK, F, SURFACE, "status ok")
    text(d, (48, 356), "Drag a device to change which edge it follows.", MUTED, F_S, SURFACE)
    bx = button(d, 48, 376 - 4, "Stop")
    text(d, (bx + 4, 376), "Reset positions", ACCENT, F, SURFACE, "link")

    x, y = 620, 70
    card(d, x, y, 750, 420, "Effect")
    text(d, (x + 18, y + 50), "Screen glow", TEXT, F_B, SURFACE)
    text(d, (x + 18, y + 72), "Your lights take the colours at the edges of your screens, like a TV's ambient light.", MUTED, F, SURFACE)
    text(d, (x + 18, y + 112), "SMOOTHING", MUTED, F_TINY, SURFACE)
    pills(d, x + 18, y + 134, ["Quick", "Even", "Calm"], "Even")
    text(d, (x + 18, y + 172), "Quick follows every cut. Calm drifts between colours and never flickers.", MUTED, F_S, SURFACE)
    d.line([(x + 18, y + 210), (x + 732, y + 210)], fill=LINE)
    text(d, (x + 18, y + 224), "Foxglove may read your screen for Screen glow.", TEXT, F, SURFACE)
    text(d, (x + 18, y + 248), "Stop allowing", ACCENT, F, SURFACE, "link")
    text(d, (x + 18, y + 284), "Stop allowing stops Screen glow if it runs and forgets the yes; the next Start asks again.", MUTED, F_S, SURFACE)
    text(d, (x + 18, y + 304), "No colour, palette, speed or direction here: the screen is the colour, and it moves by itself.", MUTED, F_S, SURFACE)
    text(d, (x + 18, y + 324), "Black bars (films in 21:9, 4:3 games) are skipped, so the lights follow the picture.", MUTED, F_S, SURFACE)
    text(d, (x + 18, y + 344), "Triggers: Start with this profile works; Ripple and Sweep draw over the screen's colours.", MUTED, F_S, SURFACE)

    notes = [
        "Mapping is the room map, nothing new: each lit point takes the colour of the nearest stretch of the nearest screen's edge.",
        "Keyboard below the main screen: its bottom edge, left half from the left part. The case beside the right screen: its right edge, top to bottom.",
        "The lines from device to edge show only while Screen glow runs (they are the preview); with it stopped the map is grey.",
        "Monitors follow Windows' display layout and can't be dragged, as today. A device far from every screen follows its nearest one anyway.",
    ]
    for i, ln in enumerate(notes):
        text(d, (30, 520 + i * 24), ln, MUTED, F_S, GROUND)
    return im


def tray():
    im, d = page("3  Always visible while it reads the screen: tray icon, tooltip, tray panel")
    # tray icons
    text(d, (30, 76), "Tray icon (art: new 'screen' variant, 16/20/24/32, white and black)", MUTED, F_S, GROUND)
    states = [("Connected", None), ("Screen glow on", OK), ("Paused", None)]
    for i, (lbl, dot) in enumerate(states):
        ix, iy = 40, 104 + i * 52
        rr(d, [ix, iy, ix + 40, iy + 40], fill=RAISED, r=6)
        d.text((ix + 12, iy + 12),"\ue765", fill=TEXT, font=ICON)  # stands in for the fox glyph
        if dot:
            d.ellipse([ix + 24, iy + 20, ix + 38, iy + 34], fill=GROUND)
            d.text((ix + 25, iy + 21), EYE, fill=dot, font=m.ICON_S)
        if lbl == "Paused":
            d.rectangle([ix + 27, iy + 22, ix + 29, iy + 32], fill=TEXT)
            d.rectangle([ix + 32, iy + 22, ix + 34, iy + 32], fill=TEXT)
        text(d, (ix + 52, iy + 12), lbl, TEXT, F, GROUND, "tray state")
    text(d, (30, 266), "Paused wins over the eye: while", MUTED, F_S, GROUND)
    text(d, (30, 284), "Foxglove is paused nothing reads the screen.", MUTED, F_S, GROUND)
    # tooltip
    text(d, (30, 324), "Tooltip", MUTED, F_S, GROUND)
    rr(d, [30, 346, 360, 414], fill=RAISED, outline=LINE, r=4)
    text(d, (42, 354),"Connected · Screen glow on", TEXT, F, RAISED, "tooltip")
    text(d, (42, 380), "Left 82 % · Right 61 %", MUTED, F_S, RAISED, "tooltip")

    # tray panel
    px, py = 430, 70
    text(d, (px, py + 6), "Tray panel: a Screen glow row on top while it runs", MUTED, F_S, GROUND)
    rr(d, [px, py + 30, px + 420, py + 430], fill=SURFACE, outline=LINE, r=10)
    y = py + 48
    rr(d, [px + 12, y, px + 408, y + 66], fill=RAISED, r=6)
    d.text((px + 26, y + 24), EYE, fill=OK, font=ICON)
    text(d, (px + 52, y + 10), "Screen glow", TEXT, F_B, RAISED)
    text(d, (px + 52, y + 34), "Reading your screen for the lights", MUTED, F_S, RAISED)
    button(d, px + 330, y + 18, "Stop", on=RAISED)
    y += 84
    for cap, val in [("BATTERIES", "Left 82 % · Right 61 %"), ("PROFILE", "Default"), ("LIGHTS", "Scene: Screen glow")]:
        text(d, (px + 18, y), cap, MUTED, F_TINY, SURFACE)
        text(d, (px + 18, y + 18), val, TEXT, F, SURFACE)
        y += 58
    text(d, (px + 18, py + 390), "Open Foxglove →", ACCENT, F, SURFACE, "link")

    # paused row
    qx = 900
    text(d, (qx, 76), "The same row while a gate holds it", MUTED, F_S, GROUND)
    rows = [
        ("Paused while a game is full screen", WARN),
        ("Paused while the PC is locked", MUTED),
        ("Can't read the screen right now", WARN),
    ]
    yy = 100
    for sub, col in rows:
        rr(d, [qx, yy, qx + 420, yy + 66], fill=RAISED, r=6)
        d.text((qx + 14, yy + 24), EYE, fill=col, font=ICON)
        text(d, (qx + 40, yy + 10), "Screen glow", TEXT, F_B, RAISED)
        text(d, (qx + 40, yy + 34), sub, MUTED, F_S, RAISED)
        button(d, qx + 342, yy + 18, "Stop", on=RAISED)
        yy += 80
    text(d, (qx, yy + 4), "The eye stays in the tray only while frames are read.", MUTED, F_S, GROUND)
    text(d, (qx, yy + 24), "Held by a gate: the tray icon drops the eye; this row stays.", MUTED, F_S, GROUND)

    notes = [
        "Stop here is the Scenes page's Stop: the scene ends and the lights get their colours back. Pause Foxglove (tray menu) pauses it with everything else.",
        "The eye is never colour alone: it has a shape, the tooltip says it in words, and screen readers read the row as 'Screen glow, reading your screen, Stop button'.",
        "No sound, no toast on start: the user just clicked Start. One toast only if a profile switch or a restart started it while the window was closed.",
    ]
    for i, ln in enumerate(notes):
        text(d, (30, 560 + i * 24), ln, MUTED, F_S, GROUND)
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("1-consent", consent), ("2-running", running), ("3-tray", tray)]:
        path = os.path.join(OUT, f"ux-screen-ambient-proposed-{name}.png")
        fn().save(path)
        print(path)
    worst = {}
    for label, fg, bg in PAIRS:
        worst[label] = min(m.contrast(fg, bg), worst.get(label, 99))
    print(f"min over {len(worst)} pairs: {min(worst.values()):.2f}:1 ({min(worst, key=worst.get)})")
    bad = {k: round(v, 2) for k, v in worst.items() if v < 4.5}
    if bad:
        print("UNDER 4.5:1:", bad)
        sys.exit(1)


if __name__ == "__main__":
    main()
