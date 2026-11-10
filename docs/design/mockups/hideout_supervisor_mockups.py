"""Mockups for the Hideout supervisor's states (#966). Python + Pillow, no app code.

Run: python docs/design/mockups/hideout_supervisor_mockups.py [out_dir]
Writes ux-supervisor-proposed-*.png (default: C:/Users/fuchs/splitkeeb/review/hideout-supervisor)
and fails if any drawn label is under 4.5:1. Reuses the tab mockups' helpers and hideout-shell's
DARK tokens (hideout_tabs_mockups.py).
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402
from hideout_tabs_mockups import (ACCENT, BG, BORDER, CANVAS, CHROME, DANGER, F, F_B, F_H, F_S,  # noqa: E402
                                  G, ICON, ICON_S, MUTED, ON_ACCENT, SURFACE, TEXT, PAIRS, rr, text)

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\review\hideout-supervisor"
AMBER_BG = (0x3A, 0x2A, 0x14)
AMBER = (0xFF, 0xC0, 0x6B)


def button(d, x, y, label, primary=False, on=SURFACE):
    bw = d.textlength(label, font=F) + 24
    if primary:
        rr(d, [x, y, x + bw, y + 32], fill=ACCENT, r=5)
        text(d, (x + 12, y + 5), label, ON_ACCENT, F, ACCENT, "primary button")
    else:
        rr(d, [x, y, x + bw, y + 32], outline=BORDER, r=5)
        text(d, (x + 12, y + 5), label, TEXT, F, on, f"button {label}")
    return x + bw + 10


def module_list(d, x0, y0, h, rows, selected):
    """Sidebar: rows of (key, name, mark) with mark None|'ring'|'stop'|'off'|'safe'."""
    d.rectangle([x0, y0, x0 + 190, y0 + h], fill=CHROME)
    text(d, (x0 + 14, y0 + 12), "Hideout", TEXT, F_B, CHROME)
    y = y0 + 46
    for key, name, mark in rows:
        if key == "-":
            d.line([(x0 + 14, y + 6), (x0 + 176, y + 6)], fill=BORDER)
            y += 14
            continue
        sel = key == selected
        fill = ACCENT if sel else CHROME
        fg = ON_ACCENT if sel else (MUTED if mark == "off" else TEXT)
        if sel:
            rr(d, [x0 + 8, y, x0 + 182, y + 30], fill=ACCENT, r=5)
        ix = x0 + 16
        if mark == "stop":
            d.text((ix, y + 8), G["warn"], fill=ON_ACCENT if sel else DANGER, font=ICON)
            if not sel:
                PAIRS.append(("sidebar stop mark", DANGER, CHROME))
        elif mark == "ring":
            d.arc([ix - 2, y + 6, ix + 16, y + 24], 200, 470, fill=ON_ACCENT if sel else ACCENT, width=2)
            d.text((ix + 3, y + 10), G[key], fill=fg, font=ICON_S)
        else:
            d.text((ix, y + 8), G[key], fill=fg, font=ICON)
        text(d, (x0 + 40, y + 5), name, fg, F, fill, f"sidebar {name}{' (selected)' if sel else ''}")
        y += 34
    d.line([(x0 + 190, y0), (x0 + 190, y0 + h)], fill=BORDER)


ROWS = [("keyboard", "Keyboard", None), ("lights", "Lights", "ring"), ("scenes", "Scenes", None),
        ("art", "Art", "stop"), ("chat", "Chat", "ring"), ("images", "Images", "off"),
        ("-", "-", None), ("modules", "Modules", None), ("settings", "Settings", None)]


def states():
    im, d = m.new(1500, 900)
    m.heading(d, "Supervisor states: sidebar marks, the crash bar, the stopped card, tray and notification")
    # window 1: Chat restarting (crash bar)
    m.window(d, [30, 70, 760, 430])
    module_list(d, 31, 71, 358, ROWS, "chat")
    tabs = [{"key": "keyboard", "name": "Keyboard"}, {"key": "chat", "name": "Chat", "state": "updating"},
            {"key": "art", "name": "Art", "state": "crash"}]
    m.strip(d, 222, 72, 537, tabs, 1)
    rr(d, [232, 118, 748, 186], fill=AMBER_BG, r=6)
    d.text((246, 134), G["sync"], fill=AMBER, font=ICON)
    PAIRS.append(("crash bar icon", AMBER, AMBER_BG))
    text(d, (272, 124), "Chat stopped unexpectedly. Restarting in 4 s (2 of 5).", TEXT, F_B, AMBER_BG, "crash bar title")
    text(d, (272, 150), "Your place on this page is kept.", TEXT, F, AMBER_BG, "crash bar line")
    text(d, (668, 150), "Open log", AMBER, F, AMBER_BG, "crash bar link")
    text(d, (242, 204), "Chat", TEXT, F_H, BG)
    for i in range(4):
        rr(d, [242, 244 + i * 40, 600 - i * 60, 274 + i * 40], fill=(0x2E, 0x2A, 0x27), r=6)
    text(d, (242, 404), "Last page drawn, greyed until Chat is back.", MUTED, F_S, BG)

    # window 2: Art crash-stopped card
    m.window(d, [790, 70, 1470, 430])
    module_list(d, 791, 71, 358, ROWS, "art")
    m.strip(d, 982, 72, 487, tabs, 2)
    text(d, (1002, 124), "Art", TEXT, F_H, BG)
    rr(d, [1002, 162, 1456, 318], fill=SURFACE, outline=DANGER, r=6)
    d.text((1018, 178), G["warn"], fill=DANGER, font=ICON)
    PAIRS.append(("card stop mark", DANGER, SURFACE))
    text(d, (1042, 174), "Stopped after 5 crashes", TEXT, F_B, SURFACE)
    text(d, (1042, 198), "Art crashed 5 times in 10 minutes.", MUTED, F, SURFACE)
    bx = button(d, 1042, 232, "Start again", True)
    button(d, bx, 232, "Start with default settings")
    button(d, 1042, 274, "Go back to 0.4.2")
    text(d, (1002, 330), "Copy diagnostics", ACCENT, F, BG, "link")
    text(d, (1002, 360), "Second stop in a row: Start with default settings", MUTED, F_S, BG)
    text(d, (1002, 378), "becomes the orange one (the ladder below).", MUTED, F_S, BG)

    # tray menu
    tx, ty = 30, 470
    rr(d, [tx, ty, tx + 330, ty + 230], fill=SURFACE, outline=BORDER, r=8)
    text(d, (tx + 16, ty + 14), "Tray menu (right click)", MUTED, F_S, SURFACE)
    d.text((tx + 16, ty + 46), G["warn"], fill=DANGER, font=ICON)
    PAIRS.append(("tray stop mark", DANGER, SURFACE))
    text(d, (tx + 40, ty + 42), "Art stopped", TEXT, F_B, SURFACE)
    text(d, (tx + 40, ty + 64), "Open Art to start it again", MUTED, F_S, SURFACE)
    d.line([(tx + 12, ty + 92), (tx + 318, ty + 92)], fill=BORDER)
    for i, lab in enumerate(["Open Hideout", "Pause Hideout", "Quit"]):
        text(d, (tx + 40, ty + 104 + i * 30), lab, TEXT, F, SURFACE)
    text(d, (tx + 16, ty + 200), "Only the worst state, root cause only.", MUTED, F_S, SURFACE)

    # tray icon with mark
    ix, iy = 390, 480
    rr(d, [ix, iy, ix + 150, iy + 60], fill=CHROME, r=6)
    d.text((ix + 20, iy + 18), G["keyboard"], fill=TEXT, font=m.font("segmdl2.ttf", 20))
    d.ellipse([ix + 38, iy + 34, ix + 50, iy + 46], fill=DANGER, outline=CHROME, width=2)
    text(d, (ix + 64, iy + 20), "tray icon", MUTED, F_S, CHROME)
    text(d, (ix, iy + 70), "Red dot: something is stopped.", MUTED, F_S, CANVAS)
    text(d, (ix, iy + 88), "No dot while restarting.", MUTED, F_S, CANVAS)

    # notification
    nx, ny = 600, 470
    rr(d, [nx, ny, nx + 380, ny + 150], fill=SURFACE, outline=BORDER, r=8)
    text(d, (nx + 16, ny + 12), "Hideout", MUTED, F_S, SURFACE)
    text(d, (nx + 16, ny + 36), "Art stopped", TEXT, F_B, SURFACE)
    text(d, (nx + 16, ny + 60), "It crashed 5 times in 10 minutes.", TEXT, F, SURFACE)
    button(d, nx + 16, ny + 98, "Start again")
    button(d, nx + 140, ny + 98, "Open Art")
    text(d, (nx, ny + 160), "Once per module per sign-in. Never for restarting.", MUTED, F_S, CANVAS)

    # rollback bar
    rx, ry = 1010, 470
    rr(d, [rx, ry, rx + 460, ry + 110], fill=SURFACE, outline=BORDER, r=8)
    d.text((rx + 16, ry + 18), "\ue7a7", fill=ACCENT, font=ICON)
    PAIRS.append(("rollback icon", ACCENT, SURFACE))
    text(d, (rx + 40, ry + 14), "Went back to Art 0.4.2", TEXT, F_B, SURFACE)
    text(d, (rx + 40, ry + 38), "0.5.0 crashed 5 times after the update.", TEXT, F, SURFACE)
    button(d, rx + 40, ry + 66, "OK")
    text(d, (rx + 110, ry + 71), "Open log", ACCENT, F, SURFACE, "link on surface")
    text(d, (rx, ry + 120), "Automatic rollback (§7.8): on the page and as the notification.", MUTED, F_S, CANVAS)

    notes = [
        "Sidebar: orange ring = starting, updating or restarting; red mark = stopped, needs a newer Hideout, couldn't install;",
        "grey label = off. The tab strip uses the same marks (hideout-tabs-ux.md). Running shows nothing.",
        "Crash bar: amber, at the top of the module's page and its pop-out, only while restarting. One link: Open log.",
        "Never shown: state ids, exit codes, error ids (rpc.frame-too-large), pids, hashes. They go to Copy diagnostics and the log.",
    ]
    for i, ln in enumerate(notes):
        text(d, (30, 780 + i * 24), ln, MUTED, F_S, CANVAS)
    return im


def safe_mode():
    im, d = m.new(1500, 780)
    m.heading(d, "Safe mode and finding the module that breaks Hideout (§7.10)")
    m.window(d, [30, 70, 900, 560])
    rows = [("modules", "Modules", None), ("-", "-", None), ("settings", "Settings", None)]
    module_list(d, 31, 71, 488, rows, "modules")
    rr(d, [232, 84, 888, 176], fill=AMBER_BG, r=6)
    text(d, (248, 94), "Safe mode", TEXT, F_B, AMBER_BG, "safe banner title")
    text(d, (248, 118), "Hideout closed 3 times in 5 minutes, so no modules were started.", TEXT, F, AMBER_BG, "safe banner line")
    bx = 248
    rr(d, [bx, 140, bx + 130, 170], fill=ACCENT, r=5)
    text(d, (bx + 12, 144), "Start normally", ON_ACCENT, F, ACCENT, "primary button")
    text(d, (bx + 146, 144), "Find the module that breaks Hideout", AMBER, F, AMBER_BG, "safe banner link")
    text(d, (248, 196), "Modules", TEXT, F_H, BG)
    mods = [("Lights", "On · not started in safe mode"), ("Art", "On · not started in safe mode"),
            ("Chat", "On · not started in safe mode"), ("Images", "Off")]
    for i, (n, s) in enumerate(mods):
        y = 236 + i * 56
        rr(d, [248, y, 872, y + 46], fill=SURFACE, r=6)
        text(d, (264, y + 12), n, TEXT, F_B, SURFACE)
        text(d, (380, y + 13), s, MUTED, F, SURFACE)
        lab = "Turn off" if s != "Off" else "Turn on"
        bw = d.textlength(lab, font=F) + 24
        rr(d, [856 - bw, y + 7, 856, y + 39], outline=BORDER, r=5)
        text(d, (868 - bw, y + 11), lab, TEXT, F, SURFACE)
    text(d, (248, 470), "Turning a module off here is saved; safe mode itself is only this start.", MUTED, F_S, BG)

    # bisect steps
    sx = 940
    steps = [
        ("Step 1 of about 3", "Half your modules are off for now. Use Hideout as before.",
         "Is the problem still there?", ["Still broken", "Works now"]),
        ("Found it", "Art seems to break Hideout.", "Your other modules are back on.",
         ["Turn off Art", "Go back to Art 0.4.2"]),
    ]
    y = 70
    for title, l1, l2, btns in steps:
        rr(d, [sx, y, sx + 530, y + 170], fill=SURFACE, outline=BORDER, r=8)
        text(d, (sx + 20, y + 16), title, TEXT, F_B, SURFACE)
        text(d, (sx + 20, y + 46), l1, TEXT, F, SURFACE)
        text(d, (sx + 20, y + 70), l2, TEXT, F, SURFACE)
        bx = sx + 20
        for i, b in enumerate(btns):
            bx = button(d, bx, y + 116, b, i == 0)
        text(d, (sx + 420, y + 122), "Stop", ACCENT, F, SURFACE, "link on surface")
        y += 200
    text(d, (sx, y), "A small window, always on top, until the answer. Stop at any step puts every module back.", MUTED, F_S, CANVAS)
    text(d, (sx, y + 20), "Bisect never edits the saved on/off list; only the last choice does.", MUTED, F_S, CANVAS)

    notes = [
        "Tray in safe mode: icon gets the amber dot, tooltip and status line \"Hideout · safe mode\", menu item Start normally.",
        "One notification at start: \"Hideout started in safe mode\" / \"It closed 3 times in 5 minutes.\" / Open Hideout.",
        "A hub restart that takes its modules back (§7.9) shows nothing: no crash bar, no count. Modules not back in 30 s show Starting…",
    ]
    for i, ln in enumerate(notes):
        text(d, (30, 640 + i * 24), ln, MUTED, F_S, CANVAS)
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("states", states), ("safe-mode", safe_mode)]:
        path = os.path.join(OUT, f"ux-supervisor-proposed-{name}.png")
        fn().save(path)
        print(path)
    worst = {}
    for label, fg, bg in PAIRS:
        r = m.contrast(fg, bg)
        worst[label] = min(r, worst.get(label, 99))
    print(f"min over {len(worst)} pairs: {min(worst.values()):.2f}:1 ({min(worst, key=worst.get)})")
    bad = {k: round(v, 2) for k, v in worst.items() if v < 4.5}
    if bad:
        print("UNDER 4.5:1:", bad)
        sys.exit(1)


if __name__ == "__main__":
    main()
