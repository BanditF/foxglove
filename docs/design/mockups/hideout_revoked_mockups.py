"""Mockups for a module version its catalog withdrew (#1026). Python + Pillow, no app code.

Run: python docs/design/mockups/hideout_revoked_mockups.py [out_dir]
Writes ux-revoked-proposed-*.png (default: C:/Users/fuchs/splitkeeb/worktrees/_review/h1026)
and fails if any drawn label is under 4.5:1. Reuses the tab, supervisor and plugin-module
mockups' helpers and hideout-shell's DARK tokens. Window-free: it only writes PNGs.
"""

import os
import sys
import unicodedata

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402
from hideout_tabs_mockups import (ACCENT, BG, BORDER, CANVAS, CHROME, DANGER, F, F_B, F_H, F_S,  # noqa: E402
                                  G, ICON, MUTED, SURFACE, TEXT, PAIRS, rr, text)
from hideout_supervisor_mockups import button, module_list  # noqa: E402
import hideout_plugin_modules_mockups as pm  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\worktrees\_review\h1026"
CAP = 160  # characters of the publisher's note drawn on the card (decision 1)

ROWS = [("keyboard", "Keyboard", None), ("lights", "Lights", None), ("scenes", "Scenes", None),
        ("art", "Art", "stop"), ("chat", "Chat", None), ("images", "Images", "off"),
        ("-", "-", None), ("modules", "Modules", None), ("settings", "Settings", None)]


def clean_note(raw):
    """The card's rule for the catalog's reason: plain text, one paragraph, capped.

    Drops control and format characters (C0/C1, bidi overrides, zero-width), folds every run of
    whitespace to one space, trims, and cuts at CAP characters with an ellipsis. Empty after
    cleaning means "no reason given". Nothing is parsed: no markup, no links."""
    out = []
    for ch in raw:
        cat = unicodedata.category(ch)
        if ch in "\t\r\n" or cat == "Zs":
            out.append(" ")
        elif cat in ("Cc", "Cf", "Co", "Cs", "Cn"):
            continue
        else:
            out.append(ch)
    s = " ".join("".join(out).split())
    if len(s) > CAP:
        s = s[:CAP - 1].rstrip() + "\u2026"
    return s


def wrap(d, s, f, width):
    lines, cur = [], ""
    for w in s.split(" "):
        t = (cur + " " + w).strip()
        if d.textlength(t, font=f) > width and cur:
            lines.append(cur)
            cur = w
        else:
            cur = t
    if cur:
        lines.append(cur)
    return lines


def card(d, x, y, w, name, version, note, prev=None, status=None, primary="Check for updates"):
    """The withdrawn card: red mark, title, reason line, optional publisher's note, buttons."""
    note = clean_note(note) if note is not None else ""
    body = [("Its publisher withdrew this version, so Hideout stopped it.", TEXT)]
    note_lines = wrap(d, f"\u201c{note}\u201d", F, w - 56) if note else []
    h = 150 + 22 * len(note_lines) + (24 if note_lines else 0) + (26 if status else 0)
    rr(d, [x, y, x + w, y + h], fill=SURFACE, outline=DANGER, r=6)
    d.text((x + 16, y + 16), G["warn"], fill=DANGER, font=ICON)
    PAIRS.append(("card withdrawn mark", DANGER, SURFACE))
    text(d, (x + 40, y + 12), f"{name} {version} was withdrawn", TEXT, F_B, SURFACE, "card title")
    yy = y + 38
    for s, c in body:
        text(d, (x + 40, yy), s, c, F, SURFACE, "card reason line")
        yy += 22
    if note_lines:
        yy += 4
        text(d, (x + 40, yy), "Publisher\u2019s note:", MUTED, F_S, SURFACE, "note label")
        yy += 20
        for ln in note_lines:
            text(d, (x + 40, yy), ln, TEXT, F, SURFACE, "publisher note")
            yy += 22
    if status:
        text(d, (x + 40, yy + 2), status, MUTED, F_S, SURFACE, "check status")
        yy += 26
    yy += 12
    bx = button(d, x + 40, yy, primary, True)
    if prev:
        bx = button(d, bx, yy, f"Go back to {prev}")
    button(d, bx, yy, f"Turn off {name}")
    text(d, (x + 40, yy + 44), "Copy diagnostics", ACCENT, F, SURFACE, "link on surface")
    return y + h


