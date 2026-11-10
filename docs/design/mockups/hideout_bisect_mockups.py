"""Mockups for finding the module that breaks Hideout (safe-mode bisect, #1033). Python + Pillow, no app code.

Run: python docs/design/mockups/hideout_bisect_mockups.py [out_dir]
Writes ux-bisect-proposed-*.png (default: C:/Users/fuchs/splitkeeb/worktrees/_review/h1033) and
fails if any drawn label is under 4.5:1. Reuses the supervisor and tab mockups' helpers and
hideout-shell's DARK tokens. Window-free: draws to files only.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402
from hideout_tabs_mockups import (ACCENT, BG, BORDER, CANVAS, CHROME, F, F_B, F_H, F_S,  # noqa: E402
                                  ON_ACCENT, SURFACE, TEXT, MUTED, PAIRS, rr, text)
from hideout_supervisor_mockups import AMBER, AMBER_BG, button, module_list  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\worktrees\_review\h1033"


def bar(d, x, y, w, title, lines, buttons, links=()):
    """Amber round bar / card: title, lines, then buttons (first is primary) and text links."""
    h = 40 + 22 * len(lines) + 46
    rr(d, [x, y, x + w, y + h], fill=AMBER_BG, r=6)
    text(d, (x + 16, y + 10), title, TEXT, F_B, AMBER_BG, "bar title")
    yy = y + 36
    for ln in lines:
        text(d, (x + 16, yy), ln, TEXT, F, AMBER_BG, "bar line")
        yy += 22
    bx = x + 16
    for i, b in enumerate(buttons):
        bx = button(d, bx, yy + 6, b, i == 0, on=AMBER_BG)
    for ln in links:
        text(d, (bx + 4, yy + 11), ln, AMBER, F, AMBER_BG, "bar link")
        bx += d.textlength(ln, font=F) + 22
    return y + h


def rows(d, x, y, w, items, lock=True):
    for i, (n, s) in enumerate(items):
        yy = y + i * 50
        rr(d, [x, yy, x + w, yy + 42], fill=SURFACE, r=6)
        text(d, (x + 16, yy + 11), n, TEXT, F_B, SURFACE, "row name")
        text(d, (x + 150, yy + 12), s, MUTED, F, SURFACE, "row state")
        lab = "Turn on" if s.startswith("Off") else "Turn off"
        bw = d.textlength(lab, font=F) + 24
        rr(d, [x + w - 16 - bw, yy + 6, x + w - 16, yy + 36], outline=BORDER, r=5)
        text(d, (x + w - 4 - bw, yy + 10), lab, MUTED, F, SURFACE, "locked button")
    yy = y + len(items) * 50 + 4
    if lock:
        text(d, (x, yy), "Locked while finding the module. Stop finding to change it.", MUTED, F_S, BG, "lock line")


def label(d, x, y, s):
    text(d, (x, y), s, MUTED, F_S, CANVAS, "caption")


def rounds():
    im, d = m.new(1560, 1000)
    m.heading(d, "Finding the module that breaks Hideout: start, a round, between rounds, tray")

    # 0. the banner link that starts it (#1032)
    label(d, 30, 62, "1. Safe-mode banner (#1032). The link starts round 1 at once.")
    bar(d, 30, 84, 700, "Safe mode", ["Hideout closed 3 times in 5 minutes, so no modules were started."],
        ["Start normally"], ["Find the module that breaks Hideout"])

    # 1. a round in progress
    label(d, 30, 218, "2. Round 2 in progress: the bar sits on top of every Hideout page, the Modules page shows the halves.")
    m.window(d, [30, 240, 900, 860])
    side = [("keyboard", "Keyboard", "off"), ("lights", "Lights", None), ("scenes", "Scenes", "off"),
            ("art", "Art", None), ("chat", "Chat", None), ("images", "Images", "off"),
            ("-", "-", None), ("modules", "Modules", None), ("settings", "Settings", None)]
    module_list(d, 31, 241, 618, side, "modules")
    end = bar(d, 236, 254, 652, "Finding the module that breaks Hideout · Round 2 of 4",
              ["3 of 6 modules are on: Lights, Art, Chat.",
               "Use Hideout as usual. If it closes, it'll pick up here.",
               "This round ends in 4 min if Hideout stays open."],
              ["It's fine", "It closed"], ["Stop finding"])
    text(d, (252, end + 14), "Modules", TEXT, F_H, BG)
    rows(d, 252, end + 52, 620, [("Lights", "On in this round"), ("Art", "On in this round"),
                                 ("Chat", "On in this round"), ("Keyboard", "Off for this round"),
                                 ("Scenes", "Off for this round"), ("Images", "Off for this round"),
                                 ("Screens", "Off · not part of the search")])
    label(d, 30, 870, "Sidebar: started modules as usual; the others muted, tooltip and name \"Keyboard, off for this round\".")
    label(d, 30, 890, "Their pages say \"Off for this round\" with no Turn on. Timer counts only while Hideout is open and the PC is awake.")

    # 2. between rounds
    x = 940
    label(d, x, 62, "3. Hideout closed in round 2. Next start: nothing on, one click for round 3.")
    e = bar(d, x, 84, 590, "Round 2: Hideout closed.", ["That narrows it to Lights, Art or Chat."],
            ["Start round 3"], ["Stop finding"])
    label(d, x, e + 14, "A round that stays open 5 min (or It's fine) goes straight on: \"Round 2 was fine.\"")

    # 3. paused / asked
    label(d, x, e + 50, "4. Hideout was quit (tray Quit): the round is paused, not counted.")
    e = bar(d, x, e + 72, 590, "Round 2 was paused.", ["Hideout was quit during round 2."],
            ["Go on with round 2"], ["Stop finding"])

    # 4. tray
    ty = e + 40
    label(d, x, ty - 22, "5. Tray during the search (amber safe-mode dot stays).")
    rr(d, [x, ty, x + 360, ty + 196], fill=CHROME, outline=BORDER, r=6)
    text(d, (x + 14, ty + 12), "Hideout · round 2 closed, start round 3", MUTED, F_S, CHROME, "tray status")
    d.line([(x + 10, ty + 38), (x + 350, ty + 38)], fill=BORDER)
    for i, it in enumerate(["Open Hideout", "Start round 3", "Stop finding", "Quit"]):
        text(d, (x + 14, ty + 50 + i * 34), it, TEXT, F, CHROME, "tray item")
    ix, iy = x + 400, ty + 20
    rr(d, [ix, iy, ix + 40, iy + 40], fill=SURFACE, r=6)
    text(d, (ix + 12, iy + 9), "H", ACCENT, F_B, SURFACE, "tray icon letter")
    d.ellipse([ix + 28, iy + 28, ix + 40, iy + 40], fill=AMBER)
    label(d, ix - 6, iy + 50, "amber dot")
    label(d, x, ty + 210, "During a round: \"Hideout · finding the module (round 2 of 4)\".")
    label(d, x, ty + 230, "No It's fine in the tray. No notification per round.")
    return im


def results():
    im, d = m.new(1560, 860)
    m.heading(d, "Finding the module: results, one module, and when Hideout can't tell")
    x1, x2, w = 30, 800, 730
    label(d, x1, 62, "1. Found it. Other modules start on the choice. Focus on Turn off Art.")
    e = bar(d, x1, 84, w, "Art seems to make Hideout close.",
            ["Hideout closed in rounds 1 and 3 with Art on, and stayed open without it.",
             "Art 0.4.2 worked before."],
            ["Turn off Art", "Copy diagnostics"], ["Start normally anyway", "Go back to Art 0.4.2"])
    label(d, x1, e + 10, "\"Art 0.4.2 worked before.\" and Go back show only when an older version is kept.")

    label(d, x1, e + 50, "2. Couldn't find one module.")
    e = bar(d, x1, e + 72, w, "Hideout stayed open in every round.",
            ["It may need two modules on together, or the problem has passed."],
            ["Start normally", "Copy diagnostics"], ["Try again"])

    label(d, x1, e + 30, "3. Closed while nothing was on (e.g. between rounds).")
    e = bar(d, x1, e + 52, w, "Hideout closed with no modules on.",
            ["The problem is in Hideout itself, not a module."],
            ["Copy diagnostics"], ["Start normally anyway"])

    label(d, x2, 62, "4. One module in the search: the link and a single round.")
    e = bar(d, x2, 84, w, "Safe mode", ["Hideout closed 3 times in 5 minutes, so no modules were started."],
            ["Start normally"], ["Check whether Art breaks Hideout"])
    e = bar(d, x2, e + 14, w, "Finding the module that breaks Hideout · Round 1 of 1",
            ["Only Art is on.", "Use Hideout as usual. If it closes, it'll pick up here.",
             "This round ends in 5 min if Hideout stays open."],
            ["It's fine", "It closed"], ["Stop finding"])

    label(d, x2, e + 30, "5. Exit Hideout can't read (Windows restarted, no Quit): ask once.")
    e = bar(d, x2, e + 52, w, "Hideout closed during round 2. Did it close by itself?", [],
            ["It closed", "I closed it"], ["Stop finding"])

    label(d, x2, e + 30, "6. No modules in the search: no link.")
    e = bar(d, x2, e + 52, w, "Safe mode", ["No modules are on, so the problem is in Hideout itself."],
            ["Copy diagnostics"], ["Start normally"])

    notes = ["Stop finding (any state) returns to the safe-mode banner: \"Stopped finding. Nothing was changed.\"",
             "The saved on/off list changes only on Turn off <Module> or Go back. Enter = primary; Esc does nothing.",
             "One notification when the result is ready and the window is closed: \"Found the module\" / Open Hideout."]
    for i, ln in enumerate(notes):
        label(d, 30, 780 + i * 22, ln)
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("rounds", rounds), ("results", results)]:
        path = os.path.join(OUT, f"ux-bisect-proposed-{name}.png")
        fn().save(path)
        print(path)
    worst = {}
    for lab, fg, bg in PAIRS:
        worst[lab] = min(m.contrast(fg, bg), worst.get(lab, 99))
    print(f"min over {len(worst)} pairs: {min(worst.values()):.2f}:1 ({min(worst, key=worst.get)})")
    bad = {k: round(v, 2) for k, v in worst.items() if v < 4.5}
    if bad:
        print("UNDER 4.5:1:", bad)
        sys.exit(1)


if __name__ == "__main__":
    main()
