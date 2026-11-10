"""Mockups for the knob's "Hide from knob" (#952). Python + Pillow, no app code.

Run: python docs/design/mockups/knob_exclude_mockups.py [out_dir]
Writes ux-knob-exclude-proposed-*.png (default: C:/Users/fuchs/splitkeeb/review/knob-exclude) and
fails if any drawn label is under 4.5:1. Colours are Foxglove's DARK tokens (app theme.rs:
ground, surface, surface_raised, line, muted) with the Bandit orange.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402  (font, contrast, rounded boxes)
from hideout_tabs_mockups import F, F_B, F_H, F_S, F_TINY, ICON, ICON_S, rr  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\review\knob-exclude"
GROUND = (0x0E, 0x12, 0x11)
SURFACE = (0x16, 0x1C, 0x1A)
RAISED = (0x1D, 0x24, 0x22)
LINE = (0x2A, 0x33, 0x30)
MUTED = (0x8E, 0x9A, 0x94)
TEXT = (0xE8, 0xEE, 0xEB)
ACCENT = (0xFF, 0x8A, 0x2A)
ON_ACCENT = (0x1A, 0x0E, 0x04)
PAIRS = []
MUSIC, SPEAKER, HIDE, SHOW, KNOB = "\ue8d6", "\ue767", "\ue7b3", "\ue890", "\ue7f3"


def text(d, xy, s, fill, f=F, bg=None, label=None):
    d.text(xy, s, fill=fill, font=f)
    if bg is not None:
        PAIRS.append((label or s[:24], fill, bg))


def button(d, x, y, label, primary=False, on=SURFACE, f=F):
    bw = d.textlength(label, font=f) + 24
    if primary:
        rr(d, [x, y, x + bw, y + 30], fill=ACCENT, r=5)
        text(d, (x + 12, y + 5), label, ON_ACCENT, f, ACCENT, "primary button")
    else:
        rr(d, [x, y, x + bw, y + 30], outline=LINE, r=5)
        text(d, (x + 12, y + 5), label, TEXT, f, on, f"button {label}")
    return x + bw + 10


def app_icon(d, x, y, tint, glyph=MUSIC):
    d.ellipse([x, y, x + 30, y + 30], fill=tint)
    d.text((x + 8, y + 8), glyph, fill=TEXT, font=ICON)


def flyout(d, x, y, title, note, level=None):
    rr(d, [x, y, x + 360, y + 64], fill=RAISED, outline=LINE, r=8)
    app_icon(d, x + 14, y + 17, (0x3A, 0x6A, 0x4A))
    text(d, (x + 56, y + 12), title, TEXT, F_B, RAISED)
    text(d, (x + 56, y + 36), note, MUTED, F_S, RAISED)
    if level is not None:
        d.rectangle([x + 200, y + 42, x + 340, y + 46], fill=LINE)
        d.rectangle([x + 200, y + 42, x + 200 + int(1.4 * level), y + 46], fill=ACCENT)


def tray_and_popup():
    im, d = m.new(1400, 760)
    text(d, (30, 22), "Hide from knob: from the tray panel while the knob is on an app", TEXT, m.F_T, GROUND)
    # 1 popup (unchanged, click-through)
    text(d, (30, 76), "1  The knob's card (unchanged: click-through, never takes focus)", MUTED, F_S, GROUND)
    flyout(d, 30, 100, "Logi Options+", "1 of 3 · 40 %", 40)
    # 2 tray panel
    px, py = 430, 70
    text(d, (px, py + 6), "2  Tray panel, new Knob row (only while the knob is on an app)", MUTED, F_S, GROUND)
    rr(d, [px, py + 30, px + 400, py + 470], fill=SURFACE, outline=LINE, r=10)
    y = py + 50
    for cap, val in [("BATTERIES", "Left 82 % · Right 61 %"), ("PROFILE", "Default")]:
        text(d, (px + 18, y), cap, MUTED, F_TINY, SURFACE)
        text(d, (px + 18, y + 18), val, TEXT, F, SURFACE)
        y += 58
    d.line([(px + 14, y), (px + 386, y)], fill=LINE)
    y += 12
    text(d, (px + 18, y), "KNOB", MUTED, F_TINY, SURFACE)
    app_icon(d, px + 18, y + 22, (0x3A, 0x6A, 0x4A))
    text(d, (px + 58, y + 20), "Logi Options+", TEXT, F_B, SURFACE)
    text(d, (px + 58, y + 42), "1 of 3 · 40 %", MUTED, F_S, SURFACE)
    bw = d.textlength("Hide from knob", font=F) + 46
    rr(d, [px + 382 - bw, y + 24, px + 382, y + 54], outline=LINE, r=5)
    d.text((px + 392 - bw, y + 32), HIDE, fill=TEXT, font=ICON_S)
    text(d, (px + 410 - bw, y + 28), "Hide from knob", TEXT, F, SURFACE)
    y += 76
    d.line([(px + 14, y), (px + 386, y)], fill=LINE)
    y += 12
    text(d, (px + 18, y), "LIGHTS", MUTED, F_TINY, SURFACE)
    text(d, (px + 18, y + 18), "Bandit gem", TEXT, F, SURFACE)
    text(d, (px + 18, py + 430), "Open Foxglove →", ACCENT, F, SURFACE, "link")
    # 3 after hide
    qx = 870
    text(d, (qx, 76), "3  After the click: the knob moves on, both places say so", MUTED, F_S, GROUND)
    flyout(d, qx, 100, "Logi Options+ hidden", "knob: Spotify · 2 of 2")
    rr(d, [qx, 190, qx + 400, 300], fill=SURFACE, outline=LINE, r=10)
    text(d, (qx + 18, 202), "KNOB", MUTED, F_TINY, SURFACE)
    text(d, (qx + 18, 224), "Logi Options+ is hidden from the knob.", TEXT, F, SURFACE)
    bx = button(d, qx + 18, 254, "Undo")
    text(d, (bx + 4, 259), "Change in Settings", ACCENT, F, SURFACE, "link")
    text(d, (qx, 312), "Undo stays until the panel closes. Later: Settings, Keyboard, Knob.", MUTED, F_S, GROUND)
    notes = [
        "Why not a button on the card: the card is drawn click-through on the overlay so it never steals a click or focus mid-game (#719).",
        "The row shows only while the knob is on an app: on master there is nothing to hide.",
        "Hidden means: the knob's click skips it. Windows' Volume Mixer, the app's own sound and the left screen are untouched.",
        "Matched by program (its file), not by session, so it stays hidden after the app restarts. Its file name only in the tooltip.",
    ]
    for i, ln in enumerate(notes):
        text(d, (30, 600 + i * 24), ln, MUTED, F_S, GROUND)
    return im


def settings():
    im, d = m.new(1400, 800)
    text(d, (30, 22), "Settings, Keyboard page: the Knob card, where hiding is undone", TEXT, m.F_T, GROUND)
    x, y = 30, 80
    rr(d, [x, y, x + 760, y + 600], fill=SURFACE, outline=LINE, r=10)
    d.text((x + 20, y + 22), KNOB, fill=TEXT, font=ICON)
    text(d, (x + 46, y + 16), "Knob", TEXT, F_H, SURFACE)
    text(d, (x + 20, y + 54), "A click on the knob goes to the next app that's playing sound.", MUTED, F, SURFACE)
    text(d, (x + 20, y + 76), "It skips apps that haven't played anything and apps on this list.", MUTED, F, SURFACE)
    text(d, (x + 20, y + 116), "HIDDEN FROM THE KNOB", MUTED, F_TINY, SURFACE)
    rows = [("Logi Options+", "Hidden by you · today", True), ("Discord", "Hidden by you · 11/02", True),
            ("NVIDIA Broadcast", "On Foxglove's list", False)]
    yy = y + 140
    for name, sub, mine in rows:
        rr(d, [x + 20, yy, x + 740, yy + 56], fill=RAISED, r=6)
        app_icon(d, x + 32, yy + 13, (0x4A, 0x45, 0x6A), SPEAKER)
        text(d, (x + 74, yy + 8), name, TEXT, F_B, RAISED)
        text(d, (x + 74, yy + 30), sub, MUTED, F_S, RAISED)
        bw = d.textlength("Show again", font=F) + 24
        rr(d, [x + 728 - bw, yy + 12, x + 728, yy + 44], outline=LINE, r=5)
        text(d, (x + 740 - bw, yy + 17), "Show again", TEXT, F, RAISED, "row button")
        yy += 64
    text(d, (x + 20, yy + 8), "Add an app…", ACCENT, F, SURFACE, "link")
    text(d, (x + 20, yy + 38), "Add an app lists the apps with sound right now (the Volume Mixer's list).", MUTED, F_S, SURFACE)
    d.line([(x + 20, yy + 72), (x + 740, yy + 72)], fill=LINE)
    text(d, (x + 20, yy + 88), "Nothing hidden:", MUTED, F_S, SURFACE)
    text(d, (x + 20, yy + 108), "\"No apps hidden. Use Hide from knob in the tray panel while the knob is on one.\"", TEXT, F_S, SURFACE)
    # side: tooltip + after Show again
    sx = 830
    text(d, (sx, 86), "Row tooltip", MUTED, F_S, GROUND)
    rr(d, [sx, 108, sx + 520, 168], fill=RAISED, outline=LINE, r=6)
    text(d, (sx + 14, 116), "Logi Options+", TEXT, F_B, RAISED)
    text(d, (sx + 14, 140), "LogiOptionsMgr.exe · matched by this file", MUTED, F_S, RAISED)
    text(d, (sx, 200), "After Show again", MUTED, F_S, GROUND)
    rr(d, [sx, 222, sx + 520, 282], fill=RAISED, r=6)
    text(d, (sx + 14, 230), "Discord is back in the knob's cycle.", TEXT, F, RAISED)
    text(d, (sx + 14, 254), "Undo", ACCENT, F, RAISED, "link on raised")
    text(d, (sx, 300), "The row leaves the list; one line with Undo replaces it until", MUTED, F_S, GROUND)
    text(d, (sx, 318), "the page changes. Foxglove's own entries come back with", MUTED, F_S, GROUND)
    text(d, (sx, 336), "Show again too: the user's choice wins over the shipped list.", MUTED, F_S, GROUND)
    text(d, (30, 710), "No restart, no Save button: changes apply at the knob's next click (store-as-you-go, like the other cards).", MUTED, F_S, GROUND)
    text(d, (30, 734), "Saved in Foxglove's settings as program files; the shipped list is a data file Foxglove reads, never written.", MUTED, F_S, GROUND)
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("tray-hide", tray_and_popup), ("settings-undo", settings)]:
        path = os.path.join(OUT, f"ux-knob-exclude-proposed-{name}.png")
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
