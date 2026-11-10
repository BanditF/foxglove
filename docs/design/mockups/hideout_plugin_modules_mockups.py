"""Mockups for the existing plugins as Hideout modules (#975, H2.8). Python + Pillow, no app code.

Run: python docs/design/mockups/hideout_plugin_modules_mockups.py [out_dir]
Writes ux-h28-proposed-*.png (default: C:/Users/fuchs/splitkeeb/worktrees/_review/h28) and fails if
any drawn label is under 4.5:1. Reuses the tab and supervisor mockups' helpers and hideout-shell's
DARK tokens. Every tile is drawn by one function from data (the module's tree), never per plugin.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hideout_tabs_mockups as m  # noqa: E402
from hideout_tabs_mockups import (ACCENT, BG, BORDER, CANVAS, CHROME, DANGER, F, F_B, F_H, F_S,  # noqa: E402
                                  F_TINY, ICON, ICON_S, MUTED, ON_ACCENT, SURFACE, TEXT, PAIRS, rr, text)
from hideout_supervisor_mockups import AMBER, AMBER_BG, button  # noqa: E402

OUT = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\fuchs\splitkeeb\worktrees\_review\h28"
GREEN = (0x7B, 0xD8, 0x8F)
GREY_FG = (0x9A, 0x92, 0x8B)  # greyed body text on SURFACE (stale view)
F_BIG = m.font("seguisb.ttf", 26)
F_XL = m.font("seguisb.ttf", 34)

# tone -> colour of the glance value (spec item S1: `tone` prop)
TONE = {"neutral": TEXT, "active": ACCENT, "live": DANGER, "ok": GREEN, "attention": AMBER, "off": MUTED}

GL = {"mic": "\ue720", "timer": "\ue916", "video": "\ue714", "color": "\ue790", "lights": "\uea80",
      "more": "\ue712", "warn": "\ue7ba", "sync": "\ue895", "key": "\ue765"}

TW, TH = 196, 140  # tile size in the tray panel (two columns)


def chip(d, x, y, label, bg):
    w = d.textlength(label, font=F_TINY) + 10
    rr(d, [x, y, x + w, y + 18], outline=BORDER, r=4)
    text(d, (x + 5, y + 2), label, MUTED, F_TINY, bg, "key chip")
    return w


def small_button(d, x, y, label, primary=True, enabled=True, bg=SURFACE):
    w = d.textlength(label, font=F) + 20
    if primary and enabled:
        rr(d, [x, y, x + w, y + 28], fill=ACCENT, r=5)
        text(d, (x + 10, y + 4), label, ON_ACCENT, F, ACCENT, "tile primary button")
    else:
        rr(d, [x, y, x + w, y + 28], outline=BORDER, r=5)
        text(d, (x + 10, y + 4), label, TEXT if enabled else MUTED, F, bg, f"tile button {label}")
    return w


def tile(d, x, y, t):
    """One tile, any module. t: icon, title, value, sub, tone, button, key, meter, state, stale.

    Hub frame: icon + title (manifest), state mark, the more (...) menu, the amber restart strip,
    the greyed stale view. Module body: value (glance, with tone), sub line, optional meter, one
    button (optionally tied to a declared action, so the hub shows its bound key)."""
    state = t.get("state")
    rr(d, [x, y, x + TW, y + TH], fill=SURFACE, outline=DANGER if state in ("stopped", "needs-hub", "quarantined") else BORDER, r=8)
    stale = state in ("restarting", "stopped", "starting")
    # header
    ix, iy = x + 12, y + 12
    if state in ("stopped", "needs-hub", "quarantined"):
        d.text((ix, iy + 1), GL["warn"], fill=DANGER, font=ICON)
        PAIRS.append(("tile stop mark", DANGER, SURFACE))
    elif state in ("starting", "restarting"):
        d.arc([ix - 3, iy - 2, ix + 15, iy + 16], 200, 470, fill=ACCENT, width=2)
        d.text((ix + 2, iy + 3), GL[t["icon"]], fill=TEXT, font=ICON_S)
    else:
        d.text((ix, iy + 1), GL[t["icon"]], fill=MUTED, font=ICON)
    text(d, (x + 34, y + 8), t["title"], MUTED, F_B, SURFACE, f"tile title {t['title']}")
    d.text((x + TW - 24, y + 12), GL["more"], fill=MUTED, font=ICON)
    by = y + 36
    if state == "restarting":
        rr(d, [x + 1, y + 34, x + TW - 1, y + 56], fill=AMBER_BG, r=0)
        text(d, (x + 12, y + 36), t.get("restart", "Restarting in 4 s (2 of 5)"), AMBER, F_S, AMBER_BG, "tile restart strip")
        by = y + 58
    if state in ("stopped", "needs-hub", "quarantined"):
        words = {"stopped": ("Stopped after", "5 crashes"), "needs-hub": ("Needs a newer", "Hideout"),
                 "quarantined": ("Its tile couldn't", "be shown")}[state]
        text(d, (x + 12, by + 2), words[0], TEXT, F_B, SURFACE, "tile stopped line")
        text(d, (x + 12, by + 22), words[1], TEXT, F_B, SURFACE, "tile stopped line 2")
        lab = "Update Hideout" if state == "needs-hub" else "Open"
        small_button(d, x + 12, y + TH - 38, lab, primary=True)
        return
    vcol = GREY_FG if stale else TONE[t.get("tone", "neutral")]
    vf = F_BIG if len(t["value"]) <= 12 else F_B
    text(d, (x + 12, by - 2), t["value"], vcol, vf, SURFACE, f"tile value {t['value']}")
    sy = by + (32 if vf is F_BIG else 22)
    if stale and state != "starting":  # the sub line gives way to the stale caption
        text(d, (x + 12, sy), "Last seen 12 s ago", MUTED, F_S, SURFACE, "stale caption")
    elif t.get("sub"):
        text(d, (x + 12, sy), t["sub"], GREY_FG if stale else MUTED, F_S, SURFACE, f"tile sub {t['sub']}")
    if t.get("meter") is not None and not stale:
        my = sy + 19
        rr(d, [x + 12, my, x + TW - 12, my + 6], fill=BORDER, r=3)
        rr(d, [x + 12, my, x + 12 + int((TW - 24) * t["meter"]), my + 6], fill=TONE[t.get("tone", "active")], r=3)
    if t.get("button"):
        bw = small_button(d, x + 12, y + TH - 38, t["button"], primary=t.get("primary", True),
                          enabled=not stale)
        if t.get("key"):
            chip(d, x + 12 + bw + 8, y + TH - 33, t["key"], SURFACE)


def tray_panel(d, x, y, tiles, title="Hideout", footer=True):
    rows = (len(tiles) + 1) // 2
    h = 58 + rows * (TH + 12) + (40 if footer else 0)
    rr(d, [x, y, x + 2 * TW + 36, y + h], fill=CHROME, outline=BORDER, r=10)
    d.text((x + 14, y + 16), GL["key"], fill=TEXT, font=ICON)
    text(d, (x + 38, y + 12), title, TEXT, F_B, CHROME, "tray panel title")
    text(d, (x + 38, y + 32), "Keyboard connected · Profile: Work", MUTED, F_S, CHROME, "tray status line")
    for i, t in enumerate(tiles):
        tile(d, x + 12 + (i % 2) * (TW + 12), y + 58 + (i // 2) * (TH + 12), t)
    if footer:
        fy = y + h - 32
        text(d, (x + 14, fy), "Open Hideout", ACCENT, F, CHROME, "tray link")
        text(d, (x + 2 * TW - 70, fy), "Edit tiles", ACCENT, F, CHROME, "tray link 2")
    return h


MIC = dict(icon="mic", title="Mic mute")
FOC = dict(icon="timer", title="Focus timer")
OBS = dict(icon="video", title="OBS")
ICUE = dict(icon="color", title="Corsair iCUE")
ORGB = dict(icon="lights", title="OpenRGB")


def T(base, **kw):
    t = dict(base)
    t.update(kw)
    return t


# 1. The tray panel and the tile anatomy -----------------------------------------------------------
def tiles_overview():
    im, d = m.new(1500, 900)
    m.heading(d, "Tray panel: five plugin modules, one tile pattern (normal states)")
    tiles = [
        T(MIC, value="Muted", sub="Default communications mic", tone="live", button="Unmute", key="F13"),
        T(FOC, value="18:42", sub="Focus · round 2 of 4", tone="active", meter=0.25, button="Pause", key="F14"),
        T(OBS, value="Live", sub="Scene: Gameplay", tone="live"),
        T(ICUE, value="Following", sub="K70 RGB PRO", tone="active"),
        T(ORGB, value="Following", sub="OpenRGB on this PC", tone="active"),
    ]
    tray_panel(d, 30, 70, tiles)

    # anatomy
    ax, ay = 520, 70
    text(d, (ax, ay), "One tile, any module", TEXT, F_H, CANVAS)
    tile(d, ax, ay + 40, tiles[0])
    notes = [
        ("Hub frame (from the manifest)", ["Icon and title: [[ui.panels]] slot = \"tile\".",
                                           "··· menu: Open Mic mute · Hide tile · Move.",
                                           "State mark and the amber strip: §7.8 states."]),
        ("Module body (ui.register tree)", ["value: the glance, one line, with a tone.",
                                            "label: one sub line. meter: optional.",
                                            "button: at most one, the primary action."]),
        ("Hub extras from data", ["Key chip: the button names a declared action,",
                                  "so the hub shows its bound key (UX3).",
                                  "Click anywhere else: opens the module's page."]),
    ]
    y = ay + 40
    for title, lines in notes:
        m.callout(d, [ax + TW + 30, y, ax + TW + 470, y + 40 + 22 * len(lines)], title, lines)
        y += 60 + 22 * len(lines)

    # tones
    tx, ty = 520, 560
    text(d, (tx, ty), "Tones (spec S1): the module picks a word, the hub picks the colour", TEXT, F_B, CANVAS)
    for i, (k, lab) in enumerate([("neutral", "Off air"), ("active", "Following"), ("live", "Muted"),
                                  ("ok", "Rest"), ("attention", "Needs setup"), ("off", "OBS isn't open")]):
        bx = tx + (i % 3) * 230
        by = ty + 34 + (i // 3) * 64
        rr(d, [bx, by, bx + 216, by + 52], fill=SURFACE, r=6)
        text(d, (bx + 12, by + 4), k, MUTED, F_S, SURFACE, f"tone name {k}")
        text(d, (bx + 12, by + 20), lab, TONE[k], F_B, SURFACE, f"tone {k}")
    notes = [
        "Tiles show only for modules that are on. A module turned off leaves the tray panel; Modules has Turn on.",
        "First-party tiles arrive shown (UX8); OBS, Corsair iCUE and OpenRGB only once their app has been seen (question 3).",
        "Colour is never the only signal: the value word always says the state (\"Muted\", \"Live\"), the tone only repeats it.",
    ]
    for i, ln in enumerate(notes):
        text(d, (30, 800 + i * 24), ln, MUTED, F_S, CANVAS)
    return im


# 2. Every state per module ------------------------------------------------------------------------
STATE_ROWS = [
    ("Mic mute", [
        ("on", T(MIC, value="Mic on", sub="Default communications mic", tone="neutral", button="Mute", key="F13", primary=False)),
        ("muted", T(MIC, value="Muted", sub="Default communications mic", tone="live", button="Unmute", key="F13")),
        ("no mic", T(MIC, value="No microphone", sub="Windows has no default mic", tone="off")),
        ("refused", T(MIC, value="Couldn't mute", sub="Windows said no. Try again.", tone="attention", button="Try again")),
    ]),
    ("Focus timer", [
        ("focus", T(FOC, value="18:42", sub="Focus · round 2 of 4", tone="active", meter=0.25, button="Pause", key="F14")),
        ("rest", T(FOC, value="3:10", sub="Rest · then round 3", tone="ok", meter=0.37, button="Pause", key="F14")),
        ("paused", T(FOC, value="18:42", sub="Paused", tone="off", button="Resume", key="F14")),
        ("no keyboard", T(FOC, value="18:42", sub="Keyboard not connected", tone="active", meter=0.25, button="Pause")),
    ]),
    ("OBS", [
        ("off air", T(OBS, value="Off air", sub="Scene: Starting soon", tone="neutral")),
        ("recording", T(OBS, value="Recording", sub="Scene: Gameplay", tone="live")),
        ("OBS closed", T(OBS, value="OBS isn't open", sub="Shows here when you open it", tone="off")),
        ("can't reach", T(OBS, value="Can't reach OBS", sub="Turn on its WebSocket server", tone="attention", button="How to", primary=False)),
        ("password", T(OBS, value="Needs password", sub="OBS asks for one", tone="attention", button="Enter password")),
    ]),
    ("Corsair iCUE", [
        ("following", T(ICUE, value="Following", sub="K70 RGB PRO", tone="active")),
        ("first run", T(ICUE, value="Needs setup", sub="Corsair's iCUE SDK file", tone="attention", button="Set up")),
        ("iCUE closed", T(ICUE, value="iCUE isn't open", sub="Follows again when it opens", tone="off")),
        ("SDK off", T(ICUE, value="iCUE said no", sub="Turn on SDK in iCUE settings", tone="attention", button="How to", primary=False)),
        ("no device", T(ICUE, value="No device found", sub="Nothing matches \"dark core\"", tone="attention", button="Change device", primary=False)),
    ]),
    ("OpenRGB", [
        ("following", T(ORGB, value="Following", sub="OpenRGB on this PC", tone="active")),
        ("first run", T(ORGB, value="Waiting for OpenRGB", sub="Add Foxglove in OpenRGB", tone="attention", button="How to connect")),
        ("quiet", T(ORGB, value="Waiting for OpenRGB", sub="Last colours 3 min ago", tone="off")),
        ("port busy", T(ORGB, value="Port 5568 is busy", sub="Another app uses it", tone="attention", button="Try again")),
    ]),
]


def tile_states():
    im, d = m.new(1500, 1100)
    m.heading(d, "Every state per module: normal, empty / first run, outside app missing, error")
    y = 70
    for name, cells in STATE_ROWS:
        text(d, (30, y + 54), name, TEXT, F_B, CANVAS)
        for i, (lab, t) in enumerate(cells):
            x = 170 + i * (TW + 64)
            text(d, (x, y - 2), lab, MUTED, F_S, CANVAS, f"state label {lab}")
            tile(d, x, y + 16, t)
        y += TH + 54
    text(d, (30, y), "Outside problems (OBS closed, iCUE said no, port busy) are tile states, never crashes: the module stays running and says what it", MUTED, F_S, CANVAS)
    text(d, (30, y + 20), "needs (spec S6). \"How to\" / \"Set up\" open the module's page at its How to connect section (spec S2). OBS never gets a Start/Stop button.", MUTED, F_S, CANVAS)
    return im


# 3. Supervisor states on a tile and on a page -------------------------------------------------------
def supervisor():
    im, d = m.new(1500, 820)
    m.heading(d, "When the supervisor has the module: the same tile and page, the §7.8 states")
    cells = [
        ("Starting… (cached tile)", T(MIC, value="Muted", sub="Default communications mic", tone="live", button="Unmute", key="F13", state="starting")),
        ("Restarting (amber strip)", T(MIC, value="Muted", sub="Default communications mic", tone="live", button="Unmute", key="F13", state="restarting")),
        ("Stopped after 5 crashes", T(MIC, value="", state="stopped")),
        ("Needs a newer Hideout", T(OBS, value="", state="needs-hub")),
        ("Panel guard (§5.5)", T(ICUE, value="", state="quarantined")),
    ]
    for i, (lab, t) in enumerate(cells):
        x = 30 + i * (TW + 92)
        text(d, (x, 70), lab, MUTED, F_S, CANVAS, f"sup label {lab}")
        tile(d, x, 92, t)
    text(d, (30, 240), "Restarting: button greyed, tooltip \"Mic mute is restarting. A key press waits up to 2 s.\" The value stays, greyed, with \"Last seen\": a stale", MUTED, F_S, CANVAS)
    text(d, (30, 258), "\"Muted\" must never look current. Stopped: the button is Open (the page has the recovery ladder), as the tray menu says \"Open Mic mute to start it again\".", MUTED, F_S, CANVAS)

    # page with crash bar
    m.window(d, [30, 300, 740, 780])
    m.strip(d, 31, 302, 708, [{"key": "keyboard", "name": "Keyboard"}, {"key": "lights", "name": "Mic mute", "state": "updating"}], 1)
    rr(d, [44, 348, 726, 414], fill=AMBER_BG, r=6)
    d.text((58, 364), GL["sync"], fill=AMBER, font=ICON)
    PAIRS.append(("crash bar icon", AMBER, AMBER_BG))
    text(d, (84, 354), "Mic mute stopped unexpectedly. Restarting in 4 s (2 of 5).", TEXT, F_B, AMBER_BG, "crash bar title")
    text(d, (84, 380), "Your place on this page is kept.", TEXT, F, AMBER_BG, "crash bar line")
    text(d, (640, 380), "Open log", AMBER, F, AMBER_BG, "crash bar link")
    text(d, (50, 430), "Mic mute", TEXT, F_H, BG)
    text(d, (50, 470), "Muted", GREY_FG, F_XL, BG, "greyed glance on page")
    text(d, (50, 516), "Last seen 12 s ago. Windows keeps the mic as it was.", MUTED, F, BG, "stale line on page")
    rr(d, [50, 548, 150, 580], outline=BORDER, r=5)
    text(d, (62, 552), "Unmute", MUTED, F, BG, "disabled button")
    text(d, (50, 600), "Keys", TEXT, F_B, BG)
    text(d, (50, 626), "Mute or unmute the microphone", GREY_FG, F, BG, "greyed row")
    chip(d, 300, 628, "F13", BG)
    text(d, (50, 740), "Supervisor UX: hideout-supervisor-ux.md (crash bar, stopped card, ladder).", MUTED, F_S, BG)

    # page stopped card
    m.window(d, [770, 300, 1470, 780])
    m.strip(d, 771, 302, 698, [{"key": "keyboard", "name": "Keyboard"}, {"key": "lights", "name": "OpenRGB", "state": "crash"}], 1)
    text(d, (790, 354), "OpenRGB", TEXT, F_H, BG)
    rr(d, [790, 392, 1450, 548], fill=SURFACE, outline=DANGER, r=6)
    d.text((806, 408), GL["warn"], fill=DANGER, font=ICON)
    PAIRS.append(("card stop mark", DANGER, SURFACE))
    text(d, (830, 404), "Stopped after 5 crashes", TEXT, F_B, SURFACE)
    text(d, (830, 428), "OpenRGB crashed 5 times in 10 minutes.", MUTED, F, SURFACE)
    bx = button(d, 830, 462, "Start again", True)
    button(d, bx, 462, "Start with default settings")
    text(d, (830, 510), "Copy diagnostics", ACCENT, F, SURFACE, "link on surface")
    text(d, (790, 570), "Lights: the keyboard goes back to its own lighting while OpenRGB is", MUTED, F, BG)
    text(d, (790, 592), "stopped (lights handed back, §7.9). The page says so in one line.", MUTED, F, BG)
    return im


# 4. Pages: one layout, three modules ------------------------------------------------------------------
def section(d, x, y, title):
    text(d, (x, y), title, MUTED, F_B, BG, f"section {title}")
    return y + 26


def page_frame(d, box, title, icon):
    m.window(d, box)
    x0, y0 = box[0], box[1]
    m.strip(d, x0 + 1, y0 + 2, box[2] - x0 - 2, [{"key": "lights", "name": title}], 0)
    d.text((x0 + 20, y0 + 58), GL[icon], fill=MUTED, font=m.font("segmdl2.ttf", 18))
    text(d, (x0 + 48, y0 + 52), title, TEXT, F_H, BG)
    return x0 + 20, y0 + 96


def pages():
    im, d = m.new(1500, 960)
    m.heading(d, "Pages: Now · How to connect · On your keyboard · Keys · Settings · About (same order everywhere)")

    # Mic mute, normal
    x, y = page_frame(d, [30, 70, 500, 900], "Mic mute", "mic")
    y = section(d, x, y, "Now")
    text(d, (x, y), "Muted", DANGER, F_XL, BG, "page glance live")
    text(d, (x, y + 46), "Default communications mic: Headset (USB)", MUTED, F, BG)
    button(d, x, y + 76, "Unmute", True)
    y += 130
    y = section(d, x, y, "On your keyboard")
    rr(d, [x, y, x + 64 * 2, y + 52 * 2], fill=(0, 0, 0), outline=BORDER, r=2)
    text(d, (x + 30, y + 40), "MUTED", (0xFF, 0xFF, 0xFF), F_B, (0, 0, 0), "info screen preview")
    text(d, (x + 144, y + 4), "Info screen: MUTED", TEXT, F, BG)
    text(d, (x + 144, y + 26), "Lights: red while muted", TEXT, F, BG)
    y += 122
    y = section(d, x, y, "Keys")
    for a, k in [("Mute or unmute the microphone", "F13"), ("Mute the microphone", None), ("Unmute the microphone", None)]:
        text(d, (x, y), a, TEXT, F, BG)
        if k:
            chip(d, x + 330, y + 2, k, BG)
        else:
            text(d, (x + 330, y), "Set key", ACCENT, F, BG, "set key link")
        y += 26
    y += 10
    y = section(d, x, y, "About")
    for ln in ["This is Windows' mute for the default", "communications mic. Apps like Discord can still", "mute themselves."]:
        text(d, (x, y), ln, MUTED, F, BG)
        y += 22

    # OBS, can't reach
    x, y = page_frame(d, [520, 70, 990, 900], "OBS", "video")
    y = section(d, x, y, "Now")
    text(d, (x, y), "Can't reach OBS", AMBER, F_BIG, BG, "page glance attention")
    text(d, (x, y + 38), "OBS is open, but its WebSocket server is off.", MUTED, F, BG)
    y += 76
    y = section(d, x, y, "How to connect")
    steps = ["In OBS, open Tools > WebSocket Server Settings.", "Tick Enable WebSocket server.",
             "Leave Server Port at 4455.", "If Enable Authentication is ticked, copy the", "password, then click Enter password below."]
    nums = [1, 2, 3, 4, None]
    for n, s in zip(nums, steps):
        if n:
            text(d, (x, y), f"{n}.", ACCENT, F_B, BG, "step number")
        text(d, (x + 22, y), s, TEXT, F, BG)
        y += 24
    y += 6
    button(d, x, y, "Enter password")
    text(d, (x + 160, y + 6), "Hideout connects by itself.", MUTED, F_S, BG)
    y += 52
    y = section(d, x, y, "On your keyboard")
    text(d, (x, y), "Info screen: REC or LIVE and the scene", TEXT, F, BG)
    text(d, (x, y + 22), "Lights: red while recording or live", TEXT, F, BG)
    y += 60
    y = section(d, x, y, "Settings")
    text(d, (x, y), "OBS WebSocket password", TEXT, F, BG)
    rr(d, [x, y + 24, x + 300, y + 54], fill=SURFACE, outline=BORDER, r=4)
    text(d, (x + 10, y + 30), "Not set", MUTED, F, SURFACE, "secret field")
    y += 74
    y = section(d, x, y, "About")
    for ln in ["Hideout only listens to OBS. It never starts,", "stops or switches anything."]:
        text(d, (x, y), ln, MUTED, F, BG)
        y += 22

    # Corsair iCUE, first run
    x, y = page_frame(d, [1010, 70, 1480, 900], "Corsair iCUE", "color")
    y = section(d, x, y, "Now")
    text(d, (x, y), "Needs setup", AMBER, F_BIG, BG, "page glance attention 2")
    text(d, (x, y + 38), "Your keyboard's underglow can follow a Corsair", MUTED, F, BG)
    text(d, (x, y + 60), "device. Hideout needs one file from Corsair.", MUTED, F, BG)
    y += 96
    y = section(d, x, y, "How to connect")
    steps = [(1, "Download the iCUE SDK from Corsair."), (2, "Unzip it."),
             (3, "Click Choose file and pick"), (None, "iCUESDK.x64_2019.dll."),
             (4, "In iCUE, open Settings and turn on SDK.")]
    for n, s in steps:
        if n:
            text(d, (x, y), f"{n}.", ACCENT, F_B, BG, "step number")
        text(d, (x + 22, y), s, TEXT, F, BG)
        y += 24
    text(d, (x + 22, y - 120), "", TEXT, F, BG)
    rr(d, [x + 22, y + 4, x + 300, y + 34], fill=SURFACE, outline=BORDER, r=4)
    text(d, (x + 32, y + 10), "No file chosen", MUTED, F, SURFACE, "file field")
    button(d, x + 310, y + 3, "Choose file")
    y += 56
    text(d, (x, y), "Download page", ACCENT, F, BG, "link")
    text(d, (x + 120, y), "opens corsair.com in your browser.", MUTED, F_S, BG)
    y += 34
    y = section(d, x, y, "Settings")
    for lab, val in [("Corsair device to follow", "First one with lights"), ("Colour reads per second", "10")]:
        text(d, (x, y), lab, TEXT, F, BG)
        rr(d, [x, y + 24, x + 300, y + 54], fill=SURFACE, outline=BORDER, r=4)
        text(d, (x + 10, y + 30), val, MUTED, F, SURFACE, "field")
        y += 66
    y = section(d, x, y, "About")
    for ln in ["Mirroring only: Hideout reads colours and", "never takes control of Corsair lighting.",
               "Foxglove ships no Corsair files."]:
        text(d, (x, y), ln, MUTED, F, BG)
        y += 22
    return im


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, fn in [("tiles", tiles_overview), ("tile-states", tile_states),
                     ("supervisor", supervisor), ("pages", pages)]:
        path = os.path.join(OUT, f"ux-h28-proposed-{name}.png")
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
