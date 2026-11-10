"""Mockups for Hideout pop-out windows (H2.11, #998). Python + Pillow, no app code, no window.

Run: python docs/design/mockups/hideout_popouts_mockups.py [out_dir]
Writes ux-h211-proposed-*.png (default: C:/Users/fuchs/splitkeeb/worktrees/_review/h211) and fails
if any drawn label is under 4.5:1. Reuses the tab mockups' helpers and hideout-shell's DARK tokens
(hideout_tabs_mockups.py) and the supervisor mockups' button and amber bar colours.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402
from hideout_tabs_mockups import (ACCENT, BG, BORDER, CANVAS, CHROME, DANGER, F, F_B, F_H, F_S,  # noqa: E402
                                  G, ICON, ICON_S, MUTED, ON_ACCENT, SURFACE, TEXT, PAIRS, rr, text)
from hideout_supervisor_mockups import AMBER, AMBER_BG, button  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\worktrees\_review\h211"
G.update({"pin": "\ue718", "pinned": "\ue840", "more": "\ue712", "min": "\ue921", "max": "\ue922",
          "x": "\ue8bb", "app": "\ue765"})


def sidebar_out(d, x0, y0, h, selected, out=()):
    """The tabs doc's sidebar, plus a muted pop-out mark on entries that are in their own window."""
    m.sidebar(d, x0, y0, h, m.SIDEBAR, selected)
    y = y0 + 46
    for key, name in m.SIDEBAR:
        if name == "-":
            y += 14
            continue
        if key in out:
            d.text((x0 + 148, y + 10), G["popout"], fill=MUTED, font=ICON_S)
            PAIRS.append(("sidebar pop-out mark", MUTED, CHROME))
        y += 34


def popout(d, x, y, w, h, key, title, pinned=False, focused=True, hover_dock=False):
    """A pop-out window: one title row (page icon, title, Keep on top, Dock back, more, caption
    buttons), then the page. Returns the page area's top-left."""
    rr(d, [x, y, x + w, y + h], fill=BG, outline=ACCENT if focused else BORDER, r=8)
    d.rectangle([x + 1, y + 1, x + w - 1, y + 36], fill=CHROME)
    d.text((x + 14, y + 12), G[key], fill=TEXT, font=ICON)
    text(d, (x + 38, y + 8), f"Hideout · {title}", TEXT if focused else MUTED, F, CHROME, "pop-out title")
    cx = x + w - 3 * 40
    for i, g in enumerate(["min", "max", "x"]):
        d.text((cx + 14 + i * 40, y + 13), G[g], fill=TEXT, font=ICON_S)
    bx = cx - 34
    d.text((bx + 8, y + 12), G["more"], fill=TEXT, font=ICON)
    lab = "Dock back"
    bw = d.textlength(lab, font=F_S) + 32
    bx -= bw + 8
    rr(d, [bx, y + 7, bx + bw, y + 29], fill=SURFACE if not hover_dock else BORDER,
       outline=ACCENT if hover_dock else BORDER, r=4)
    d.text((bx + 8, y + 13), G["dock"], fill=TEXT, font=ICON_S)
    text(d, (bx + 24, y + 9), lab, TEXT, F_S, SURFACE if not hover_dock else BORDER, "Dock back button")
    px = bx - 34
    if pinned:
        rr(d, [px, y + 7, px + 26, y + 29], fill=ACCENT, r=4)
        d.text((px + 7, y + 12), G["pinned"], fill=ON_ACCENT, font=ICON_S)
        PAIRS.append(("Keep on top on", ON_ACCENT, ACCENT))
    else:
        d.text((px + 7, y + 12), G["pin"], fill=TEXT, font=ICON_S)
        PAIRS.append(("Keep on top off", TEXT, CHROME))
    d.line([(x + 1, y + 37), (x + w - 1, y + 37)], fill=BORDER)
    return x + 1, y + 38


def chat_body(d, x, y, w, lines=3):
    msgs = ["How do I make the left half glow when Caps Lock is on?",
            "Open Lights, pick a key and set it to follow Caps Lock.",
            "Thanks. And for Num Lock?"]
    for i, s in enumerate(msgs[:lines]):
        mine = i % 2 == 0
        while d.textlength(s, font=F) + 28 > w - 40 and len(s) > 4:
            s = s[:-2].rstrip() + "…"
        tw = d.textlength(s, font=F) + 28
        bx = x + w - tw - 16 if mine else x + 16
        rr(d, [bx, y + i * 46, bx + tw, y + i * 46 + 34], fill=SURFACE, r=8)
        text(d, (bx + 14, y + i * 46 + 7), s, TEXT, F, SURFACE, "chat bubble")


