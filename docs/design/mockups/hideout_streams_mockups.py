"""Mockups for live streams in panels (#976, H2.9). Python + Pillow, no app code, no window.

Run: python docs/design/mockups/hideout_streams_mockups.py [out_dir]
Writes ux-h29-proposed-*.png (default: C:/Users/fuchs/splitkeeb/worktrees/_review/h29) and fails if
any drawn hub label is under 4.5:1. Reuses the tab and supervisor mockups' helpers and
hideout-shell's DARK tokens. One function per stream kind draws every state from data
(`stream_box` for canvas and image, `text_stream` for text); nothing is per module. The room map
and the screen frame are sample module content, not hub chrome, so they are not contrast-checked.
"""

import os
import sys

from PIL import Image, ImageDraw

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402
from hideout_tabs_mockups import (ACCENT, BG, BORDER, CANVAS, CHROME, F, F_B, F_H, F_S, F_TINY,  # noqa: E402
                                  ICON, ICON_S, MUTED, ON_ACCENT, SURFACE, TEXT, PAIRS, rr, text)
from hideout_supervisor_mockups import AMBER, AMBER_BG  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\worktrees\_review\h29"
F_MONO = m.font("consola.ttf", 13)
GL = {"play": "\ue768", "pause": "\ue769", "copy": "\ue8c8", "down": "\ue74b", "warn": "\ue7ba",
      "check": "\ue73e", "sync": "\ue895"}
DIM = 0.38  # share of the frame left when the hub dims a stale frame (rest is SURFACE)


# ---- sample module content (what a module streams; the hub knows none of it) --------------------
ROOMS = [  # canvas shapes: id, rect in canvas units (canvas declares 400 x 240)
    ("desk", (10, 10, 170, 120), "Desk"), ("shelf", (180, 10, 390, 120), "Shelf"),
    ("bed", (10, 130, 230, 230), "Bed"), ("hall", (240, 130, 390, 230), "Hall"),
]
LAMPS = {"desk": [(60, 70), (120, 70)], "shelf": [(220, 60), (285, 60), (350, 60)],
         "bed": [(70, 190), (170, 190)], "hall": [(315, 185)]}
PHASE = [(0xFF, 0x7A, 0x22), (0xFF, 0x4A, 0x6A), (0x9A, 0x6B, 0xFF), (0x3A, 0xC8, 0xFF), (0xFF, 0xB0, 0x30)]


def room_map(w, h, t=0):
    """The lab sample: an animated room map. Rooms are rects, lamps circles; the stream colours them."""
    im = Image.new("RGB", (w, h), SURFACE)
    d = ImageDraw.Draw(im)
    s = min(w / 400, h / 240)  # contain, keep aspect
    ox, oy = (w - 400 * s) / 2, (h - 240 * s) / 2
    for i, (rid, (x0, y0, x1, y1), name) in enumerate(ROOMS):
        c = PHASE[(i + t) % len(PHASE)]
        floor = tuple(int(BG[k] * 0.7 + c[k] * 0.3) for k in range(3))
        rr(d, [ox + x0 * s, oy + y0 * s, ox + x1 * s, oy + y1 * s], fill=floor, outline=BORDER, r=max(2, int(6 * s)))
        d.text((ox + x0 * s + 8 * s, oy + y0 * s + 6 * s), name, fill=TEXT, font=F_S if s < 0.9 else F)
        for j, (cx, cy) in enumerate(LAMPS[rid]):
            lc = PHASE[(i + j + t) % len(PHASE)]
            r = 13 * s
            d.ellipse([ox + cx * s - r, oy + cy * s - r, ox + cx * s + r, oy + cy * s + r], fill=lc, outline=TEXT)
    return im


