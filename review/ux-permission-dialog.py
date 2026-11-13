# Renders review/ux-permission-dialog.png: the module permission dialog (light + dark),
# a failure state and the CLI transcript. Mockup with the hub's theme tokens, not an egui render.
import sys
from PIL import Image, ImageDraw, ImageFont

OUT = sys.argv[1]
F = r"C:\Windows\Fonts"
def font(name, size):
    return ImageFont.truetype(f"{F}\\{name}", size)
R = font("segoeui.ttf", 15); B = font("segoeuib.ttf", 15); H = font("segoeuib.ttf", 20)
S = font("segoeui.ttf", 13); SB = font("segoeuib.ttf", 13); M = font("consola.ttf", 13)
T = font("segoeuib.ttf", 24)

LIGHT = dict(bg=(0xfa,0xf8,0xf6), chrome=(0xf0,0xec,0xe8), surface=(0xff,0xff,0xff), border=(0xd8,0xd2,0xcc),
             text=(0x1f,0x1b,0x18), muted=(0x5c,0x55,0x4f), accent=(0xb8,0x4a,0x00), on_accent=(0xff,0xff,0xff),
             danger=(0xb0,0x1e,0x1e), ok=(0x1e,0x6b,0x34), attention=(0x9a,0x4a,0x00))
DARK = dict(bg=(0x17,0x15,0x14), chrome=(0x20,0x1d,0x1b), surface=(0x2a,0x26,0x23), border=(0x3d,0x37,0x33),
            text=(0xf2,0xee,0xea), muted=(0xa8,0xa0,0x99), accent=(0xff,0x8a,0x2a), on_accent=(0x1a,0x0e,0x04),
            danger=(0xff,0x6b,0x6b), ok=(0x6c,0xd0,0x8a), attention=(0xff,0xb0,0x5a))

def lum(c):
    def ch(v):
        v /= 255
        return v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4
    r, g, b = (ch(x) for x in c)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b
