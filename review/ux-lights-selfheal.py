"""Drawn mockup for docs/lights-selfheal-ux.md (Python + Pillow, not an egui render).

python review/ux-lights-selfheal.py  ->  review/ux-lights-selfheal.png, and prints the contrast
table (WCAG 2.x) for every text/background pair the mockup uses, from theme.rs's tokens.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

OUT = Path(__file__).with_name("ux-lights-selfheal.png")
FONTS = Path("C:/Windows/Fonts")

# theme.rs dark() / light(), accent from resources/palettes/bandit.json
DARK = dict(ground=0x0E1211, surface=0x161C1A, raised=0x1D2422, line=0x2A3330, control=0x232B28,
            control_line=0x34403B, text=0xD9E0DC, strong=0xF2F5F3, muted=0x8E9A94, accent=0xF08A3C,
            on_accent=0x140D07, ok=0x5FC48E, warn=0xF0B53C, danger=0xE5604F)
LIGHT = dict(ground=0xF1F4F2, surface=0xFFFFFF, raised=0xE8EDE9, line=0xCFD8D3, control=0xE8EDE9,
             control_line=0xB9C4BE, text=0x26302C, strong=0x0E1211, muted=0x5A6660, accent=0xB4540F,
             on_accent=0xFFFFFF, ok=0x237A4B, warn=0x8F6100, danger=0xB8352A)


def rgb(h):
    return (h >> 16 & 255, h >> 8 & 255, h & 255)


def lum(h):
    def ch(c):
        c /= 255
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = rgb(h)
    return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)


def contrast(a, b):
    la, lb = sorted((lum(a), lum(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


PAIRS = [("text", "surface"), ("muted", "surface"), ("ok", "surface"), ("warn", "surface"),
         ("danger", "surface"), ("accent", "surface"), ("on_accent", "accent"),
         ("text", "control"), ("muted", "raised"), ("text", "raised")]

F = lambda size, bold=False: ImageFont.truetype(str(FONTS / ("seguisb.ttf" if bold else "segoeui.ttf")), size)
MONO = ImageFont.truetype(str(FONTS / "consola.ttf"), 12)
W, COL = 1500, 720


def wrap(d, text, font, width):
    words, lines, cur = text.split(" "), [], ""
    for w in words:
        t = (cur + " " + w).strip()
        if d.textlength(t, font=font) <= width:
            cur = t
        else:
            lines.append(cur)
            cur = w
    lines.append(cur)
    return lines


def card(d, p, x, y, w, title, body_h):
    d.rounded_rectangle((x, y, x + w, y + body_h), 8, fill=rgb(p["surface"]), outline=rgb(p["line"]))
    d.text((x + 16, y + 12), title, font=F(15, True), fill=rgb(p["strong"]))
    return y + 42


def button(d, p, x, y, label, primary=False, enabled=True):
    f = F(13)
    w = d.textlength(label, font=f) + 24
    if primary:
        d.rounded_rectangle((x, y, x + w, y + 26), 5, fill=rgb(p["accent"]))
        d.text((x + 12, y + 4), label, font=f, fill=rgb(p["on_accent"]))
    else:
        d.rounded_rectangle((x, y, x + w, y + 26), 5, fill=rgb(p["control"]), outline=rgb(p["control_line"]))
        d.text((x + 12, y + 4), label, font=f, fill=rgb(p["text"] if enabled else p["muted"]))
    return x + w + 8


def health_row(d, p, x, y, w, level, item, detail, tag):
    colour = {"OK": p["ok"], "Warning": p["warn"], "Failed": p["danger"]}[level]
    d.text((x, y - 18), tag, font=F(11), fill=rgb(p["muted"]))
    d.text((x, y), level, font=F(13, True), fill=rgb(colour))
    lines = wrap(d, f"{item}: {detail}", F(13), w - 80)
    for i, ln in enumerate(lines):
        d.text((x + 72, y + i * 19), ln, font=F(13), fill=rgb(p["text"]))
    return y + len(lines) * 19 + 30


def column(img, p, x0, name):
    d = ImageDraw.Draw(img)
    d.rectangle((x0, 0, x0 + COL + 30, 1190), fill=rgb(p["ground"]))
    d.text((x0 + 20, 16), f"{name} theme", font=F(13, True), fill=rgb(p["muted"]))
    x, w = x0 + 20, COL - 10
    # 1. Keyboard health card (App page), the new row in its states
    y = card(d, p, x, 44, w, "Keyboard health  (App page)", 420)
    bx = button(d, p, x + 16, y, "Check now")
    d.text((bx + 4, y + 4), "Checked just now", font=F(13), fill=rgb(p["muted"]))
    y += 58
    item = "Keyboard lights during scenes"
    for level, detail, tag in [
        ("OK", "never went dark since Foxglove started", "healthy"),
        ("OK", "went dark, putting the lights back now…", "putting back (about 2 min dark)"),
        ("OK", "went dark 2 times since start, put back each time (last at 14:05)", "recovered"),
        ("Warning", "went dark 4 times in the last hour, put back each time (last at 14:05)", "keeps going dark"),
        ("Failed", "dark since 14:05 and Foxglove couldn't put the lights back. Click Repaint now on the Scenes page.", "not recovered"),
    ]:
        y = health_row(d, p, x + 16, y, w - 16, level, item, detail, tag)
    # 2. Scenes page status line + buttons
    y = card(d, p, x, 480, w, "Scenes page, scene status", 330)
    for tag, line, tone, hint, buttons in [
        ("running (unchanged)", "Running: Bandit scanline", "ok", None, [("Stop", False, True)]),
        ("putting back", "Running: Bandit scanline · keyboard went dark, putting the lights back…", "warn", None,
         [("Stop", False, True), ("Repainting…", False, False)]),
        ("not recovered", "Running: Bandit scanline · keyboard lights dark since 14:05", "warn",
         "Repaint now sends the scene's colours again. Stop gives the keyboard its own lights back.",
         [("Repaint now", True, True), ("Stop", False, True)]),
    ]:
        d.text((x + 16, y - 4), tag, font=F(11), fill=rgb(p["muted"]))
        y += 14
        d.text((x + 16, y), line, font=F(13, True), fill=rgb(p[tone]))
        y += 22
        if hint:
            for ln in wrap(d, hint, F(13), w - 40):
                d.text((x + 16, y), ln, font=F(13), fill=rgb(p["muted"]))
                y += 19
        bx = x + 16
        for label, prim, en in buttons:
            bx = button(d, p, bx, y + 2, label, prim, en)
        y += 46
    # 3. Notification (FAIL only, at most once a day)
    y = 830
    d.text((x, y), "Notification (Failed only, once a day)", font=F(11), fill=rgb(p["muted"]))
    d.rounded_rectangle((x, y + 18, x + 420, y + 118), 8, fill=rgb(p["raised"]), outline=rgb(p["line"]))
    d.text((x + 14, y + 28), "Keyboard lights stayed dark", font=F(14, True), fill=rgb(p["text"]))
    for i, ln in enumerate(wrap(d, "A scene is running but the keyboard shows no light. Open the Scenes page and click Repaint now.", F(13), 390)):
        d.text((x + 14, y + 52 + i * 19), ln, font=F(13), fill=rgb(p["muted"]))


def main():
    img = Image.new("RGB", (W, 1200), rgb(0x101010))
    column(img, DARK, 0, "Dark")
    column(img, LIGHT, 750, "Light")
    d = ImageDraw.Draw(img)
    # 4. CLI
    y = 990
    d.rectangle((0, y - 20, W, 1200), fill=(12, 12, 12))
    d.text((20, y - 12), "foxglove-cli scene  (status | stop | repaint)", font=F(13, True), fill=(200, 200, 200))
    blocks = [
        ("> foxglove-cli scene", ["scene: Bandit scanline (running)", "keyboard lights: lit",
                                  "went dark: 2 times since start, put back each time (last at 14:05)",
                                  "detail: held over USB, owner USB, takes 14, hand-backs 13,", "        frames drawn 5120 / sent 5123", "(exit 0)"]),
        ("> foxglove-cli scene", ["scene: Bandit scanline (running)", "keyboard lights: DARK since 14:05, next try at 14:35",
                                  "went dark: 3 times since start, 2 put back", "next: foxglove-cli scene repaint, or foxglove-cli scene stop",
                                  "detail: held over USB, owner USB, takes 15, hand-backs 14,", "        frames drawn 0 / sent 5402", "(exit 3)"]),
        ("> foxglove-cli scene repaint", ["scene: repainted Bandit scanline", "keyboard lights: lit", "(exit 0)", "",
                                          "> foxglove-cli scene stop", "scene: stopped Bandit scanline; the keyboard's own lights are back", "(exit 0)"]),
    ]
    xs = [20, 520, 1020]
    for (cmd, lines), bx in zip(blocks, xs):
        d.text((bx, y + 14), cmd, font=MONO, fill=(240, 138, 60))
        for i, ln in enumerate(lines):
            d.text((bx, y + 34 + i * 17), ln, font=MONO, fill=(217, 224, 220) if not ln.startswith("(exit") else (142, 154, 148))
    d.text((20, 1172), "Drawn mockup (Pillow, Segoe UI), not an egui render. Text widths estimated. docs/lights-selfheal-ux.md",
           font=F(12), fill=(142, 154, 148))
    img.save(OUT)
    for name, p in (("dark", DARK), ("light", LIGHT)):
        print(name, ", ".join(f"{a}/{b} {contrast(p[a], p[b]):.1f}" for a, b in PAIRS))


if __name__ == "__main__":
    main()