def menu(d, x, y, items, w=250, hi=None):
    n = sum(1 for i in items if i != "-")
    s = sum(1 for i in items if i == "-")
    rr(d, [x, y, x + w, y + n * 28 + s * 10 + 14], fill=SURFACE, outline=BORDER, r=6)
    yy = y + 7
    for i, it in enumerate(items):
        if it == "-":
            d.line([(x + 8, yy + 4), (x + w - 8, yy + 4)], fill=BORDER)
            yy += 10
            continue
        lab, key = it if isinstance(it, tuple) else (it, "")
        sel = lab == hi
        bg = ACCENT if sel else SURFACE
        if sel:
            rr(d, [x + 4, yy, x + w - 4, yy + 26], fill=ACCENT, r=4)
        text(d, (x + 14, yy + 3), lab, ON_ACCENT if sel else TEXT, F, bg, f"menu {lab}")
        if key:
            text(d, (x + w - 14 - d.textlength(key, font=F_S), yy + 5), key, ON_ACCENT if sel else MUTED, F_S, bg, "menu key")
        yy += 28


def notes(d, x, y, lines):
    for i, ln in enumerate(lines):
        text(d, (x, y + i * 22), ln, MUTED, F_S, CANVAS)


TABS = [{"key": "keyboard", "name": "Keyboard", "w": 112}, {"key": "lights", "name": "Lights", "w": 100},
        {"key": "chat", "name": "Chat", "state": "popped", "w": 96}, {"key": "images", "name": "Images", "state": "popped", "w": 110},
        {"key": "scenes", "name": "Scenes", "w": 96}]


def strip_popout_button(d, x0, y0, w):
    """The strip's right end: the Pop out button for the active tab."""
    bx = x0 + w - 36
    d.line([(bx, y0 + 8), (bx, y0 + m.TAB_H - 8)], fill=BORDER)
    d.text((bx + 12, y0 + 12), G["popout"], fill=TEXT, font=ICON_S)
    PAIRS.append(("strip pop-out button", TEXT, CHROME))


# 1. A page in its own window: hub, two pop-outs, the taskbar group -------------------------------
def windows():
    im, d = m.new(1560, 900)
    m.heading(d, "Pop-out windows: the page in its own window, the tab kept as a placeholder, one taskbar group")
    m.window(d, [30, 70, 760, 470])
    sidebar_out(d, 31, 71, 398, "lights", out=("chat", "images"))
    m.strip(d, 202, 72, 557, TABS, 1)
    strip_popout_button(d, 202, 72, 557)
    m.page_body(d, 202, 110, 557, 360, "Lights", ["Left half · Right half · Desk strip"])
    m.keyboard_canvas(d, 222, 200)
    # tooltip on the Chat placeholder
    rr(d, [400, 112, 640, 140], fill=SURFACE, outline=BORDER, r=4)
    text(d, (410, 116), "Chat · In its own window", TEXT, F_S, SURFACE, "tooltip")
    # Chat pop-out, focused, keep on top on
    px, py = popout(d, 800, 70, 720, 300, "chat", "Chat", pinned=True)
    chat_body(d, px, py + 20, 718)
    # more menu
    menu(d, 1268, 108, [("Dock back", "Ctrl+Shift+O"), ("Keep on top", "on"), "-",
                        ("Show Hideout", ""), ("Close page", "")], w=240, hi=None)
    # Images pop-out, not focused
    ix, iy = popout(d, 800, 400, 460, 230, "images", "Images", focused=False)
    for i in range(4):
        c = (0x40 + i * 24, 0x30 + i * 6, 0x28)
        rr(d, [ix + 18 + i * 108, iy + 22, ix + 112 + i * 108, iy + 116], fill=c, r=6)
    text(d, (ix + 18, iy + 132), "Four images", MUTED, F_S, BG)
    # taskbar group
    tx, ty = 30, 520
    d.rectangle([tx, ty + 210, tx + 740, ty + 258], fill=CHROME)
    rr(d, [tx + 300, ty + 214, tx + 344, ty + 254], fill=SURFACE, r=6)
    d.text((tx + 314, ty + 226), G["app"], fill=ACCENT, font=ICON)
    PAIRS.append(("taskbar icon", ACCENT, SURFACE))
    d.line([(tx + 312, ty + 252), (tx + 332, ty + 252)], fill=ACCENT, width=3)
    for i, (k, t) in enumerate([("keyboard", "Hideout"), ("chat", "Hideout · Chat"), ("images", "Hideout · Images")]):
        bx = tx + 40 + i * 230
        rr(d, [bx, ty, bx + 210, ty + 190], fill=SURFACE, outline=BORDER, r=6)
        d.text((bx + 12, ty + 12), G["app"], fill=ACCENT, font=ICON_S)
        text(d, (bx + 30, ty + 8), t, TEXT, F_S, SURFACE, "thumbnail title")
        rr(d, [bx + 12, ty + 36, bx + 198, ty + 176], fill=BG, r=4)
        d.text((bx + 96, ty + 96), G[k], fill=MUTED, font=ICON)
    m.callout(d, [800, 660, 1520, 870], "Window rules (contract §2.5)", [
        "Title \"Hideout · <page>\"; Hideout's icon, so all windows group under one button.",
        "The page icon shows in the window's own title row only.",
        "Pin = Keep on top, off by default, remembered per page.",
        "Dock back (or Ctrl+Shift+O) puts it back as the active tab and shows Hideout.",
        "Close (×) puts it back as a tab quietly: nothing is lost, nothing asks.",
        "Close page (in the More menu) closes the tab too; it asks only with unsaved work.",
    ])
    notes(d, 30, 790, ["Placeholder tabs keep their place, dimmed with ↗; clicking one brings its window to the front.",
                       "The sidebar entry of a popped-out page carries the same muted ↗.",
                       "The strip's right end has Pop out (↗) for the active tab."])
    return im