def contrast(a, b):
    la, lb = sorted((lum(a), lum(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)

PAIRS = [("text/surface", "text", "surface"), ("muted/surface", "muted", "surface"),
         ("accent/surface (outline button, links)", "accent", "surface"),
         ("on_accent/accent (Not now)", "on_accent", "accent"),
         ("attention/surface (group 'Makes changes')", "attention", "surface"),
         ("danger/surface (sensitive, can't be allowed)", "danger", "surface"),
         ("ok/surface ('Allowed')", "ok", "surface"), ("text/chrome (header line)", "text", "chrome"),
         ("muted/chrome", "muted", "chrome"), ("muted/surface checkbox outline (non-text, 3:1)", "muted", "surface")]
for name, a, b in PAIRS:
    print(f"| {name} | {contrast(LIGHT[a], LIGHT[b]):.1f} | {contrast(DARK[a], DARK[b]):.1f} |")
print("border/surface", round(contrast(LIGHT['border'], LIGHT['surface']), 2), round(contrast(DARK['border'], DARK['surface']), 2))

GROUPS = [
    ("Share its status", "muted", [
        ("Share your keyboard's status in Hideout", "which keyboard, battery, layer, lights"),
        ("See which modules are installed and running", None)]),
    ("Answer questions from apps", "muted", [
        ("The keyboard's health report", None), ("What the Lights page shows", None),
        ("Which scenes and wallpaper looks are listed", None), ("Where the layer overlay window is", None),
        ("Whether the layer overlay is on", None), ("Whether the tray panel is open", None),
        ("Your PC's lighting zones", None)]),
    ("Make changes when an app asks", "attention", [
        ("Bring the layer overlay back on screen", None), ("Turn the layer overlay on or off", None),
        ("Open or close the tray panel", None), ("Change a PC lighting zone's colour", None)]),
]

def checkbox(d, x, y, t, on=True):
    d.rounded_rectangle([x, y, x + 16, y + 16], 3, outline=t["muted"], width=2,
                        fill=t["accent"] if on else t["surface"])
    if on:
        d.line([x + 4, y + 8, x + 7, y + 12, x + 13, y + 4], fill=t["on_accent"], width=2)

def button(d, x, y, label, t, main=False, focus=False):
    w = int(d.textlength(label, font=B)) + 32
    if main:
        d.rounded_rectangle([x, y, x + w, y + 34], 6, fill=t["accent"])
        d.text((x + 16, y + 7), label, font=B, fill=t["on_accent"])
    else:
        d.rounded_rectangle([x, y, x + w, y + 34], 6, outline=t["accent"], width=2, fill=t["surface"])
        d.text((x + 16, y + 7), label, font=B, fill=t["accent"])
    if focus:
        d.rounded_rectangle([x - 4, y - 4, x + w + 4, y + 38], 8, outline=t["text"], width=2)
    return w

def dialog(img, ox, oy, t, label):
    d = ImageDraw.Draw(img)
    W, Hh = 620, 830
    d.rectangle([ox - 20, oy - 46, ox + W + 20, oy + Hh + 20], fill=t["bg"])
    d.text((ox, oy - 38), label, font=SB, fill=t["muted"])
    d.rounded_rectangle([ox, oy, ox + W, oy + Hh], 10, fill=t["surface"], outline=t["border"])
    x, y = ox + 28, oy + 24
    d.text((x, y), "Let Foxglove run in Hideout?", font=H, fill=t["text"]); y += 36
    d.text((x, y), "Foxglove asks for these 13 things. Each app that uses them still", font=R, fill=t["text"]); y += 21
    d.text((x, y), "needs its own permission. Nothing leaves this PC.", font=R, fill=t["text"]); y += 32
    checkbox(d, x, y + 2, t); d.text((x + 26, y), "All 13", font=B, fill=t["text"]); y += 32
    for title, col, items in GROUPS:
        d.text((x, y), title.upper(), font=SB, fill=t[col]); y += 24
        for name, sub in items:
            checkbox(d, x + 4, y + 2, t)
            d.text((x + 30, y), name, font=R, fill=t["text"])
            if sub:
                y += 20; d.text((x + 30, y), sub, font=S, fill=t["muted"])
            y += 26
        y += 8
    d.text((x, y), "+  Also let Foxglove handle your keyboard for other apps (off)", font=R, fill=t["accent"]); y += 34
    d.line([x, y, ox + W - 28, y], fill=t["border"]); y += 14
    d.text((x, y), "Only for this copy of Foxglove:", font=R, fill=t["text"]); y += 22
    d.text((x, y), r"C:\Program Files\Foxglove\foxglove.exe", font=M, fill=t["text"]); y += 22
    d.text((x, y), "If the program or this list changes, Hideout asks again.", font=S, fill=t["muted"]); y += 20
    d.text((x, y), "You can turn each one off any time in Modules, Foxglove.", font=S, fill=t["muted"]); y += 20
    d.text((x, y), "List code 7f3a91c20b4e55d1", font=S, fill=t["muted"]); y += 34
    w = button(d, x, y, "Allow 13", t)
    button(d, x + w + 14, y, "Not now", t, main=True, focus=True)
    d.text((x + w + 140, y + 9), "Enter, Esc, close = Not now", font=S, fill=t["muted"])

def changed(img, ox, oy, t):
    d = ImageDraw.Draw(img)
    W = 620
    d.rounded_rectangle([ox, oy, ox + W, oy + 150], 10, fill=t["surface"], outline=t["border"])
    x, y = ox + 28, oy + 20
    d.text((x, y), "Let Foxglove run in Hideout?", font=H, fill=t["text"]); y += 34
    d.text((x, y), "The list changed while it was open. Nothing was allowed.", font=B, fill=t["attention"]); y += 24
    d.text((x, y), "Check the new list below, then choose again.", font=R, fill=t["text"]); y += 30
    d.text((x, y), "Can't be allowed: Change a PC lighting zone's colour", font=R, fill=t["danger"]); y += 20
    d.text((x, y), "Another program already answers these.", font=S, fill=t["muted"])

def terminal(img, ox, oy):
    d = ImageDraw.Draw(img)
    W, Hh = 1300, 440
    d.rounded_rectangle([ox, oy, ox + W, oy + Hh], 8, fill=(0x0c, 0x0c, 0x0c))
    lines = [
        ("> hideout modules permissions foxglove", (0xcc, 0xcc, 0xcc)),
        ("Foxglove 0.3.0 (adopted) asks for 13. None allowed yet.", (0xf2, 0xf2, 0xf2)),
        ("Program: C:\\Program Files\\Foxglove\\foxglove.exe  sha256 3c0d…9e21", (0xf2, 0xf2, 0xf2)),
        ("  Share its status            facts.write:keyboard.*   Share your keyboard's status in Hideout", (0xf2, 0xf2, 0xf2)),
        ("  ...                         (13 lines, one per permission)", (0x9a, 0x9a, 0x9a)),
        ("  Make changes when asked     provide:pc_lighting.set_zone   Change a PC lighting zone's colour", (0xf2, 0xf2, 0xf2)),
        ("List code 7f3a91c20b4e55d1. To allow: hideout modules grant foxglove --all-listed --listed 7f3a91c20b4e55d1", (0xf2, 0xf2, 0xf2)),
        ("", None),
        ("> hideout modules grant foxglove --all-listed", (0xcc, 0xcc, 0xcc)),
        ("Not allowed: --all-listed needs --listed <code> from 'hideout modules permissions foxglove'.   (exit 3)", (0xff, 0xb0, 0x5a)),
        ("", None),
        ("> hideout modules grant foxglove --all-listed --listed 7f3a91c20b4e55d1", (0xcc, 0xcc, 0xcc)),
        ("Allowed 13 for Foxglove 0.3.0 (this copy: C:\\Program Files\\Foxglove\\foxglove.exe):", (0x6c, 0xd0, 0x8a)),
        ("  facts.read:module.*, facts.write:keyboard.*, provide:health.read, ... (all 13 printed)", (0xf2, 0xf2, 0xf2)),
        ("Hideout shows this on Modules, Foxglove: 'Allowed from the command line'. Next: hideout modules enable foxglove", (0xf2, 0xf2, 0xf2)),
        ("", None),
        ("> hideout modules grant foxglove --all-listed --listed 7f3a91c20b4e55d1      (manifest changed meanwhile)", (0xcc, 0xcc, 0xcc)),
        ("Not allowed: the list changed since you read it (permission.list_changed). Read it again.   (exit 3)", (0xff, 0xb0, 0x5a)),
    ]
    y = oy + 16
    for s, col in lines:
        if col:
            d.text((ox + 18, y), s, font=M, fill=col)
        y += 22

img = Image.new("RGB", (1360, 1690), (0xe6, 0xe2, 0xde))
d = ImageDraw.Draw(img)
d.text((30, 18), "Module permission dialog (#1178) — mockup with Hideout tokens, not an egui render", font=T, fill=(0x1f, 0x1b, 0x18))
dialog(img, 30, 110, LIGHT, "LIGHT · opens on Turn on / Review… only · Not now focused")
dialog(img, 710, 110, DARK, "DARK · same dialog")
d.text((30, 980), "Failure state: the list changed between showing and Allow (TOCTOU guard)", font=B, fill=(0x1f, 0x1b, 0x18))
changed(img, 30, 1010, LIGHT)
changed(img, 710, 1010, DARK)
d.text((30, 1190), "Headless path (Hub, scripts): read-only list, then a grant bound to the list code; never silent", font=B, fill=(0x1f, 0x1b, 0x18))
terminal(img, 30, 1220)
img.save(OUT)
print("wrote", OUT)