def states():
    im, d = m.new(1500, 960)
    m.heading(d, "A withdrawn module version: page card, sidebar and tab, tile, Modules row, tray, notification")
    m.window(d, [30, 70, 900, 520])
    module_list(d, 31, 71, 448, ROWS, "art")
    tabs = [{"key": "keyboard", "name": "Keyboard"}, {"key": "art", "name": "Art", "state": "crash"}]
    m.strip(d, 222, 72, 677, tabs, 1)
    text(d, (242, 124), "Art", TEXT, F_H, BG)
    end = card(d, 242, 160, 640, "Art", "0.5.0",
               "This build can send your screen captures to a test server. Please update to 0.5.1.",
               prev="0.4.2")
    for i in range(3):
        rr(d, [242, end + 16 + i * 34, 640 - i * 70, end + 42 + i * 34], fill=(0x2E, 0x2A, 0x27), r=6)
    text(d, (242, 500), "Last page drawn stays below, greyed. Art is stopped; its leases ended.", MUTED, F_S, BG)

    # Modules row
    mx, my = 940, 70
    rr(d, [mx, my, mx + 530, my + 196], fill=BG, outline=BORDER, r=8)
    text(d, (mx + 16, my + 12), "Modules page", MUTED, F_S, BG)
    rows = [("Art", "Withdrawn \u00b7 0.5.0", "Check for updates", True),
            ("Art (after Turn off)", "Off \u00b7 0.5.0 was withdrawn", "Check for updates", False),
            ("Chat", "On", "Turn off", False)]
    for i, (n, s, b, prim) in enumerate(rows):
        y = my + 40 + i * 50
        rr(d, [mx + 12, y, mx + 518, y + 42], fill=SURFACE, r=6)
        if i == 0:
            d.text((mx + 24, y + 13), G["warn"], fill=DANGER, font=ICON)
        text(d, (mx + 46, y + 10), n, TEXT, F_B, SURFACE)
        text(d, (mx + 210, y + 11), s, MUTED, F, SURFACE)
        bw = d.textlength(b, font=F) + 20
        pm.small_button(d, mx + 506 - bw, y + 7, b, primary=prim)
    text(d, (mx, my + 204), "No Turn on for a withdrawn version: Check for updates takes its place.", MUTED, F_S, CANVAS)

    # tray panel with the withdrawn tile
    tx, ty = 940, 300
    mic = pm.T(pm.MIC, value="Unmuted", sub="Default mic", button="Mute", key="F13")
    pm.tray_panel(d, tx, ty, [mic, pm.T(pm.FOC, value="25:00", sub="Ready", button="Start")])
    x2, y2 = tx + 12 + pm.TW + 12, ty + 58
    rr(d, [x2, y2, x2 + pm.TW, y2 + pm.TH], fill=SURFACE, outline=DANGER, r=8)
    d.text((x2 + 12, y2 + 13), G["warn"], fill=DANGER, font=ICON)
    PAIRS.append(("tile withdrawn mark", DANGER, SURFACE))
    text(d, (x2 + 34, y2 + 8), "Art", MUTED, F_B, SURFACE, "tile title")
    text(d, (x2 + 12, y2 + 38), "Version withdrawn", TEXT, F_B, SURFACE, "tile line")
    text(d, (x2 + 12, y2 + 58), "by its publisher", TEXT, F_B, SURFACE, "tile line 2")
    pm.small_button(d, x2 + 12, y2 + pm.TH - 38, "Open", primary=True)

    # tray menu + icon
    qx, qy = 30, 560
    rr(d, [qx, qy, qx + 330, qy + 220], fill=SURFACE, outline=BORDER, r=8)
    text(d, (qx + 16, qy + 14), "Tray menu (right click)", MUTED, F_S, SURFACE)
    d.text((qx + 16, qy + 46), G["warn"], fill=DANGER, font=ICON)
    PAIRS.append(("tray mark", DANGER, SURFACE))
    text(d, (qx + 40, qy + 42), "Art was withdrawn", TEXT, F_B, SURFACE)
    text(d, (qx + 40, qy + 64), "Open Art to check for updates", MUTED, F_S, SURFACE)
    d.line([(qx + 12, qy + 92), (qx + 318, qy + 92)], fill=BORDER)
    for i, lab in enumerate(["Open Hideout", "Pause Hideout", "Quit"]):
        text(d, (qx + 40, qy + 104 + i * 30), lab, TEXT, F, SURFACE)
    ix, iy = 390, 570
    rr(d, [ix, iy, ix + 150, iy + 60], fill=CHROME, r=6)
    d.text((ix + 20, iy + 18), G["keyboard"], fill=TEXT, font=m.font("segmdl2.ttf", 20))
    d.ellipse([ix + 38, iy + 34, ix + 50, iy + 46], fill=DANGER, outline=CHROME, width=2)
    text(d, (ix + 64, iy + 20), "tray icon", MUTED, F_S, CHROME)
    text(d, (ix, iy + 70), "Red dot until Art is updated,", MUTED, F_S, CANVAS)
    text(d, (ix, iy + 88), "gone back, or turned off.", MUTED, F_S, CANVAS)

    # notification
    nx, ny = 600, 560
    rr(d, [nx, ny, nx + 420, ny + 150], fill=SURFACE, outline=BORDER, r=8)
    text(d, (nx + 16, ny + 12), "Hideout", MUTED, F_S, SURFACE)
    text(d, (nx + 16, ny + 36), "Art 0.5.0 was withdrawn", TEXT, F_B, SURFACE)
    text(d, (nx + 16, ny + 60), "Its publisher withdrew it, so Hideout stopped it.", TEXT, F, SURFACE)
    bx = button(d, nx + 16, ny + 98, "Check for updates")
    button(d, bx, ny + 98, "Open Art")
    text(d, (nx, ny + 160), "Once per withdrawn version, when it happens. No publisher text here.", MUTED, F_S, CANVAS)

    notes = [
        "Sidebar and tab: the red warning mark (stopped family). Accessible name and tooltip: \"Art, withdrawn by its publisher\".",
        "The publisher's note shows only on the card: plain text, one paragraph, at most 160 characters, in quotes under \"Publisher's note:\".",
        "Never shown: the state id (revoked), trust.revoked, catalog URLs, seq numbers, lease ids. They go to Copy diagnostics.",
        "Root cause once: a module that waits on Art reads \"Waiting for Art\" with Go to Art; the tray names only Art.",
    ]
    for i, ln in enumerate(notes):
        text(d, (30, 840 + i * 24), ln, MUTED, F_S, CANVAS)
    return im


