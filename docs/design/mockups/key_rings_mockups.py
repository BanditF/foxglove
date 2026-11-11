"""Mockups for key-press rings on the Scenes page (#490 UI, docs/key-rings-ux.md). Python + Pillow.

Run: python docs/design/mockups/key_rings_mockups.py [out_dir]
Writes ux-key-rings-proposed-*.png (default: C:/Users/fuchs/splitkeeb/review/key-rings) and fails
if any drawn label is under 4.5:1. Colours are the Hideout mockups' dark tokens with the Bandit
orange; swatches are lights_page::SWATCHES.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402  (fonts, contrast, rounded boxes)
from hideout_tabs_mockups import F, F_B, F_H, F_S, F_TINY, rr  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\review\key-rings"
CANVAS, SURFACE, BORDER = m.CANVAS, m.SURFACE, m.BORDER
TEXT, MUTED, ACCENT, ON_ACCENT, CHROME = m.TEXT, m.MUTED, m.ACCENT, m.ON_ACCENT, m.CHROME
DISABLED = (0x9A, 0x93, 0x8D)
SWATCHES = [(0xF0, 0x8A, 0x3C), (0xFF, 0x5F, 0x3C), (0xF0, 0xD2, 0x3C),
            (0x5F, 0xC4, 0x8E), (0x7F, 0xB0, 0xE0), (0xE6, 0xE6, 0xE6)]
text = m.text


def caps(d, x, y, s, bg=SURFACE):
    text(d, (x, y), s.upper(), MUTED, F_TINY, bg, f"caps {s}")
    return y + 20


def pills(d, x, y, labels, sel, bg=SURFACE, new=None):
    for i, lab in enumerate(labels):
        w = d.textlength(lab, font=F) + 24
        if i == sel:
            rr(d, [x, y, x + w, y + 28], fill=ACCENT, r=14)
            text(d, (x + 12, y + 4), lab, ON_ACCENT, F, ACCENT, "pill on")
            if new == i:
                rr(d, [x - 3, y - 3, x + w + 3, y + 31], outline=TEXT, r=16, w=2)  # focus ring
        else:
            rr(d, [x, y, x + w, y + 28], outline=BORDER, r=14)
            text(d, (x + 12, y + 4), lab, TEXT, F, bg, f"pill {lab}")
        x += w + 8
    return y + 38


def slider(d, x, y, w, label, value, frac):
    text(d, (x, y), label, TEXT, F, SURFACE, f"slider {label}")
    vw = d.textlength(value, font=F)
    text(d, (x + w - vw, y), value, MUTED, F, SURFACE, "slider value")
    yy = y + 30
    d.rounded_rectangle([x, yy, x + w, yy + 4], 2, fill=BORDER)
    d.rounded_rectangle([x, yy, x + int(w * frac), yy + 4], 2, fill=ACCENT)
    cx = x + int(w * frac)
    d.ellipse([cx - 8, yy - 6, cx + 8, yy + 10], fill=TEXT)
    return yy + 26


def checkbox(d, x, y, label, on, enabled=True):
    col = TEXT if enabled else DISABLED
    rr(d, [x, y + 2, x + 16, y + 18], fill=(ACCENT if on and enabled else None),
       outline=(ACCENT if on and enabled else DISABLED), r=3)
    if on:
        d.line([(x + 4, y + 10), (x + 7, y + 14), (x + 13, y + 5)],
               fill=(ON_ACCENT if enabled else DISABLED), width=2)
    text(d, (x + 26, y), label, col, F, SURFACE, f"box {label}")
    return y + 28


def weak(d, x, y, s, bg=SURFACE):
    text(d, (x, y), s, MUTED, F_S, bg, f"weak {s[:16]}")
    return y + 20


def card(d, box, title):
    rr(d, box, fill=SURFACE, outline=BORDER, r=10)
    text(d, (box[0] + 20, box[1] + 14), title, TEXT, F_H, SURFACE, f"card {title}")
    return box[1] + 54


def note(d, x, y, lines, title=None):
    if title:
        text(d, (x, y), title, ACCENT, F_B, CANVAS, "note title")
        y += 24
    for ln in lines:
        text(d, (x, y), ln, MUTED, F_S, CANVAS, "note")
        y += 19
    return y + 8


def effect_card():
    im, d = m.new(1360, 800)
    m.heading(d, "Scenes > Effect: Ripple, rings started by key presses (proposed)")
    x0, y0 = 30, 70
    y = card(d, [x0, y0, x0 + 760, y0 + 690], "Effect")
    x = x0 + 20
    y = pills(d, x, y, ["Scanline", "Sweep", "Ripple", "Breathe", "Gradient", "Drift"], 2)
    y = pills(d, x, y - 4, ["Two-tone", "Tide effect", "Screen glow"], -1)
    y += 4
    y = caps(d, x, y, "Rings start")
    y = pills(d, x, y, ["On a loop", "On each key press"], 1, new=1)
    y = caps(d, x, y, "Colour")
    sx = x
    for i, c in enumerate(SWATCHES):
        d.ellipse([sx, y, sx + 28, y + 28], fill=c)
        if i == 0:
            d.ellipse([sx - 4, y - 4, sx + 32, y + 32], outline=TEXT, width=2)
        sx += 40
    y += 44
    y = caps(d, x, y, "Background")
    y = pills(d, x, y, ["Night", "Off"], 0)
    y = slider(d, x, y, 720, "Speed", "8 of 10", 7 / 9)
    y = slider(d, x, y, 720, "Width", "60 mm", (60 - 20) / 380)
    y += 6
    rr(d, [x, y, x + 300, y + 30], outline=BORDER, r=5)
    text(d, (x + 10, y + 6), "Name this look", DISABLED, F, SURFACE, "placeholder")
    rr(d, [x + 310, y, x + 420, y + 30], outline=BORDER, r=5)
    text(d, (x + 322, y + 6), "Save look", TEXT, F, SURFACE, "save")
    y += 50
    weak(d, x, y, "Only when the right half's firmware is older (state C):")
    y += 22
    text(d, (x, y), "Rings show on the left half only. The right half needs a firmware update.",
         TEXT, F_S, SURFACE, "right-old line")
    text(d, (x, y + 22), "Update firmware", ACCENT, F_S, SURFACE, "link on surface")

    nx = 820
    ny = note(d, nx, 72, [
        "New radio group under the effect pills, Ripple only.",
        "Default \"On a loop\": today's Ripple, unchanged.",
        "\"On each key press\" reuses Colour, Background,",
        "Speed (how fast a ring grows and fades) and Width.",
        "No badge, no \"on-board\": the keyboard draws the rings",
        "itself when it can, else Foxglove does. Same look.",
    ], "What changes")
    ny = note(d, nx, ny, [
        "On a loop: \"One ring after another from the",
        "keyboard's middle\"",
        "On each key press: \"A ring grows from each key you",
        "press. Foxglove never sees what you type.\"",
    ], "Pill hovers (also the AccessKit descriptions)")
    ny = note(d, nx, ny, [
        "Radio group \"Rings start\", 2 radio buttons (n of 2).",
        "Tab lands on the picked pill; arrows move and pick (#1095).",
        "Sliders keep their labels and spoken values (\"8 of 10\").",
        "The white ring is the keyboard focus ring.",
    ], "Accessibility")
    note(d, nx, ny, [
        "Built-in look \"Ripple from the keys\" becomes this mode",
        "(speed 8, width 60 mm, Bandit orange on Night).",
        "Its name finally matches what it does.",
    ], "Looks card")
    return im


def triggers():
    im, d = m.new(1360, 760)
    m.heading(d, "Scenes > Triggers while the look's rings start on key press (proposed)")
    states = [
        ("A. Keyboard draws the rings itself", True, False, None),
        ("B. Keyboard can't: Foxglove draws them", True, True,
         "This keyboard can't draw key-press rings by itself, so they stop when Foxglove quits."),
    ]
    x0 = 30
    for i, (title, ripple_dis, keep_dis, reason) in enumerate(states):
        bx = x0 + i * 660
        text(d, (bx, 66), title, ACCENT, F_B, CANVAS, "state title")
        y = card(d, [bx, 94, bx + 640, 470], "Triggers")
        x = bx + 20
        y = checkbox(d, x, y, "Start with this profile", False)
        y = weak(d, x, y, "Profile: Gaming. Programs switch profiles under Settings, Profile.")
        y += 4
        y = checkbox(d, x, y, "Ripple on key press", True, enabled=False)
        y = weak(d, x + 26, y, "The Ripple effect already starts a ring at each key.")
        y = checkbox(d, x, y, "Sweep on layer change", False)
        y = weak(d, x, y, "Ripples and sweeps show while a scene runs.")
        y += 4
        y = checkbox(d, x, y, "Keep running on the keyboard", not keep_dis, enabled=not keep_dis)
        if reason:
            y = weak(d, x + 26, y, reason[:52])
            y = weak(d, x + 26, y - 4, reason[52:].strip())
    note(d, 30, 500, [
        "\"Ripple on key press\" stays as it is for every other look (rings over a scanline, a sweep ...).",
        "With Ripple on each key press it would double every ring, so it shows ticked and greyed, with the reason",
        "as a visible line (not hover only), which is also the box's AccessKit description.",
        "",
        "\"Keep running on the keyboard\" is the one place where who draws the rings changes what the user gets:",
        "after quitting, only a keyboard that draws them itself keeps them. State B greys the box and says why,",
        "unlike Screen glow, which hides it (that can never work; this one can, after a firmware update).",
        "State A needs no text: it just works, quit or not.",
        "",
        "Nowhere else does the page say where the rings are drawn. Diagnostics (Troubleshooting > Copy report)",
        "gets one line: \"keyboard: key rings drawn on the keyboard\" or \"... drawn by Foxglove (no KEY_RINGS)\".",
    ], "Why")
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("effect-card", effect_card), ("triggers", triggers)]:
        path = os.path.join(OUT, f"ux-key-rings-proposed-{name}.png")
        fn().save(path)
        print(path)
    worst = {}
    for label, fg, bg in m.PAIRS:
        worst[label] = min(m.contrast(fg, bg), worst.get(label, 99))
    print(f"min over {len(worst)} pairs: {min(worst.values()):.2f}:1 ({min(worst, key=worst.get)})")
    bad = {k: round(v, 2) for k, v in worst.items() if v < 4.5}
    if bad:
        print("UNDER 4.5:1:", bad)
        sys.exit(1)


if __name__ == "__main__":
    main()
