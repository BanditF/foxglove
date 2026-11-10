"""Mockups for the Hideout tab strip (#953). Python + Pillow, no app code.

Run: python docs/design/mockups/hideout_tabs_mockups.py [out_dir]
Writes ux-tabs-proposed-*.png (default: C:/Users/fuchs/splitkeeb/review/hideout-tabs) and prints
the WCAG contrast of every text/fill pair it draws; it fails if one is under 4.5:1.
Colours are hideout-shell's DARK tokens (crates/hideout-shell/src/theme.rs).
"""

import os
import sys

from PIL import Image, ImageDraw, ImageFont

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\review\hideout-tabs"
FONTS = r"C:\Windows\Fonts"

BG = (0x17, 0x15, 0x14)
CHROME = (0x20, 0x1D, 0x1B)
SURFACE = (0x2A, 0x26, 0x23)
BORDER = (0x3D, 0x37, 0x33)
TEXT = (0xF2, 0xEE, 0xEA)
MUTED = (0xA8, 0xA0, 0x99)
ACCENT = (0xFF, 0x8A, 0x2A)
ON_ACCENT = (0x1A, 0x0E, 0x04)
DANGER = (0xFF, 0x6B, 0x6B)
CANVAS = (0x12, 0x10, 0x0F)

PAIRS = []  # (label, fg, bg) for the contrast report


def font(name, size):
    return ImageFont.truetype(os.path.join(FONTS, name), size)


F = font("segoeui.ttf", 14)
F_S = font("segoeui.ttf", 12)
F_B = font("seguisb.ttf", 14)
F_H = font("seguisb.ttf", 20)
F_T = font("seguisb.ttf", 22)
F_TINY = font("seguisb.ttf", 11)
ICON = font("segmdl2.ttf", 14)
ICON_S = font("segmdl2.ttf", 10)

G = {  # Segoe MDL2 glyphs
    "keyboard": "\ue765", "lights": "\uea80", "scenes": "\ue8b2", "chat": "\ue8bd",
    "art": "\ue8b9", "screens": "\ue7f4", "images": "\ue91b", "modules": "\ue71d",
    "settings": "\ue713", "close": "\ue711", "warn": "\ue7ba", "popout": "\ue8a7",
    "down": "\ue70d", "left": "\ue76b", "right": "\ue76c", "sync": "\ue895", "dock": "\ue8a0",
}


def lum(c):
    def ch(v):
        s = v / 255
        return s / 12.92 if s <= 0.04045 else ((s + 0.055) / 1.055) ** 2.4
    return 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])