LONG = ("URGENT!!!\u202e\n\nThis version has a defect in the capture path that may upload frames to our "
        "staging server.\tWe are very sorry. Please update as soon as possible, and visit "
        "https://example.invalid/help for details, steps and a full write-up of what happened.")


def variants():
    im, d = m.new(1500, 900)
    m.heading(d, "The withdrawn card: no note, a long note capped, Check for updates outcomes, no previous version")
    col = [(30, 70), (770, 70)]
    x, y = col[0]
    text(d, (x, y), "No reason in the catalog", MUTED, F_S, CANVAS)
    e = card(d, x, y + 22, 700, "Art", "0.5.0", "", prev="0.4.2")
    text(d, (x, e + 30), "A long note, cleaned and capped (raw text has a bidi override, newlines, a tab, a URL)", MUTED, F_S, CANVAS)
    e = card(d, x, e + 52, 700, "Art", "0.5.0", LONG, prev="0.4.2")
    text(d, (x, e + 10), "The URL is plain text, never a link. The full text is in Copy diagnostics.", MUTED, F_S, CANVAS)

    x, y = col[1]
    text(d, (x, y), "Check for updates: checking", MUTED, F_S, CANVAS)
    e = card(d, x, y + 22, 700, "Art", "0.5.0", "", prev="0.4.2", status="Checking Art\u2019s source\u2026",
             primary="Checking\u2026")
    text(d, (x, e + 16), "Check for updates: nothing newer yet", MUTED, F_S, CANVAS)
    e = card(d, x, e + 38, 700, "Art", "0.5.0", "", prev="0.4.2",
             status="No new version yet. Checked just now.")
    text(d, (x, e + 16), "No previous version kept (or it was withdrawn too)", MUTED, F_S, CANVAS)
    e = card(d, x, e + 38, 700, "Art", "0.5.0", "")
    notes = [
        "A newer version found: the card gives way to \"Updating\u2026\" (or \"Update waiting for you\" when it asks for a new permission).",
        "Couldn't reach the source: status line \"Couldn't check. Try again later.\" with the same buttons.",
        "The card goes when another version of Art connects (module.<id>.revoked clears). Nothing to dismiss by hand.",
    ]
    for i, ln in enumerate(notes):
        text(d, (30, 820 + i * 22), ln, MUTED, F_S, CANVAS)
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    assert clean_note("") == "" and clean_note(" \u200b\n ") == ""
    assert "\u202e" not in clean_note(LONG) and len(clean_note(LONG)) <= CAP
    for name, fn in [("states", states), ("card", variants)]:
        path = os.path.join(OUT, f"ux-revoked-proposed-{name}.png")
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