def screen_frame():
    """A small 1-bit frame (72 x 40), as a keyboard screen preview would stream it."""
    im = Image.new("RGB", (72, 40), (0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rectangle([0, 0, 71, 39], outline=(255, 255, 255))
    d.polygon([(10, 30), (18, 8), (26, 22), (34, 8), (42, 30)], outline=(255, 255, 255))  # fox ears
    d.rectangle([48, 10, 64, 14], fill=(255, 255, 255))
    d.rectangle([48, 18, 60, 22], fill=(255, 255, 255))
    d.rectangle([48, 26, 66, 30], fill=(255, 255, 255))
    return im


def photo_frame(w, h):
    """A smooth frame (an art preview): a soft gradient with a shape."""
    im = Image.new("RGB", (w, h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = (int(40 + 180 * x / w), int(30 + 60 * y / h), int(90 + 100 * (1 - x / w)))
    d = ImageDraw.Draw(im)
    d.ellipse([w * 0.55, h * 0.2, w * 0.85, h * 0.7], fill=(0xFF, 0x9A, 0x2A))
    return im


def dim(im):
    return Image.blend(Image.new("RGB", im.size, SURFACE), im, DIM)


# ---- hub chrome: one pattern for canvas and image ---------------------------------------------------
def chip(d, x, y, label, icon=None, fg=TEXT, bg=CHROME):
    w = d.textlength(label, font=F_S) + (34 if icon else 16)
    rr(d, [x, y, x + w, y + 24], fill=bg, outline=BORDER, r=12)
    tx = x + 8
    if icon:
        d.text((x + 9, y + 7), GL[icon], fill=fg, font=ICON_S)
        tx = x + 26
    text(d, (tx, y + 4), label, fg, F_S, bg, f"chip {label}")
    return w


def stream_box(base, d, box, frame, state, title=None, note=None):
    """Draw a canvas or image stream node in any state. frame: a PIL image already fitted or None."""
    x0, y0, x1, y1 = box
    rr(d, box, fill=SURFACE, outline=BORDER, r=8)
    if frame is not None and state not in ("connecting", "refused"):
        f = dim(frame) if state in ("stalled", "ended", "restarting") else frame
        fx = x0 + (x1 - x0 - f.width) // 2
        fy = y0 + (y1 - y0 - f.height) // 2
        base.paste(f, (fx, fy))
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    if state == "connecting":
        d.arc([cx - 10, cy - 30, cx + 10, cy - 10], 200, 470, fill=ACCENT, width=2)
        lab = "Connecting…"
        text(d, (cx - d.textlength(lab, font=F) / 2, cy - 2), lab, MUTED, F, SURFACE, "connecting")
    elif state == "refused":
        d.text((cx - 7, cy - 32), GL["warn"], fill=MUTED, font=ICON)
        PAIRS.append(("refused mark", MUTED, SURFACE))
        lab = "This preview couldn't be shown."
        text(d, (cx - d.textlength(lab, font=F) / 2, cy - 8), lab, TEXT, F, SURFACE, "refused line")
        text(d, (cx - d.textlength("Open log", font=F) / 2, cy + 16), "Open log", ACCENT, F, SURFACE, "refused link")
    elif state in ("stalled", "restarting"):
        chip(d, x0 + 10, y1 - 34, "Last seen 6 s ago" if state == "stalled" else "Last seen 12 s ago")
    elif state == "ended":
        w = chip(d, x0 + 10, y1 - 34, "Stopped updating")
        text(d, (x0 + 20 + w, y1 - 31), "Try again", ACCENT, F_S, SURFACE, "try again link")
    elif state == "paused":
        chip(d, x0 + 10, y0 + 10, "Paused", icon="pause")
        rr(d, [x1 - 42, y1 - 42, x1 - 10, y1 - 10], fill=CHROME, outline=BORDER, r=16)
        d.text((x1 - 33, y1 - 33), GL["play"], fill=TEXT, font=ICON)
        PAIRS.append(("play button", TEXT, CHROME))
    elif state == "hover":
        rr(d, [x1 - 42, y1 - 42, x1 - 10, y1 - 10], fill=CHROME, outline=ACCENT, r=16)
        d.text((x1 - 33, y1 - 33), GL["pause"], fill=TEXT, font=ICON)
    if title:
        text(d, (x0, y1 + 8), title, TEXT, F_B, CANVAS, f"cell title {title}")
    if note:
        yy = y1 + (30 if title else 8)
        for ln in note:
            text(d, (x0, yy), ln, MUTED, F_S, CANVAS, "cell note")
            yy += 17


def fit(img, w, h, pixel=False):
    """Contain: the largest size that fits, aspect kept. Pixel: whole-number steps, nearest neighbour."""
    s = min(w / img.width, h / img.height)
    if pixel and s >= 1:
        s = int(s)
    nw, nh = max(1, int(img.width * s)), max(1, int(img.height * s))
    return img.resize((nw, nh), Image.NEAREST if pixel else Image.LANCZOS)


# ---- text stream ------------------------------------------------------------------------------------
LOG = ["Scanning for lights…", "Found 4 rooms, 8 lamps.", "Desk strip: 60 LEDs, mirrored.",
       "Shelf: colour from the screen, every 33 ms.", "Bed: night scene from 22:00.",
       "Hall: motion sensor linked.", "Room map ready. Streaming colours."]


def text_stream(d, box, lines, state="follow", caret=True):
    x0, y0, x1, y1 = box
    rr(d, box, fill=SURFACE, outline=ACCENT if state in ("copy", "copied") else BORDER, r=8)
    if state == "trimmed":
        text(d, (x0 + 14, y0 + 12), "Earlier text was trimmed.", MUTED, F_S, SURFACE, "trimmed line")
        y = y0 + 34
    else:
        y = y0 + 12
    fg = MUTED if state == "stale" else TEXT
    for i, ln in enumerate(lines):
        text(d, (x0 + 14, y), ln, fg, F_MONO, SURFACE, "stream text")
        y += 20
    if caret and state in ("follow", "copy", "copied"):
        d.rectangle([x0 + 14 + d.textlength(lines[-1], font=F_MONO) + 2, y - 18, x0 + 21 + d.textlength(lines[-1], font=F_MONO), y - 4], fill=ACCENT)
    if state == "scrolled":
        lab = "Jump to latest"
        w = d.textlength(lab, font=F_S) + 34
        bx = (x0 + x1 - w) / 2
        rr(d, [bx, y1 - 38, bx + w, y1 - 12], fill=ACCENT, r=13)
        d.text((bx + 10, y1 - 31), GL["down"], fill=ON_ACCENT, font=ICON_S)
        text(d, (bx + 26, y1 - 35), lab, ON_ACCENT, F_S, ACCENT, "jump chip")
    if state in ("copy", "copied"):
        lab = "Copied" if state == "copied" else "Copy"
        w = d.textlength(lab, font=F_S) + 36
        rr(d, [x1 - w - 10, y0 + 8, x1 - 10, y0 + 34], fill=CHROME, outline=BORDER, r=5)
        d.text((x1 - w, y0 + 15), GL["check" if state == "copied" else "copy"], fill=TEXT, font=ICON_S)
        text(d, (x1 - w + 18, y0 + 11), lab, TEXT, F_S, CHROME, "copy button")
    if state == "stale":
        chip(d, x0 + 10, y1 - 34, "Last seen 12 s ago")


def caption(d, x, y, title, notes):
    text(d, (x, y), title, TEXT, F_B, CANVAS, f"cell title {title}")
    yy = y + 22
    for ln in notes:
        text(d, (x, yy), ln, MUTED, F_S, CANVAS, "cell note")
        yy += 17


# 1. Every state of a canvas or image stream, and of a text stream ------------------------------------
def states():
    im, d = m.new(1600, 1080)
    m.heading(d, "One pattern per stream kind: every state (canvas shown; image is the same box)")
    CW, CH = 350, 200
    cells = [
        ("connecting", "Connecting…", ["Only after 1 s with no first frame.", "Box keeps its declared size."], 0),
        ("live", "Live", ["Frames as they come, at most 30 a second.", "No live mark: the motion is the sign."], 0),
        ("live", "Back on screen", ["The last frame stays as it was, no mark,", "while the channel opens again (≤ 2 s)."], 2),
        ("paused", "Paused by you", ["Pause button on hover or focus.", "Channel closed, frame kept, full colour."], 1),
        ("stalled", "No new frames", ["Only if the node sets a gap to expect.", "Dimmed frame + \"Last seen\"."], 3),
        ("ended", "Stopped updating", ["The module closed it; 3 tries failed.", "Try again opens it once more."], 3),
        ("restarting", "Module restarting", ["The amber crash bar says why (once).", "The box only dims + \"Last seen\"."], 3),
        ("refused", "Couldn't be shown", ["Over a cap (rate, size, count).", "Details in the module's log."], 0),
    ]
    for i, (st, title, notes, t) in enumerate(cells):
        x = 30 + (i % 4) * (CW + 30)
        y = 70 + (i // 4) * (CH + 100)
        stream_box(im, d, [x, y, x + CW, y + CH], room_map(CW - 20, CH - 20, t), st)
        caption(d, x, y + CH + 8, title, notes)
    # text stream row
    y = 690
    text(d, (30, y), "Text stream: appends, follows the end, copies", TEXT, F_H, CANVAS, "section text")
    y += 40
    TW, TH = 350, 200
    tcells = [
        ("follow", LOG[-6:], "Following", ["New text lands at the end; the view", "follows while you are at the bottom."]),
        ("scrolled", LOG[:6], "Scrolled up", ["Following stops. \"Jump to latest\"", "returns to the end and follows again."]),
        ("copied", LOG[-6:], "Copy", ["On hover or focus: Copy (all text).", "Select with the mouse for a part."]),
        ("stale", LOG[-6:], "Module restarting", ["Text stays, muted. Nothing is lost", "on screen; the bar says why."]),
    ]
    for i, (st, lines, title, notes) in enumerate(tcells):
        x = 30 + i * (TW + 30)
        text_stream(d, [x, y, x + TW, y + TH], lines, st)
        caption(d, x, y + TH + 8, title, notes)
    return im


# 2. A page with three streams, live and while its module restarts -------------------------------------
def page(im, d, x0, y0, restarting=False, t=0):
    W, H = 760, 560
    m.window(d, [x0, y0, x0 + W, y0 + H])
    m.sidebar(d, x0 + 1, y0 + 1, H - 2, m.SIDEBAR, "lights")
    m.strip(d, x0 + 172, y0 + 1, W - 173, [{"key": "keyboard", "name": "Keyboard"},
                                          {"key": "lights", "name": "Lights", "state": "updating" if restarting else None},
                                          {"key": "screens", "name": "Screens"}], 1)
    px, py = x0 + 192, y0 + 50
    if restarting:
        rr(d, [px, py, x0 + W - 20, py + 58], fill=AMBER_BG, r=6)
        d.text((px + 14, py + 10), GL["warn"], fill=AMBER, font=ICON)
        PAIRS.append(("crash bar icon", AMBER, AMBER_BG))
        text(d, (px + 40, py + 6), "Lights stopped unexpectedly. Restarting in 4 s (2 of 5).", TEXT, F_B, AMBER_BG, "crash bar title")
        text(d, (px + 40, py + 30), "Your place on this page is kept.", TEXT, F, AMBER_BG, "crash bar line")
        text(d, (x0 + W - 100, py + 30), "Open log", AMBER, F, AMBER_BG, "crash bar link")
        py += 70
    text(d, (px, py), "Room map", TEXT, F_H, BG, "page heading")
    py += 34
    bw = W - 212
    bh = 230 if not restarting else 200
    st = "restarting" if restarting else "live"
    stream_box(im, d, [px, py, px + bw, py + bh], room_map(bw - 16, bh - 16, t), st)
    py += bh + 14
    half = (bw - 14) // 2
    text(d, (px, py), "Left screen", MUTED, F_B, BG, "sub heading")
    text(d, (px + half + 14, py), "Activity", MUTED, F_B, BG, "sub heading 2")
    py += 24
    sh = y0 + H - py - 16
    stream_box(im, d, [px, py, px + half, py + sh], fit(screen_frame(), half - 16, sh - 16, pixel=True), st)
    text_stream(d, [px + half + 14, py, px + bw, py + sh], [s[:30] for s in LOG[-5:]], "stale" if restarting else "follow")


def pages():
    im, d = m.new(1600, 700)
    m.heading(d, "A module page with three streams: live, and while its module restarts")
    page(im, d, 30, 70, restarting=False, t=1)
    page(im, d, 810, 70, restarting=True, t=1)
    text(d, (30, 645), "Left: canvas (room map), image (left screen, whole-pixel steps), text (activity). "
         "Right: one amber bar names the cause; each stream only dims and says \"Last seen\".", MUTED, F, CANVAS, "foot")
    return im


# 3. Sizing, scaling, reduced motion and the pause control --------------------------------------------
def sizing():
    im, d = m.new(1600, 900)
    m.heading(d, "Sizing and scaling: fit inside, keep the shape, never stretch, never jump")
    y = 70
    # canvas: same canvas in three widths
    for i, (w, h) in enumerate([(460, 276), (300, 180), (200, 120)]):
        x = 30 + sum([480, 320][:i])
        stream_box(im, d, [x, y, x + w, y + h], room_map(w - 16, h - 16, 0), "live")
    caption(d, 30, y + 290, "Canvas: scales with the page width, aspect from its declared size", [
        "Shapes are vectors: any scale stays sharp. Height = width x declared aspect, capped at 60% of the page.",
        "Below 50% the node stops shrinking and the page scrolls sideways. Click targets stay >= 24 px."])
    # image: pixel vs smooth, letterbox
    x = 1060
    sf = screen_frame()
    stream_box(im, d, [x, y, x + 250, y + 160], fit(sf, 234, 144, pixel=True), "live")
    stream_box(im, d, [x + 270, y, x + 520, y + 160], fit(photo_frame(320, 180), 234, 144), "live")
    caption(d, x, y + 172, "Image: contain, centred, letterbox", [
        "pixel: whole-number steps, no blur", "smooth: filtered (photos, art)"])
    # no jump on frame size change
    y = 470
    stream_box(im, d, [30, y, 330, y + 170], fit(photo_frame(320, 180), 284, 154), "live")
    stream_box(im, d, [350, y, 650, y + 170], fit(photo_frame(120, 160), 284, 154), "live")
    caption(d, 30, y + 180, "A new frame size never moves the page", [
        "The box comes from the node's declared aspect; frames fit inside it.",
        "Without one, the first frame sets it once; later sizes letterbox."])
    # reduced motion
    x = 700
    for k in range(3):
        stream_box(im, d, [x + k * 150, y, x + k * 150 + 136, y + 90], room_map(124, 78, k), "live")
    caption(d, x, y + 100, "Animation effects off (Windows): 2 frames a second", [
        "Still the real colours, as a slow slide show; no fades between frames.",
        "Text follows by jumping, not smooth scrolling."])
    # pause control
    x = 1180
    stream_box(im, d, [x, y, x + 190, y + 120], room_map(174, 104, 0), "hover")
    stream_box(im, d, [x + 200, y, x + 390, y + 120], room_map(174, 104, 0), "paused")
    caption(d, x, y + 130, "Pause / Play (hub-drawn)", ["On every canvas and image stream (WCAG 2.2.2).", "Shows on hover or focus; Space toggles."])
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("states", states), ("page", pages), ("sizing", sizing)]:
        path = os.path.join(OUT, f"ux-h29-proposed-{name}.png")
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