def contrast(a, b):
    la, lb = lum(a), lum(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def text(d, xy, s, fill, f=F, bg=None, label=None):
    d.text(xy, s, fill=fill, font=f)
    if bg is not None:
        PAIRS.append((label or s[:24], fill, bg))


def rr(d, box, fill=None, outline=None, r=6, w=1):
    d.rounded_rectangle(box, r, fill=fill, outline=outline, width=w)


def callout(d, box, title, lines, anchor=None):
    rr(d, box, fill=CHROME, outline=ACCENT, r=8)
    x, y = box[0] + 14, box[1] + 12
    text(d, (x, y), title, MUTED, F_B, CHROME)
    y += 28
    for ln in lines:
        text(d, (x, y), ln, TEXT, F, CHROME)
        y += 22
    if anchor:
        d.line([(box[0], box[1] + 20), anchor], fill=ACCENT, width=1)


def sidebar(d, x0, y0, h, items, selected, badges=None):
    badges = badges or {}
    d.rectangle([x0, y0, x0 + 170, y0 + h], fill=CHROME)
    text(d, (x0 + 14, y0 + 12), "Hideout", TEXT, F_B, CHROME)
    y = y0 + 46
    for key, name in items:
        if name == "-":
            d.line([(x0 + 14, y + 6), (x0 + 156, y + 6)], fill=BORDER)
            y += 14
            continue
        sel = key == selected
        fill = ACCENT if sel else CHROME
        fg = ON_ACCENT if sel else TEXT
        if sel:
            rr(d, [x0 + 8, y, x0 + 162, y + 30], fill=ACCENT, r=5)
        d.text((x0 + 16, y + 8), G[key], fill=fg, font=ICON)
        text(d, (x0 + 40, y + 5), name, fg, F, fill, f"sidebar {name}{' (selected)' if sel else ''}")
        b = badges.get(key)
        if b == "dot":
            d.ellipse([x0 + 146, y + 11, x0 + 154, y + 19], fill=ACCENT)
        y += 34
    d.line([(x0 + 170, y0), (x0 + 170, y0 + h)], fill=BORDER)


SIDEBAR = [("keyboard", "Keyboard"), ("lights", "Lights"), ("scenes", "Scenes"),
           ("screens", "Screens"), ("art", "Art"), ("chat", "Chat"), ("images", "Images"),
           ("-", "-"), ("modules", "Modules"), ("settings", "Settings")]

TAB_H = 36


def tab(d, x, y, t, active=False, w=None, hover=False):
    """t: dict(key, name, badge=None|'dot'|'dirty'|int, state=None|'crash'|'updating'|'off'|'popped')."""
    name, state, badge = t["name"], t.get("state"), t.get("badge")
    tw = d.textlength(name, font=F)
    w = w or int(tw + 68 + (24 if isinstance(badge, int) else 0) + (14 if state == "popped" else 0))
    fill = BG if active else CHROME
    d.rectangle([x, y, x + w, y + TAB_H], fill=fill)
    if active:
        d.rectangle([x, y, x + w, y + 2], fill=ACCENT)
    d.line([(x + w, y + 8), (x + w, y + TAB_H - 8)], fill=BORDER)
    fg = TEXT if active else MUTED
    if state == "off" or state == "popped":
        fg = MUTED
    ix = x + 12
    if state == "crash":
        d.text((ix, y + 11), G["warn"], fill=DANGER, font=ICON)
        PAIRS.append(("tab crash mark", DANGER, fill))
    elif state == "updating":
        d.arc([ix - 1, y + 10, ix + 15, y + 26], 200, 470, fill=ACCENT, width=2)
        d.text((ix + 3, y + 13), G[t["key"]], fill=fg, font=ICON_S)
    else:
        d.text((ix, y + 11), G[t["key"]], fill=fg, font=ICON)
    if badge == "dot":
        d.ellipse([ix + 11, y + 8, ix + 18, y + 15], fill=ACCENT, outline=fill, width=1)
    label = name
    clip = x + w - 30 - (24 if isinstance(badge, int) else 0)
    while d.textlength(label, font=F) > clip - (ix + 24) and len(label) > 2:
        label = label[:-2] + "…"
    text(d, (ix + 24, y + 8), label, fg, F, fill, f"tab {name}{' (active)' if active else ''}")
    if state == "popped":
        d.text((x + w - 24, y + 13), G["popout"], fill=MUTED, font=ICON_S)
    if isinstance(badge, int):
        s = "99+" if badge > 99 else str(badge)
        bw = max(18, int(d.textlength(s, font=F_TINY)) + 10)
        bx = x + w - 30 - bw + 4
        rr(d, [bx, y + 10, bx + bw, y + 26], fill=ACCENT, r=8)
        text(d, (bx + (bw - d.textlength(s, font=F_TINY)) / 2, y + 11), s, ON_ACCENT, F_TINY, ACCENT, "count pill")
    cx = x + w - 24
    if badge == "dirty" and not hover:
        d.ellipse([cx + 3, y + 14, cx + 11, y + 22], fill=fg)
    elif active or hover:
        if hover:
            rr(d, [cx - 3, y + 9, cx + 17, y + 27], fill=SURFACE, r=4)
        d.text((cx + 1, y + 13), G["close"], fill=fg, font=ICON_S)
    return x + w


def strip(d, x0, y0, w, tabs, active, overflow=False):
    d.rectangle([x0, y0, x0 + w, y0 + TAB_H], fill=CHROME)
    x = x0
    end = x0 + w - (84 if overflow else 0)
    for i, t in enumerate(tabs):
        tw = t.get("w")
        nx = tab(d, x, y0, t, active=(i == active), w=tw, hover=t.get("hover", False))
        x = nx
        if x > end:
            break
    if overflow:
        d.rectangle([end, y0, x0 + w, y0 + TAB_H], fill=CHROME)
        d.line([(end, y0 + 6), (end, y0 + TAB_H - 6)], fill=BORDER)
        for i, g in enumerate(["left", "right", "down"]):
            d.text((end + 10 + i * 25, y0 + 12), G[g], fill=TEXT, font=ICON_S)
        PAIRS.append(("overflow arrows", TEXT, CHROME))
    d.line([(x0, y0 + TAB_H), (x0 + w, y0 + TAB_H)], fill=BORDER)
    if active is not None:
        # the active tab joins the page: no line under it
        pass


def window(d, box, title="Hideout"):
    rr(d, box, fill=BG, outline=BORDER, r=8)


def page_body(d, x, y, w, h, heading, lines=()):
    text(d, (x + 20, y + 16), heading, TEXT, F_H, BG)
    yy = y + 56
    for ln in lines:
        text(d, (x + 20, yy), ln, MUTED, F, BG)
        yy += 22


def keyboard_canvas(d, x, y):
    cols = [(0xFF, 0x5A, 0x1F), (0xFF, 0x7A, 0x22), (0xFF, 0x9A, 0x2A), (0xE8, 0x3A, 0x1A)]
    for r in range(3):
        for c in range(12):
            rr(d, [x + c * 30, y + r * 30, x + c * 30 + 24, y + r * 30 + 24], fill=cols[(r + c) % 4], r=4)


def heading(d, s):
    text(d, (30, 22), s, TEXT, F_T, CANVAS)


def new(w, h):
    im = Image.new("RGB", (w, h), CANVAS)
    return im, ImageDraw.Draw(im)


# 1. Many tabs and every badge/state ------------------------------------------------------------
def states():
    im, d = new(1480, 720)
    heading(d, "Open pages are tabs; the sidebar stays the launcher")
    window(d, [30, 70, 1110, 600])
    sidebar(d, 31, 71, 528, SIDEBAR, "keyboard", {"lights": "dot"})
    tabs = [
        {"key": "keyboard", "name": "Keyboard"},
        {"key": "lights", "name": "Lights", "badge": "dot"},
        {"key": "scenes", "name": "Scenes", "badge": "dirty"},
        {"key": "chat", "name": "Chat", "badge": 3},
        {"key": "art", "name": "Art", "state": "crash"},
        {"key": "screens", "name": "Screens", "state": "updating"},
        {"key": "images", "name": "Images", "state": "popped"},
    ]
    strip(d, 202, 72, 907, tabs, 0)
    page_body(d, 202, 110, 907, 480, "Keyboard")
    keyboard_canvas(d, 222, 180)
    text(d, (222, 290), "Layer: Base   ·   Battery: left 84%, right 79%", MUTED, F, BG)
    text(d, (222, 330), "Ctrl+Tab next tab · Ctrl+1–9 jump · Ctrl+W or middle-click closes", MUTED, F_S, BG)
    # legend row under the window
    y = 620
    items = [("dot", "Attention: something new wants a look"),
             ("dirty", "Unsaved: closing asks first; shows × on hover"),
             ("count", "Count: new items, 99+ above 99"),
             ("crash", "Stopped: the page says why and offers Start again"),
             ("updating", "Updating: your place on the page is kept"),
             ("popped", "Popped out: click to bring its window forward")]
    for i, (k, s) in enumerate(items):
        x = 30 + (i % 3) * 330
        yy = y + (i // 3) * 34
        if k == "dot":
            d.ellipse([x + 4, yy + 6, x + 12, yy + 14], fill=ACCENT)
        elif k == "dirty":
            d.ellipse([x + 4, yy + 6, x + 12, yy + 14], fill=TEXT)
        elif k == "count":
            rr(d, [x, yy + 2, x + 18, yy + 18], fill=ACCENT, r=8)
            d.text((x + 5, yy + 2), "3", fill=ON_ACCENT, font=F_TINY)
        elif k == "crash":
            d.text((x + 1, yy + 2), G["warn"], fill=DANGER, font=ICON)
        elif k == "updating":
            d.arc([x, yy + 1, x + 16, yy + 17], 200, 470, fill=ACCENT, width=2)
        elif k == "popped":
            d.text((x + 2, yy + 4), G["popout"], fill=MUTED, font=ICON_S)
        text(d, (x + 26, yy), s, TEXT, F_S, CANVAS)
    callout(d, [1130, 70, 1460, 330], "The strip", [
        "Icon and title come from the page.",
        "Active tab: page colour, orange top",
        "edge, full-contrast label (no orange",
        "fill behind text).",
        "Close × on the active and hovered tab;",
        "a dirty tab shows a dot there instead.",
        "Sidebar: orange fill, dark label.",
    ])
    callout(d, [1130, 350, 1460, 560], "States come from the module", [
        "Badge: the module sets it (ui.badge).",
        "Stopped, updating, off: the hub's",
        "module state, same words as the",
        "Modules page.",
        "Background tabs get ui.visibility",
        "hidden, so their streams stop.",
    ])
    return im


# 2. None open, one open ------------------------------------------------------------------------
def empty_and_one():
    im, d = new(1400, 600)
    heading(d, "Nothing open, then one tab")
    window(d, [30, 70, 680, 560])
    sidebar(d, 31, 71, 488, SIDEBAR, None)
    d.rectangle([202, 72, 679, 72 + TAB_H], fill=CHROME)
    d.line([(202, 72 + TAB_H), (679, 72 + TAB_H)], fill=BORDER)
    cx = 440
    text(d, (cx - d.textlength("Nothing open", font=F_H) / 2, 230), "Nothing open", TEXT, F_H, BG)
    s = "Pick a page in the sidebar."
    text(d, (cx - d.textlength(s, font=F) / 2, 266), s, MUTED, F, BG)
    for i, (k, v) in enumerate([("Ctrl+K", "Find a page or action"), ("Ctrl+Shift+T", "Reopen closed tab")]):
        y = 320 + i * 28
        text(d, (cx - 150, y), k, TEXT, F_B, BG)
        text(d, (cx - 10, y), v, MUTED, F, BG)
    window(d, [720, 70, 1370, 560])
    sidebar(d, 721, 71, 488, SIDEBAR, "lights")
    strip(d, 892, 72, 477, [{"key": "lights", "name": "Lights"}], 0)
    page_body(d, 892, 110, 477, 440, "Lights", ["Look: Ember  ·  Effect: Sweep  ·  Palette: Bandit"])
    for i in range(10):
        rr(d, [912 + i * 40, 210, 912 + i * 40 + 32, 242], fill=(0xFF, 0x60 + i * 8, 0x20), r=6)
    text(d, (912, 270), "One tab still shows: it says what is open", MUTED, F_S, BG)
    text(d, (912, 290), "and gives the close button a home.", MUTED, F_S, BG)
    d.line([(560, 586), (560, 586)], fill=BORDER)
    text(d, (30, 570), "Left: first start or every tab closed. The page area says where to go; no blank orange.", MUTED, F_S, CANVAS)
    return im


# 3. Overflow, list menu, context menu ---------------------------------------------------------
def overflow():
    im, d = new(1400, 640)
    heading(d, "Many tabs: shrink, then scroll; the list shows them all")
    window(d, [30, 70, 1000, 600])
    sidebar(d, 31, 71, 528, SIDEBAR, "chat")
    names = [("keyboard", "Keyboard"), ("lights", "Lights"), ("scenes", "Scenes"), ("chat", "Chat"),
             ("art", "Art"), ("screens", "Screens"), ("images", "Images"), ("settings", "Settings")]
    tabs = [{"key": k, "name": n, "w": 108} for k, n in names]
    tabs[3]["badge"] = 3
    tabs[3]["w"] = 120
    tabs[1]["badge"] = "dot"
    strip(d, 202, 72, 797, tabs, 3, overflow=True)
    page_body(d, 202, 110, 797, 480, "Chat")
    rr(d, [222, 170, 980, 230], fill=SURFACE, r=6)
    text(d, (236, 188), "Three new replies while you were on Lights.", TEXT, F, SURFACE)
    # list menu (the down chevron)
    mx, my = 760, 112
    rr(d, [mx, my, mx + 230, my + 8 * 30 + 12], fill=SURFACE, outline=BORDER, r=6)
    for i, (k, n) in enumerate(names):
        y = my + 6 + i * 30
        sel = i == 3
        if sel:
            rr(d, [mx + 4, y, mx + 226, y + 28], fill=ACCENT, r=4)
        fg = ON_ACCENT if sel else TEXT
        bg = ACCENT if sel else SURFACE
        d.text((mx + 12, y + 7), G[k], fill=fg, font=ICON)
        text(d, (mx + 36, y + 4), n, fg, F, bg, f"list {n}{' (selected)' if sel else ''}")
        text(d, (mx + 190, y + 6), f"Ctrl+{i + 1}" if i < 8 else "", MUTED if not sel else ON_ACCENT, F_S, bg, f"list shortcut{' (selected)' if sel else ''}")
    # context menu on the Lights tab
    cx, cy = 330, 112
    rr(d, [cx, cy, cx + 220, cy + 7 * 28 + 24], fill=SURFACE, outline=BORDER, r=6)
    items = ["Close", "Close others", "Close to the right", "-", "Pop out", "Move left", "Move right", "Reopen closed tab"]
    y = cy + 8
    for it in items:
        if it == "-":
            d.line([(cx + 8, y + 4), (cx + 212, y + 4)], fill=BORDER)
            y += 10
            continue
        text(d, (cx + 14, y + 3), it, TEXT, F, SURFACE)
        y += 28
    callout(d, [1030, 70, 1380, 360], "Overflow", [
        "Tabs shrink to 96 px, then the strip",
        "scrolls (wheel or ‹ ›) and keeps the",
        "active tab in view.",
        "The v button lists every open tab,",
        "with its badge and Ctrl+number.",
        "Right-click: Close, Close others, Pop",
        "out, Move left/right (the keyboard",
        "way to reorder). Or drag to reorder.",
    ], anchor=(990, 90))
    return im


# 4. Stopped page, pop-out with dock back, close with unsaved work ------------------------------
def crash_popout_dirty():
    im, d = new(1400, 720)
    heading(d, "A stopped module, a popped-out tab, and closing with unsaved work")
    window(d, [30, 70, 760, 520])
    sidebar(d, 31, 71, 448, SIDEBAR, "art")
    tabs = [{"key": "keyboard", "name": "Keyboard"}, {"key": "art", "name": "Art", "state": "crash"},
            {"key": "images", "name": "Images", "state": "popped"}, {"key": "scenes", "name": "Scenes", "badge": "dirty"}]
    strip(d, 202, 72, 557, tabs, 1)
    text(d, (222, 126), "Art", TEXT, F_H, BG)
    rr(d, [222, 166, 740, 290], fill=SURFACE, outline=DANGER, r=6)
    d.text((238, 182), G["warn"], fill=DANGER, font=ICON)
    text(d, (262, 178), "Stopped after 5 crashes", TEXT, F_B, SURFACE)
    text(d, (262, 202), "Art crashed 5 times in 10 minutes.", MUTED, F, SURFACE)
    bx = 262
    for i, lab in enumerate(["Start again", "Start with default settings", "Go back to 0.4.2"]):
        bw = d.textlength(lab, font=F) + 24
        if i == 0:
            rr(d, [bx, 238, bx + bw, 270], fill=ACCENT, r=5)
            text(d, (bx + 12, 243), lab, ON_ACCENT, F, ACCENT, "primary button")
        else:
            rr(d, [bx, 238, bx + bw, 270], outline=BORDER, r=5)
            text(d, (bx + 12, 243), lab, TEXT, F, SURFACE)
        bx += bw + 10
    text(d, (222, 304), "Copy diagnostics", ACCENT, F, BG, "link")
    text(d, (222, 340), "The last page Art drew stays below, greyed.", MUTED, F_S, BG)
    # the pop-out window
    px, py = 800, 70
    rr(d, [px, py, px + 570, py + 300], fill=BG, outline=BORDER, r=8)
    d.rectangle([px + 1, py + 1, px + 569, py + 34], fill=CHROME)
    text(d, (px + 14, py + 8), "Hideout · Images", TEXT, F, CHROME)
    lab = "Dock back"
    bw = d.textlength(lab, font=F_S) + 34
    rr(d, [px + 570 - 110 - bw, py + 6, px + 570 - 110, py + 28], fill=SURFACE, outline=BORDER, r=4)
    d.text((px + 570 - 104 - bw, py + 11), G["dock"], fill=TEXT, font=ICON_S)
    text(d, (px + 570 - 100 - bw + 8, py + 9), lab, TEXT, F_S, SURFACE)
    for i, g in enumerate(["\ue921", "\ue922", "\ue8bb"]):
        d.text((px + 470 + i * 34, py + 11), g, fill=TEXT, font=ICON_S)
    for i in range(6):
        c = (0x40 + i * 20, 0x30 + i * 6, 0x28)
        rr(d, [px + 20 + i * 90, py + 60, px + 100 + i * 90, py + 140], fill=c, r=6)
    text(d, (px + 20, py + 160), "Same page, own window. Closing it or Dock back", MUTED, F_S, BG)
    text(d, (px + 20, py + 180), "puts it back as a tab where it was.", MUTED, F_S, BG)
    # dialog
    dx, dy = 800, 400
    rr(d, [dx, dy, dx + 440, dy + 170], fill=SURFACE, outline=BORDER, r=8)
    text(d, (dx + 20, dy + 18), "Close Scenes?", TEXT, F_B, SURFACE)
    text(d, (dx + 20, dy + 48), "Your changes in Scenes aren't saved.", TEXT, F, SURFACE)
    bx = dx + 440 - 20
    for i, lab in enumerate(["Close anyway", "Keep open"]):
        bw = d.textlength(lab, font=F) + 24
        bx -= bw
        if lab == "Keep open":
            rr(d, [bx, dy + 116, bx + bw, dy + 148], fill=ACCENT, r=5)
            text(d, (bx + 12, dy + 121), lab, ON_ACCENT, F, ACCENT, "dialog default")
        else:
            rr(d, [bx, dy + 116, bx + bw, dy + 148], outline=BORDER, r=5)
            text(d, (bx + 12, dy + 121), lab, TEXT, F, SURFACE)
        bx -= 10
    text(d, (30, 560), "Stopped: the tab keeps its place with a red mark; the page has the reason and the ways out (contract §7.8).", MUTED, F_S, CANVAS)
    text(d, (30, 582), "Popped out: the tab stays, dimmed with ↗, so order and Ctrl+number don't shift; clicking it raises the window.", MUTED, F_S, CANVAS)
    text(d, (30, 604), "Unsaved (dirty badge): close asks; Keep open is the default (Enter), Esc keeps it open too.", MUTED, F_S, CANVAS)
    text(d, (30, 626), "Module turned off: its tabs stay greyed with Turn on. Module removed: its tabs close and a note says so.", MUTED, F_S, CANVAS)
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("strip-states", states), ("empty-and-one", empty_and_one),
                     ("overflow", overflow), ("stopped-popout-close", crash_popout_dirty)]:
        path = os.path.join(OUT, f"ux-tabs-proposed-{name}.png")
        fn().save(path)
        print(path)
    worst = {}
    for label, fg, bg in PAIRS:
        r = contrast(fg, bg)
        if label not in worst or r < worst[label]:
            worst[label] = r
    bad = {k: v for k, v in worst.items() if v < 4.5}
    keys = [k for k in worst if "selected" in k or "active" in k or k in ("count pill", "primary button", "dialog default", "tab crash mark")]
    for k in sorted(set(keys)):
        print(f"  {worst[k]:5.2f}:1  {k}")
    print(f"min over {len(worst)} pairs: {min(worst.values()):.2f}:1")
    if bad:
        print("UNDER 4.5:1:", bad)
        sys.exit(1)


if __name__ == "__main__":
    main()