# 2. Ways out and back: menu, drag out, drag back, keyboard ---------------------------------------
def gestures():
    im, d = m.new(1560, 960)
    m.heading(d, "Popping out and docking back: right-click, drag, keyboard, and the drop bar on the strip")
    # A: right-click menu
    m.window(d, [30, 70, 600, 450])
    sidebar_out(d, 31, 71, 378, "chat")
    tabs = [{"key": "keyboard", "name": "Keyboard", "w": 106}, {"key": "lights", "name": "Lights", "w": 96},
            {"key": "chat", "name": "Chat", "hover": True, "w": 96}, {"key": "scenes", "name": "Scenes", "w": 96}]
    m.strip(d, 202, 72, 397, tabs, 2)
    menu(d, 330, 112, ["Close", "Close others", "Close to the right", "-", ("Pop out", "Ctrl+Shift+O"),
                       "Move left", "Move right", "Reopen closed tab"], w=250, hi="Pop out")
    text(d, (30, 462), "A  Right-click a tab > Pop out, or Ctrl+Shift+O on the active tab.", TEXT, F, CANVAS)
    text(d, (30, 484), "    Opens next to Hideout on the same screen, at the page's size. Focus moves in.", MUTED, F_S, CANVAS)
    # B: drag out
    m.window(d, [640, 70, 1210, 450])
    sidebar_out(d, 641, 71, 378, "lights")
    tabs = [{"key": "keyboard", "name": "Keyboard"}, {"key": "lights", "name": "Lights"},
            {"key": "scenes", "name": "Scenes"}]
    m.strip(d, 812, 72, 397, tabs, 1)
    m.page_body(d, 812, 110, 397, 340, "Lights")
    # ghost window following the cursor, outside the hub
    gx, gy = 1150, 250
    rr(d, [gx, gy, gx + 340, gy + 150], fill=SURFACE, outline=ACCENT, r=8, w=2)
    d.text((gx + 14, gy + 14), G["chat"], fill=TEXT, font=ICON)
    text(d, (gx + 38, gy + 10), "Chat", TEXT, F_B, SURFACE, "drag ghost title")
    text(d, (gx + 14, gy + 50), "Let go to open in its own window", TEXT, F, SURFACE, "drag ghost hint")
    text(d, (gx + 14, gy + 76), "Esc cancels", MUTED, F_S, SURFACE, "drag ghost esc")
    d.polygon([(gx + 8, gy + 8), (gx + 8, gy + 28), (gx + 14, gy + 22), (gx + 22, gy + 30)], fill=TEXT)
    text(d, (640, 462), "B  Drag a tab off the strip and out of the window.", TEXT, F, CANVAS)
    text(d, (640, 484), "    Its window opens where you let go. Dropping back on the strip only reorders.", MUTED, F_S, CANVAS)
    # C: drag back onto the strip
    m.window(d, [30, 530, 700, 910])
    sidebar_out(d, 31, 531, 378, "lights", out=("chat",))
    tabs = [{"key": "keyboard", "name": "Keyboard"}, {"key": "lights", "name": "Lights"},
            {"key": "chat", "name": "Chat", "state": "popped"}, {"key": "scenes", "name": "Scenes"}]
    m.strip(d, 202, 532, 497, tabs, 1)
    # insertion bar between Keyboard and Lights
    kw = 202 + int(d.textlength("Keyboard", font=F) + 68)
    d.rectangle([kw - 1, 528, kw + 1, 572], fill=ACCENT)
    m.page_body(d, 202, 570, 497, 340, "Lights")
    px, py = popout(d, 330, 650, 420, 170, "chat", "Chat", hover_dock=False)
    chat_body(d, px, py + 16, 418, lines=2)
    d.polygon([(380, 542), (380, 562), (386, 556), (394, 564)], fill=TEXT)
    d.line([(380, 562), (380, 650)], fill=ACCENT, width=1)
    text(d, (30, 920), "C  Drag the window by its title row onto the strip; the orange bar shows where it lands.", TEXT, F, CANVAS)
    text(d, (30, 940), "    Let go: the window closes and the tab moves there. Or press Dock back, or Ctrl+Shift+O.", MUTED, F_S, CANVAS)
    # D: keyboard summary
    m.callout(d, [760, 530, 1530, 840], "Keyboard and screen reader", [
        "Ctrl+Shift+O  pop out the active tab; in a pop-out, dock it back",
        "Ctrl+W in a pop-out  close the window (back to its tab, quietly)",
        "Ctrl+K anywhere  pick a page; a popped-out page's window comes forward",
        "Alt+Space  window menu: Keep on top, Dock back, plus Windows' own items",
        "F6  move between the title row and the page",
        "Announced: \"Chat opened in its own window\" / \"Chat docked, tab 3 of 4\"",
        "Placeholder tab name: \"Chat, in its own window\"",
        "Window name: \"Hideout · Chat\", Keep on top is a toggle button with its state",
    ])
    return im


# 3. States: module states in a pop-out, hub in the tray, placement per monitor -------------------
def states():
    im, d = m.new(1560, 960)
    m.heading(d, "A pop-out follows its module's state, stays up with Hideout in the tray, and remembers each screen")
    # updating
    px, py = popout(d, 30, 70, 480, 220, "chat", "Chat")
    text(d, (px + 18, py + 12), "Updating… Your place on this page is kept.", MUTED, F_S, BG, "updating line")
    chat_body(d, px, py + 44, 478, lines=2)
    # restarting crash bar
    px, py = popout(d, 540, 70, 480, 220, "chat", "Chat", focused=False)
    rr(d, [px + 10, py + 10, px + 468, py + 76], fill=AMBER_BG, r=6)
    d.text((px + 22, py + 26), G["sync"], fill=AMBER, font=ICON)
    PAIRS.append(("crash bar icon", AMBER, AMBER_BG))
    text(d, (px + 46, py + 16), "Chat stopped unexpectedly. Restarting in 4 s (2 of 5).", TEXT, F_S, AMBER_BG, "crash bar title")
    text(d, (px + 46, py + 42), "Your place on this page is kept. · ", TEXT, F_S, AMBER_BG, "crash bar line")
    text(d, (px + 46 + d.textlength("Your place on this page is kept. · ", font=F_S), py + 42), "Open log", AMBER, F_S, AMBER_BG, "crash bar link")
    # stopped card
    px, py = popout(d, 1050, 70, 480, 220, "art", "Art", focused=False)
    rr(d, [px + 10, py + 10, px + 468, py + 150], fill=SURFACE, outline=DANGER, r=6)
    d.text((px + 24, py + 26), G["warn"], fill=DANGER, font=ICON)
    PAIRS.append(("stopped mark", DANGER, SURFACE))
    text(d, (px + 48, py + 22), "Stopped after 5 crashes", TEXT, F_B, SURFACE, "stopped title")
    text(d, (px + 48, py + 46), "Art crashed 5 times in 10 minutes.", MUTED, F, SURFACE, "stopped line")
    bx = px + 48
    for i, b in enumerate(["Start again", "Go back to 0.4.2"]):
        bx = button(d, bx, py + 90, b, i == 0)
    notes(d, 30, 300, ["Updating: one header line, under a second for a planned swap. View state (scroll, open sections, typed text) is kept.",
                       "Restarting: the same crash bar as the page. Stopped: the same card, the last page greyed below. Off: \"Off\" + Turn on.",
                       "Removed: the window closes with its tab; Hideout's status strip says \"Art was removed; its window was closed.\""])
    # hub in tray, pop-out stays
    d.rectangle([30, 390, 760, 700], fill=(0x0E, 0x0C, 0x0B))
    text(d, (44, 400), "Desktop: Hideout's window is in the tray", MUTED, F_S, (0x0E, 0x0C, 0x0B), "desktop label")
    px, py = popout(d, 60, 430, 420, 190, "chat", "Chat", pinned=True)
    chat_body(d, px, py + 16, 418, lines=2)
    d.rectangle([30, 650, 760, 700], fill=CHROME)
    rr(d, [330, 654, 374, 696], fill=SURFACE, r=6)
    d.text((344, 666), G["app"], fill=ACCENT, font=ICON)
    d.line([(342, 694), (362, 694)], fill=ACCENT, width=3)
    rr(d, [690, 660, 724, 690], fill=SURFACE, r=4)
    d.text((699, 667), G["app"], fill=ACCENT, font=ICON)
    notes(d, 30, 712, ["Closing Hideout's window sends it to the tray; pop-outs stay open and live.",
                       "The taskbar button then stands for the pop-outs only. The tray icon opens Hideout itself.",
                       "Quit Hideout (tray menu) closes every window, asking once if any page has unsaved work."])
    # placement per monitor
    ox, oy = 800, 390
    text(d, (ox, oy), "Placement is remembered per page and per screen layout", TEXT, F_B, CANVAS, "placement title")
    for i, (lab, w, h) in enumerate([("Left screen · 100%", 300, 180), ("Right screen · 150%", 300, 180)]):
        bx = ox + i * 330
        rr(d, [bx, oy + 36, bx + w, oy + 36 + h], fill=CHROME, outline=BORDER, r=4)
        text(d, (bx + 10, oy + 44), lab, MUTED, F_S, CHROME, "screen label")
    rr(d, [ox + 20, oy + 76, ox + 170, oy + 190], fill=BG, outline=BORDER, r=4)
    text(d, (ox + 30, oy + 84), "Hideout", TEXT, F_S, BG, "mini hub")
    rr(d, [ox + 360, oy + 76, ox + 560, oy + 196], fill=BG, outline=ACCENT, r=4)
    text(d, (ox + 370, oy + 84), "Hideout · Chat", TEXT, F_S, BG, "mini chat")
    # right screen gone
    text(d, (ox, oy + 236), "Next time, right screen unplugged:", TEXT, F, CANVAS, "placement fallback")
    rr(d, [ox, oy + 266, ox + 300, oy + 446], fill=CHROME, outline=BORDER, r=4)
    text(d, (ox + 10, oy + 274), "Left screen · 100%", MUTED, F_S, CHROME, "screen label")
    rr(d, [ox + 20, oy + 306, ox + 170, oy + 420], fill=BG, outline=BORDER, r=4)
    text(d, (ox + 30, oy + 314), "Hideout", TEXT, F_S, BG, "mini hub")
    rr(d, [ox + 140, oy + 330, ox + 290, oy + 430], fill=BG, outline=ACCENT, r=4)
    text(d, (ox + 150, oy + 338), "Hideout · Chat", TEXT, F_S, BG, "mini chat")
    notes(d, ox + 330, oy + 270, ["Opens on Hideout's screen, same size in", "screen units, fully on screen.",
                                  "Plug the right screen back in and the", "next opening uses the old spot again."])
    notes(d, 30, 800, ["Sizes are kept in screen-independent units: a window moved to the 150% screen keeps its size to the eye, crisp, without restarting its streams.",
                       "Maximised and snapped windows come back maximised or snapped. A window never opens off screen: at least its title row is on a screen.",
                       "After a crash restart every pop-out comes back where it was, drawn from the cache with \"Starting…\" (contract §2.4, §7.9).",
                       "At sign-in with Hideout starting in the tray, pop-outs come back when you first open Hideout (question 1)."])
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("windows", windows), ("gestures", gestures), ("states", states)]:
        path = os.path.join(OUT, f"ux-h211-proposed-{name}.png")
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
